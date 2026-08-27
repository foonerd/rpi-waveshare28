//! CST328 capacitive touch reader.
//!
//! Reads the controller directly over i2cdev. No kernel input driver, no
//! overlay, no device tree node.
//!
//! Register map is from the Hynitron CST328 datasheet, section 12, normal
//! reporting mode:
//!
//! ```text
//! 0xD000  high nibble finger ID, low nibble status (0x06 = pressed)
//! 0xD001  X_Position >> 4
//! 0xD002  Y_Position >> 4
//! 0xD003  high nibble X & 0x0F, low nibble Y & 0x0F
//! 0xD004  pressure
//! 0xD005  bit 7 button flag, low bits finger count
//! 0xD006  fixed 0xAB
//! 0xD007+ fingers 2 to 5, five bytes each
//! ```
//!
//! Two things learned the hard way on real hardware and worth keeping:
//!
//! 1. The finger count at 0xD005 lags the per-finger status at 0xD000. A
//!    packet can report a finger present in the count while the status nibble
//!    already says lifted. Gate on the status nibble, never on the count.
//! 2. Polling 0xD005 for readiness before reading the packet buys nothing. In
//!    204 captured events on real hardware it never once rejected a sample,
//!    and the latency it added was enough to lose short taps entirely. Read
//!    the packet on the interrupt and validate it, do not pre-poll.
//!
//! Reset timing is from datasheet 10.5: TRST 0.1 ms pulse, TRON 200 ms
//! re-initialisation after release. The 50 ms that circulates in example code
//! is a fourfold shortfall.

use embedded_hal::delay::DelayNs;
use embedded_hal::digital::OutputPin;
use embedded_hal::i2c::I2c;
use std::time::Duration;

/// Base of the touch report block.
const REG_TOUCH_DATA: u16 = 0xD000;
/// Length of the block covering finger 1 and the fixed marker.
const TOUCH_DATA_LEN: usize = 28;
/// Value the marker byte must hold for the packet to be valid.
const MARKER: u8 = 0xAB;
/// Status nibble value meaning a finger is down.
const STATUS_PRESSED: u8 = 0x06;

/// Reset assertion, datasheet TRST is 0.1 ms typical.
pub const RESET_ASSERT: Duration = Duration::from_millis(1);
/// Post-release settle, datasheet TRON is 200 ms typical.
pub const RESET_SETTLE: Duration = Duration::from_millis(250);

/// A validated single-finger touch sample in panel-native coordinates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Touch {
    /// X in panel-native frame, 0 to 239.
    pub x: u16,
    /// Y in panel-native frame, 0 to 319.
    pub y: u16,
    /// Contact pressure as reported by the controller.
    pub pressure: u8,
    /// Number of fingers the controller currently reports.
    pub fingers: u8,
}

/// Errors the reader can produce.
#[derive(Debug, thiserror::Error)]
pub enum TouchError {
    /// The I2C transfer itself failed.
    #[error("i2c transfer failed")]
    Bus,
    /// The packet did not carry the fixed 0xAB marker.
    #[error("packet marker missing")]
    BadMarker,
    /// Driving the reset line failed.
    #[error("reset line")]
    Reset,
}

/// Drive the controller through a reset cycle.
///
/// Timings are from datasheet 10.5, not from example code: TRST is a 0.1 ms
/// pulse and TRON is 200 ms of re-initialisation after release. The 50 ms
/// settle that circulates in published examples is a fourfold shortfall and
/// presents as intermittent failure after power-on rather than a clean fault.
pub fn reset<P, D>(rst: &mut P, delay: &mut D) -> Result<(), TouchError>
where
    P: OutputPin,
    D: DelayNs,
{
    rst.set_low().map_err(|_| TouchError::Reset)?;
    delay.delay_ms(RESET_ASSERT.as_millis() as u32);
    rst.set_high().map_err(|_| TouchError::Reset)?;
    delay.delay_ms(RESET_SETTLE.as_millis() as u32);
    Ok(())
}

/// CST328 reader over any embedded-hal I2C bus.
pub struct Cst328<I> {
    i2c: I,
    addr: u8,
}

impl<I: I2c> Cst328<I> {
    /// Bind to a bus and address. Does not touch the device.
    pub fn new(i2c: I, addr: u16) -> Self {
        Self {
            i2c,
            addr: addr as u8,
        }
    }

    /// Read one touch report.
    ///
    /// Returns `Ok(None)` when the packet is structurally valid but reports no
    /// finger down, which is the normal case on a release edge. Returns
    /// `Err(BadMarker)` when the packet is not a touch report at all.
    ///
    /// Uses a single combined write-read transaction, so the 16-bit register
    /// pointer and the burst read happen without an intervening stop. That is
    /// both faster and less fragile than setting the pointer and then issuing
    /// individual byte reads.
    pub fn read(&mut self) -> Result<Option<Touch>, TouchError> {
        let reg = REG_TOUCH_DATA.to_be_bytes();
        let mut buf = [0u8; TOUCH_DATA_LEN];

        self.i2c
            .write_read(self.addr, &reg, &mut buf)
            .map_err(|_| TouchError::Bus)?;

        if buf[6] != MARKER || buf[0] == MARKER {
            return Err(TouchError::BadMarker);
        }

        if buf[0] & 0x0F != STATUS_PRESSED {
            return Ok(None);
        }

        let x = (u16::from(buf[1]) << 4) | u16::from((buf[3] & 0xF0) >> 4);
        let y = (u16::from(buf[2]) << 4) | u16::from(buf[3] & 0x0F);

        Ok(Some(Touch {
            x,
            y,
            pressure: buf[4],
            // Bit 7 is the button flag, not part of the count.
            fingers: buf[5] & 0x7F,
        }))
    }
}
