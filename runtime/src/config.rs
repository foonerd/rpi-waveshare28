//! Runtime configuration.
//!
//! Defaults match the Waveshare SKU 27579 wiring in BCM numbering. Everything
//! here is overridable so a differently wired board or a clone module with a
//! different touch address does not need a rebuild.

use serde::Deserialize;
use std::path::Path;

/// Panel and host wiring, plus behaviour knobs.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// SPI character device the ST7789V is on.
    pub spi_dev: String,
    /// SPI clock in Hz. The bcm2835 divides core_freq by an even integer, so
    /// the achieved rate is the nearest divisor step, not this exact value.
    pub spi_speed_hz: u32,
    /// BCM pin for display data/command.
    pub dc_pin: u32,
    /// BCM pin for display reset.
    pub rst_pin: u32,
    /// BCM pin for the backlight. GPIO only, on or off; the panel has no
    /// dimming path.
    pub backlight_pin: u32,

    /// I2C character device the CST328 is on.
    pub i2c_dev: String,
    /// CST328 7-bit address. Customisable in chip firmware, so clones vary.
    pub touch_addr: u16,
    /// BCM pin for the touch interrupt.
    pub touch_int_pin: u32,
    /// BCM pin for the touch reset.
    pub touch_rst_pin: u32,

    /// gpiochip to request lines from.
    pub gpiochip: String,

    /// Display rotation applied at init, degrees clockwise.
    pub rotation: u16,

    /// Volumio state endpoint.
    pub state_url: String,
    /// Volumio command endpoint base. The `cmd` query is appended.
    pub command_url: String,
    /// How often to poll it, milliseconds.
    pub poll_interval_ms: u64,
    /// Minimum gap between two accepted touches, milliseconds. The controller
    /// reports repeatedly while a finger is held; this is what stops one press
    /// becoming a burst of commands.
    pub touch_debounce_ms: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            spi_dev: "/dev/spidev0.0".into(),
            spi_speed_hz: 32_000_000,
            dc_pin: 25,
            rst_pin: 27,
            backlight_pin: 18,

            i2c_dev: "/dev/i2c-1".into(),
            touch_addr: 0x1a,
            touch_int_pin: 4,
            touch_rst_pin: 17,

            gpiochip: "/dev/gpiochip0".into(),

            rotation: 0,

            state_url: "http://localhost:3000/api/v1/getState".into(),
            command_url: "http://localhost:3000/api/v1/commands/".into(),
            poll_interval_ms: 500,
            touch_debounce_ms: 300,
        }
    }
}

impl Config {
    /// Load from a TOML file, falling back to defaults if it is absent.
    ///
    /// A missing file is not an error: the defaults are the reference wiring.
    /// A malformed file is an error, because silently ignoring a typo in a
    /// pin number would be worse than refusing to start.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            tracing::info!(path = %path.display(), "no config file, using defaults");
            return Ok(Self::default());
        }
        anyhow::bail!(
            "{} exists but config file parsing is not implemented yet; \
             remove it to run with the reference defaults",
            path.display()
        )
    }
}
