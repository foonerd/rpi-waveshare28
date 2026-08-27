//! Touch input, on its own thread.
//!
//! Two reasons this is not in the main loop.
//!
//! First, edges must not be missed. The CST328 asserts IRQ for a few
//! milliseconds when a report is ready. Sampling the line level from a loop
//! that also does a 39 ms SPI redraw and a network poll misses most presses
//! outright, because the pulse is over before the level is next read. The
//! GPIO character device queues edge events in the kernel, so a press that
//! happens while this thread is busy is still delivered afterwards rather
//! than lost.
//!
//! Second, latency. Blocking on an edge event costs nothing while idle and
//! wakes immediately on a press, instead of waiting for whatever the render
//! and poll happen to be doing.

use anyhow::{anyhow, Context, Result};
use gpio_cdev::{Chip, EventRequestFlags, LineRequestFlags};
use linux_embedded_hal::{CdevPin, Delay, I2cdev};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::touch::{self, Cst328};
use crate::ui::{Action, Layout};

const CONSUMER: &str = "waveshare28-panel";

/// Claim the touch controller and start reading it on a background thread.
///
/// Returns once the controller has been reset and the interrupt line claimed,
/// so a wiring or permission problem is reported at startup rather than
/// silently producing a thread that never emits anything.
pub fn spawn(cfg: &Config, layout: Layout, tx: Sender<Action>) -> Result<()> {
    let mut chip = Chip::new(&cfg.gpiochip).with_context(|| format!("opening {}", cfg.gpiochip))?;

    // Reset first, while we still have the line as a plain output.
    let mut rst = {
        let handle = chip
            .get_line(cfg.touch_rst_pin)
            .context("getting touch reset line")?
            .request(LineRequestFlags::OUTPUT, 1, CONSUMER)
            .context("requesting touch reset line")?;
        CdevPin::new(handle).context("wrapping touch reset line")?
    };
    touch::reset(&mut rst, &mut Delay).map_err(|e| anyhow!("resetting touch: {e}"))?;

    // Falling edge: the interrupt is open drain with a pull-up, so it rests
    // high and the controller pulls it low when a report is ready. Reading on
    // the assert rather than waiting for release is what keeps short taps from
    // being missed; the finger is still down when the packet is read.
    let events = chip
        .get_line(cfg.touch_int_pin)
        .context("getting touch interrupt line")?
        .events(
            LineRequestFlags::INPUT,
            EventRequestFlags::FALLING_EDGE,
            CONSUMER,
        )
        .context("requesting touch interrupt events")?;

    let i2c = I2cdev::new(&cfg.i2c_dev).with_context(|| format!("opening {}", cfg.i2c_dev))?;
    let mut ctrl = Cst328::new(i2c, cfg.touch_addr);
    let debounce = Duration::from_millis(cfg.touch_debounce_ms);

    thread::Builder::new()
        .name("touch".into())
        .spawn(move || {
            // Keep the reset line owned for the life of the thread. Dropping
            // it releases the line handle, and the pin reverts to input.
            let _rst = rst;

            let mut last = Instant::now() - debounce;

            for event in events {
                if let Err(e) = event {
                    tracing::warn!(error = %e, "touch interrupt");
                    continue;
                }

                let t = match ctrl.read() {
                    Ok(Some(t)) => t,
                    // No finger down by the time the packet was read. Normal
                    // on a release edge.
                    Ok(None) => continue,
                    Err(e) => {
                        tracing::debug!(error = %e, "touch read");
                        continue;
                    }
                };

                // The controller reports repeatedly while a finger is held.
                if Instant::now().duration_since(last) < debounce {
                    continue;
                }

                let Some(action) = crate::ui::hit(&layout, t) else {
                    continue;
                };
                tracing::debug!(x = t.x, y = t.y, ?action, "touch");
                last = Instant::now();

                // A closed channel means the main loop is gone, so there is
                // nothing left to do.
                if tx.send(action).is_err() {
                    return;
                }
            }
        })
        .context("spawning touch thread")?;

    Ok(())
}
