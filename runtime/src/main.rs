//! Userspace renderer and touch reader for the Waveshare 2.8 inch SPI LCD.
//!
//! Owns the panel directly over `/dev/spidev0.0` and the touch controller over
//! `/dev/i2c-1`. No X, no DRM, no compositor, no kernel display or input
//! driver. Requires only `dtparam=spi=on` in `/boot/userconfig.txt`; I2C is
//! already enabled on Volumio.
//!
//! Note that this is mutually exclusive with the kernel display path. Loading
//! an fbtft or mipi-dbi overlay on spi0 cs0 disables the spidev node, and
//! `/dev/spidev0.0` will not exist for this process to open.

mod config;
mod display;
mod http;
mod state;
mod touch;
mod ui;

use anyhow::{anyhow, Context, Result};
use embedded_hal::digital::InputPin;
use gpio_cdev::{Chip, LineRequestFlags};
use linux_embedded_hal::{CdevPin, Delay, I2cdev};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use config::Config;
use display::Panel;
use state::{Command, CommandSink, PlayerState, StateSource};
use touch::Cst328;
use ui::Action;

/// How often the interrupt line is sampled. The controller refreshes at 120 Hz
/// typical, so 5 ms is well inside its reporting rate while keeping the
/// process asleep for most of every interval.
const TOUCH_POLL: Duration = Duration::from_millis(5);

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let cfg_path = std::env::args()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/etc/waveshare28-panel.toml"));

    let cfg = Config::load(&cfg_path).context("loading configuration")?;
    tracing::info!(?cfg, "starting");

    run(cfg)
}

fn run(cfg: Config) -> Result<()> {
    let poll_interval = Duration::from_millis(cfg.poll_interval_ms);
    let debounce = Duration::from_millis(cfg.touch_debounce_ms);
    // Shorter than the poll interval so a stalled request cannot queue up
    // behind the next one.
    let net_timeout = poll_interval / 2;

    let source = StateSource::new(&cfg.state_url, net_timeout);
    let commands = CommandSink::new(&cfg.command_url, net_timeout);

    let mut panel = Panel::open(&cfg).context("opening panel")?;

    let mut chip = Chip::new(&cfg.gpiochip).with_context(|| format!("opening {}", cfg.gpiochip))?;

    // The interrupt is open drain with a pull-up, so it rests high and the
    // controller pulls it low when a report is ready.
    let int_handle = chip
        .get_line(cfg.touch_int_pin)
        .context("getting touch interrupt line")?
        .request(LineRequestFlags::INPUT, 0, "waveshare28-panel")
        .context("requesting touch interrupt line")?;
    let mut touch_int = CdevPin::new(int_handle).context("wrapping touch interrupt line")?;

    let mut touch_rst = {
        let handle = chip
            .get_line(cfg.touch_rst_pin)
            .context("getting touch reset line")?
            .request(LineRequestFlags::OUTPUT, 1, "waveshare28-panel")
            .context("requesting touch reset line")?;
        CdevPin::new(handle).context("wrapping touch reset line")?
    };

    touch::reset(&mut touch_rst, &mut Delay).map_err(|e| anyhow!("resetting touch: {e}"))?;

    let i2c = I2cdev::new(&cfg.i2c_dev).with_context(|| format!("opening {}", cfg.i2c_dev))?;
    let mut touch = Cst328::new(i2c, cfg.touch_addr);

    panel.backlight(true)?;

    let mut shown: Option<PlayerState> = None;
    let mut current = PlayerState::default();
    let mut next_poll = Instant::now();
    let mut last_action = Instant::now() - debounce;
    let mut int_was_high = true;

    loop {
        if Instant::now() >= next_poll {
            match source.poll() {
                Ok(s) => current = s,
                // A failed poll keeps the last good state on screen. Blanking
                // on a transient failure during a Volumio restart would be
                // worse than showing something slightly stale.
                Err(e) => tracing::warn!(error = %e, "state poll failed"),
            }
            next_poll = Instant::now() + poll_interval;
        }

        // Redraw only on change. A full frame is about 39 ms at 32 MHz, which
        // is cheap but not free, and redrawing an unchanged screen would burn
        // it twice a second for nothing.
        if shown.as_ref() != Some(&current) {
            panel.render(&current)?;
            shown = Some(current.clone());
        }

        // Falling edge means the controller has a report ready. Reading on the
        // assert rather than waiting for release is what keeps short taps from
        // being missed: the finger is still down when the packet is read.
        let int_high = touch_int
            .is_high()
            .map_err(|e| anyhow!("reading touch interrupt: {e:?}"))?;

        if int_was_high && !int_high {
            match touch.read() {
                Ok(Some(t)) => {
                    if Instant::now().duration_since(last_action) >= debounce {
                        if let Some(action) = ui::hit(t) {
                            tracing::debug!(x = t.x, y = t.y, ?action, "touch");
                            if let Some(cmd) = command_for(action) {
                                if let Err(e) = commands.send(cmd) {
                                    tracing::warn!(error = %e, ?cmd, "command failed");
                                } else {
                                    // Do not wait out the poll interval to show
                                    // the result of a press.
                                    next_poll = Instant::now();
                                }
                            }
                            last_action = Instant::now();
                        }
                    }
                }
                // No finger down by the time the packet was read. Normal on a
                // release edge, not worth logging at info.
                Ok(None) => tracing::trace!("touch packet reported no contact"),
                Err(e) => tracing::debug!(error = %e, "touch read"),
            }
        }
        int_was_high = int_high;

        std::thread::sleep(TOUCH_POLL);
    }
}

/// Map a UI action to a player command, or `None` for actions that do not
/// control playback.
fn command_for(action: Action) -> Option<Command> {
    match action {
        Action::Prev => Some(Command::Prev),
        Action::PlayPause => Some(Command::Toggle),
        Action::Next => Some(Command::Next),
        Action::Art => None,
    }
}
