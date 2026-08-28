//! Userspace renderer and touch reader for the Waveshare 2.8 inch SPI LCD.
//!
//! Two display backends. `spi` owns `/dev/spidev0.0` and cannot coexist with
//! an fbtft overlay. `framebuffer` draws into `/dev/fb1` while fbtft holds
//! the bus, which is how Plymouth can run in the initramfs and this process
//! can take over after `plymouth-quit`. Touch is always `/dev/i2c-1`.

mod art;
mod config;
mod display;
mod fbdev;
mod http;
mod input;
mod net;
mod state;
mod touch;
mod ui;

use anyhow::{Context, Result};
use std::path::PathBuf;
use std::sync::mpsc;
use std::time::{Duration, Instant};

use art::{Art, ArtLoader};
use config::Config;
use display::Panel;
use net::{HostInfo, NetMonitor};
use state::{poll_system_status, Command, CommandSink, PlayerState, StateSource, SystemStatus};
use ui::{Action, TextPane};

/// How long the main loop waits for a touch before going round again.
///
/// Also the ticker step interval, so text scrolls at one pixel per tick. A
/// press wakes the loop immediately regardless, because touches arrive on a
/// channel from a thread blocked on a GPIO edge event.
const TICK: Duration = Duration::from_millis(40);
/// How often to ask `/status` while still on the address screen.
const STATUS_POLL: Duration = Duration::from_secs(1);
/// Fall through to `getState` if `/status` never becomes `ready`.
const STATUS_WAIT_CAP: Duration = Duration::from_secs(120);
/// Spinner step on the `starting` footer.
const SPIN_INTERVAL: Duration = Duration::from_millis(400);
const SPIN: [char; 4] = ['|', '/', '-', '\\'];

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            // rustls logs every handshake at debug, which drowns this crate's
            // own output and fills the journal once this runs as a service.
            // Narrowing it here rather than in the unit file means
            // RUST_LOG=debug stays useful without anyone having to remember.
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,rustls=warn".into()),
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
    // Fixed rather than derived from the poll interval. Measured on a Pi 3A+,
    // getState answers in 5 to 11 ms, so this is generous headroom for a
    // loaded board without being long enough to stack requests.
    let net_timeout = Duration::from_secs(2);

    let source = StateSource::new(&cfg.state_url, net_timeout);
    let commands = CommandSink::new(&cfg.command_url, net_timeout);

    let mut panel = Panel::open(&cfg).context("opening panel")?;
    panel.backlight(true)?;
    let layout = *panel.layout();

    let (tx, rx) = mpsc::channel::<Action>();
    input::spawn(&cfg, layout, tx).context("starting touch input")?;

    // Fetching and decoding a cover is slow enough to stall the ticker and
    // delay a touch, so it runs on its own thread and the picture arrives when
    // it arrives.
    let mut loader = ArtLoader::spawn(cfg.art_base.clone(), panel.art_size(), net_timeout)
        .context("starting album art loader")?;
    let mut art: Option<Art> = None;

    let mut pane = TextPane::default();

    let mut shown: Option<PlayerState> = None;
    let mut current = PlayerState::default();
    let mut next_poll = Instant::now();

    // Until the backend is ready and getState succeeds once, the panel
    // shows host addresses. The renderer starts early and deliberately
    // does not wait for volumio.service: getState answers as soon as
    // Express is up, which is well before plugins finish. /status is
    // the gate; the address is the one thing someone needs before the
    // player is reachable.
    let mut player = false;
    let mut backend_ready = false;
    let wait_from = Instant::now();
    let mut next_status = Instant::now();
    let mut next_spin = Instant::now() + SPIN_INTERVAL;
    let mut spin = 0usize;
    let mut last_footer: Option<String> = None;
    let mut netmon = NetMonitor::default();
    let mut host = HostInfo::default();

    loop {
        if !player {
            // Driven by wireless.js touching /tmp/networkstatus, not by a
            // timer over getifaddrs. See src/net.rs.
            let mut dirty = false;
            if let Some(now) = netmon.poll() {
                if now != host {
                    host = now;
                    dirty = true;
                }
            }

            if !backend_ready && Instant::now() >= next_status {
                if poll_system_status(&cfg.status_url, net_timeout) == Some(SystemStatus::Ready) {
                    tracing::info!("backend ready, waiting for player state");
                    backend_ready = true;
                    next_poll = Instant::now();
                }
                next_status = Instant::now() + STATUS_POLL;
                if !backend_ready && wait_from.elapsed() >= STATUS_WAIT_CAP {
                    tracing::warn!(
                        secs = STATUS_WAIT_CAP.as_secs(),
                        "backend still not ready, falling through to getState"
                    );
                    backend_ready = true;
                    next_poll = Instant::now();
                }
            }

            let want_spin = host.has_address() && !backend_ready;
            let spin_due = want_spin && Instant::now() >= next_spin;
            if spin_due {
                spin = (spin + 1) % SPIN.len();
                next_spin = Instant::now() + SPIN_INTERVAL;
            }
            let footer = want_spin.then(|| format!("starting {}", SPIN[spin]));
            if dirty || spin_due || footer != last_footer {
                panel.render_status(&host, footer.as_deref())?;
                last_footer = footer;
            }
        }

        if Instant::now() >= next_poll {
            if backend_ready {
                match source.poll() {
                    Ok(s) => {
                        // First success: leave the status screen. `shown` is
                        // cleared so the next pass draws the player in full,
                        // since none of it is on screen yet.
                        if !player {
                            tracing::info!("player available, switching from status screen");
                            player = true;
                            shown = None;
                        }
                        current = s;
                    }
                    // A failed poll keeps whatever is on screen. Once the
                    // player has been seen once the status screen is never
                    // shown again: a failure during a Volumio restart is
                    // transient, and reverting to an address list mid-
                    // listening would be worse than a slightly stale player.
                    Err(e) => tracing::warn!(error = %e, "state poll failed"),
                }
            }
            next_poll = Instant::now() + poll_interval;
        }

        if !player {
            // Drain touches so the channel cannot fill, and use the same
            // blocking wait as the player path so the loop is not spinning.
            if let Ok(action) = rx.recv_timeout(TICK) {
                tracing::debug!(?action, "touch before player ready, ignored");
            }
            continue;
        }

        pane.set(
            current.title.as_deref().unwrap_or(""),
            current.artist.as_deref().unwrap_or(""),
            current.album.as_deref().unwrap_or(""),
            layout.text,
        );

        if let Some(path) = current.album_art.as_deref() {
            loader.request(path);
        }

        // A decoded cover repaints only the art box. Redrawing the whole
        // screen for it would undo the ticker's current position.
        if let Some(new) = loader.poll() {
            art = new;
            panel.render_art(art.as_ref())?;
        }

        // Full redraw only when the scene changes: a different track, or a
        // transport state change. `seek` advances every second while playing,
        // so gating on the whole state would clear and repaint twice a
        // second, which is visible as a flicker. Progress and volume are
        // repainted in place instead.
        match shown.as_ref() {
            Some(prev) if prev.same_scene(&current) => {
                if prev.seek != current.seek {
                    panel.render_progress(&current)?;
                }
                if prev.volume != current.volume || prev.mute != current.mute {
                    panel.render_volume(&current)?;
                }
                shown = Some(current.clone());
            }
            _ => {
                panel.render(&current, art.as_ref(), &pane)?;
                shown = Some(current.clone());
            }
        }

        // Advance the scroller. Repaint only when something actually moved;
        // text that fits never moves and costs nothing.
        if pane.step() {
            panel.render_rows(&pane)?;
        }

        // Block until a touch arrives or the tick expires. Waiting here rather
        // than sleeping means a press is acted on as soon as the touch thread
        // reports it, instead of after whatever the loop was going to do next.
        match rx.recv_timeout(TICK) {
            Ok(action) => {
                if let Some(cmd) = command_for(action) {
                    if let Err(e) = commands.send(cmd) {
                        tracing::warn!(error = %e, ?cmd, "command failed");
                    } else {
                        // Do not wait out the poll interval to show the result
                        // of a press.
                        next_poll = Instant::now();
                    }
                }
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("touch thread stopped");
            }
        }
    }
}

/// Map a UI action to a player command, or `None` for actions that do not
/// control playback.
fn command_for(action: Action) -> Option<Command> {
    match action {
        Action::Prev => Some(Command::Prev),
        Action::PlayPause => Some(Command::Toggle),
        Action::Next => Some(Command::Next),
        Action::Volume(v) => Some(Command::Volume(v)),
        Action::Art => None,
    }
}
