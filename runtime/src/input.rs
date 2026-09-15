//! Touch input, on its own thread.
//!
//! Edges must not be missed: the CST328 asserts IRQ for a few milliseconds.
//! The GPIO character device queues falling edges. The face needs press,
//! move and release, so a lift that arrives as `Ok(None)` is an `Up`.

use anyhow::{anyhow, Context, Result};
use embedded_graphics::prelude::Point;
use gpio_cdev::{Chip, EventRequestFlags, LineRequestFlags};
use linux_embedded_hal::{CdevPin, Delay, I2cdev};
use std::sync::mpsc::Sender;
use std::thread;
use std::time::{Duration, Instant};

use crate::config::Config;
use crate::touch::{self, Cst328};
use crate::ui::{Layout, TouchEv};

const CONSUMER: &str = "waveshare28-panel";

/// Claim the touch controller and start reading it on a background thread.
pub fn spawn(cfg: &Config, layout: Layout, tx: Sender<TouchEv>) -> Result<()> {
    let mut chip = Chip::new(&cfg.gpiochip).with_context(|| format!("opening {}", cfg.gpiochip))?;

    let mut rst = {
        let handle = chip
            .get_line(cfg.touch_rst_pin)
            .context("getting touch reset line")?
            .request(LineRequestFlags::OUTPUT, 1, CONSUMER)
            .context("requesting touch reset line")?;
        CdevPin::new(handle).context("wrapping touch reset line")?
    };
    touch::reset(&mut rst, &mut Delay).map_err(|e| anyhow!("resetting touch: {e}"))?;

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
            let _rst = rst;
            let mut last_down = Instant::now() - debounce;
            let mut finger = false;
            let mut last_move = Instant::now();
            let mut last_pt = Point::new(0, 0);

            for event in events {
                if let Err(e) = event {
                    tracing::warn!(error = %e, "touch interrupt");
                    continue;
                }

                match ctrl.read() {
                    Ok(Some(t)) => {
                        let p = layout.map(t);
                        if !finger {
                            if Instant::now().duration_since(last_down) < debounce {
                                continue;
                            }
                            finger = true;
                            last_down = Instant::now();
                            last_pt = p;
                            last_move = Instant::now();
                            if tx.send(TouchEv::Down(p)).is_err() {
                                return;
                            }
                        } else if Instant::now().duration_since(last_move)
                            >= Duration::from_millis(40)
                        {
                            last_pt = p;
                            last_move = Instant::now();
                            if tx.send(TouchEv::Move(p)).is_err() {
                                return;
                            }
                        }
                    }
                    Ok(None) => {
                        if finger {
                            finger = false;
                            if tx.send(TouchEv::Up(last_pt)).is_err() {
                                return;
                            }
                        }
                    }
                    Err(e) => tracing::debug!(error = %e, "touch read"),
                }
            }
        })
        .context("spawning touch thread")?;

    Ok(())
}
