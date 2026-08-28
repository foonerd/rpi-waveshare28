//! Runtime configuration.
//!
//! Defaults match the Waveshare SKU 27579 wiring in BCM numbering. Everything
//! here is overridable so a differently wired board or a clone module with a
//! different touch address does not need a rebuild.
//!
//! The file is optional. A missing file means the reference wiring, which is
//! what the overwhelming majority of installations want, so requiring one
//! would be ceremony. A file that exists but does not parse is an error: a
//! typo in a GPIO number that silently reverts to a default produces a panel
//! that does not work for reasons nobody can see.

use serde::Deserialize;
use std::path::Path;

/// How the renderer talks to the panel.
///
/// `spi` owns `/dev/spidev0.0` and the DC/reset/backlight GPIOs. `framebuffer`
/// draws into an existing `/dev/fbN` and claims none of those lines, so it can
/// run while fbtft holds the bus for Plymouth.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Backend {
    Spi,
    Framebuffer,
}

impl Default for Backend {
    fn default() -> Self {
        Self::Spi
    }
}

/// Clockwise degrees to the counter-clockwise value fbtft's `rotate` uses
/// for the same physical orientation.
pub fn fbtft_rotate(clockwise: u16) -> u16 {
    match clockwise {
        90 => 270,
        270 => 90,
        other => other,
    }
}

/// Panel and host wiring, plus behaviour knobs.
///
/// `deny_unknown_fields` is deliberate. A misspelled key that is quietly
/// ignored is the worst outcome: the setting appears to be applied, nothing
/// changes, and the file looks correct. Refusing to start names the typo.
#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    /// Drawing path. See [`Backend`].
    pub backend: Backend,
    /// Framebuffer device used when `backend` is `framebuffer`.
    pub fb_dev: String,
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

    /// Display rotation applied at init, degrees clockwise. 0, 90, 180 or 270.
    ///
    /// This is the only place rotation is set. The panel is rotated by the
    /// display driver and touch coordinates are mapped back through the same
    /// value, so the two cannot disagree.
    pub rotation: u16,

    /// Volumio state endpoint.
    pub state_url: String,
    /// Volumio system status, plain text `starting` or `ready`.
    pub status_url: String,
    /// Volumio command endpoint base. The `cmd` query is appended.
    pub command_url: String,
    /// Origin the relative `albumart` path from the state API is joined to.
    pub art_base: String,
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
            backend: Backend::Spi,
            fb_dev: "/dev/fb1".into(),
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
            status_url: "http://localhost:3000/status".into(),
            command_url: "http://localhost:3000/api/v1/commands/".into(),
            art_base: "http://localhost:3000".into(),
            poll_interval_ms: 500,
            touch_debounce_ms: 300,
        }
    }
}

impl Config {
    /// Load from a TOML file, falling back to defaults if it is absent.
    pub fn load(path: &Path) -> anyhow::Result<Self> {
        if !path.exists() {
            tracing::info!(path = %path.display(), "no config file, using defaults");
            return Ok(Self::default());
        }

        let text = std::fs::read_to_string(path)
            .map_err(|e| anyhow::anyhow!("reading {}: {e}", path.display()))?;

        let cfg: Self = toml::from_str(&text)
            .map_err(|e| anyhow::anyhow!("parsing {}:\n{e}", path.display()))?;

        cfg.validate()?;
        Ok(cfg)
    }

    /// Reject values that would fail later in a less obvious place.
    ///
    /// Checked here rather than at the point of use so a bad file is reported
    /// at startup, next to the filename, instead of as a panel init failure
    /// several layers down.
    fn validate(&self) -> anyhow::Result<()> {
        if !matches!(self.rotation, 0 | 90 | 180 | 270) {
            anyhow::bail!("rotation must be 0, 90, 180 or 270, got {}", self.rotation);
        }
        if self.touch_addr > 0x7f {
            anyhow::bail!(
                "touch_addr must be a 7-bit address, got {:#x}",
                self.touch_addr
            );
        }
        if self.poll_interval_ms == 0 {
            anyhow::bail!("poll_interval_ms must be greater than zero");
        }
        if self.spi_speed_hz == 0 {
            anyhow::bail!("spi_speed_hz must be greater than zero");
        }
        if self.fb_dev.is_empty() {
            anyhow::bail!("fb_dev must not be empty");
        }
        if self.spi_dev.is_empty() {
            anyhow::bail!("spi_dev must not be empty");
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write(text: &str) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        f.write_all(text.as_bytes()).unwrap();
        f
    }

    #[test]
    fn missing_file_is_defaults() {
        let cfg = Config::load(Path::new("/nonexistent/waveshare28-panel.toml")).unwrap();
        assert_eq!(cfg.rotation, 0);
        assert_eq!(cfg.backend, Backend::Spi);
        assert_eq!(cfg.status_url, "http://localhost:3000/status");
        assert_eq!(cfg.spi_dev, "/dev/spidev0.0");
        assert_eq!(cfg.fb_dev, "/dev/fb1");
    }

    #[test]
    fn partial_file_keeps_defaults_for_the_rest() {
        let f = write("rotation = 90\n");
        let cfg = Config::load(f.path()).unwrap();
        assert_eq!(cfg.rotation, 90);
        assert_eq!(cfg.dc_pin, 25);
        assert_eq!(cfg.touch_addr, 0x1a);
    }

    #[test]
    fn unknown_key_is_an_error() {
        let f = write("rotaton = 90\n");
        let err = Config::load(f.path()).unwrap_err().to_string();
        assert!(err.contains("rotaton"), "{err}");
    }

    #[test]
    fn bad_rotation_is_an_error() {
        let f = write("rotation = 45\n");
        let err = Config::load(f.path()).unwrap_err().to_string();
        assert!(err.contains("rotation"), "{err}");
    }

    #[test]
    fn out_of_range_touch_address_is_an_error() {
        let f = write("touch_addr = 0x1ff\n");
        assert!(Config::load(f.path()).is_err());
    }

    #[test]
    fn zero_poll_interval_is_an_error() {
        let f = write("poll_interval_ms = 0\n");
        assert!(Config::load(f.path()).is_err());
    }

    #[test]
    fn framebuffer_backend_parses() {
        let f = write("backend = \"framebuffer\"\nfb_dev = \"/dev/fb1\"\nrotation = 270\n");
        let cfg = Config::load(f.path()).unwrap();
        assert_eq!(cfg.backend, Backend::Framebuffer);
        assert_eq!(cfg.rotation, 270);
        assert_eq!(fbtft_rotate(cfg.rotation), 90);
    }

    #[test]
    fn fbtft_rotate_is_the_counter_clockwise_counterpart() {
        assert_eq!(fbtft_rotate(0), 0);
        assert_eq!(fbtft_rotate(90), 270);
        assert_eq!(fbtft_rotate(180), 180);
        assert_eq!(fbtft_rotate(270), 90);
    }
}
