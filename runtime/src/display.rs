//! Panel bring-up and drawing surface.
//!
//! Owns the SPI device, the D/C, reset and backlight lines, and the ST7789V
//! driver. Nothing else in the crate touches the display hardware.

use anyhow::{anyhow, Context, Result};
use embedded_hal::digital::OutputPin;
use gpio_cdev::{Chip, LineRequestFlags};
use linux_embedded_hal::spidev::{SpiModeFlags, SpidevOptions};
use linux_embedded_hal::{CdevPin, Delay, SpidevDevice};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::{Builder, Display};

use crate::art::Art;
use crate::config::Config;
use crate::net::HostInfo;
use crate::state::PlayerState;
use crate::ui::{self, Layout, TextPane};

/// Consumer label reported in `gpioinfo`, so a stuck line is traceable to
/// this process rather than showing as anonymous.
const CONSUMER: &str = "waveshare28-panel";

/// Scratch buffer the SPI interface batches pixel writes through. 512 bytes
/// is the size used in the mipidsi examples; larger buffers trade RAM for
/// fewer syscalls, which is not a trade worth making on a 512 MB board.
const SPI_BUF_LEN: usize = 512;

type PanelDisplay = Display<SpiInterface<'static, SpidevDevice, CdevPin>, ST7789, CdevPin>;

/// The display, plus the backlight line that is not part of it.
pub struct Panel {
    display: PanelDisplay,
    backlight: CdevPin,
    layout: Layout,
}

impl Panel {
    /// Open the SPI device, claim the GPIO lines, and initialise the panel.
    ///
    /// Fails rather than degrades if `/dev/spidev0.0` is absent: that means a
    /// display overlay is loaded on spi0 cs0, which disables the spidev node,
    /// and the two paths cannot coexist. Saying so plainly is more use than a
    /// generic open error.
    pub fn open(cfg: &Config) -> Result<Self> {
        let mut spi = SpidevDevice::open(&cfg.spi_dev).with_context(|| {
            format!(
                "opening {}. If a display overlay (fbtft, mipi-dbi-spi) is loaded \
                 on spi0 cs0 the spidev node is disabled and this path cannot be used.",
                cfg.spi_dev
            )
        })?;

        spi.configure(
            &SpidevOptions::new()
                .bits_per_word(8)
                .max_speed_hz(cfg.spi_speed_hz)
                .mode(SpiModeFlags::SPI_MODE_0)
                .build(),
        )
        .context("configuring spi")?;

        let mut chip =
            Chip::new(&cfg.gpiochip).with_context(|| format!("opening {}", cfg.gpiochip))?;

        let dc = request_output(&mut chip, cfg.dc_pin, 0, "panel dc")?;
        let rst = request_output(&mut chip, cfg.rst_pin, 1, "panel reset")?;
        let backlight = request_output(&mut chip, cfg.backlight_pin, 0, "panel backlight")?;

        // The interface borrows the buffer for as long as the display lives,
        // which is the life of the process. Leaking one allocation is honest
        // about that; threading a lifetime through Panel would only move the
        // problem to every caller.
        let buffer: &'static mut [u8] = Box::leak(vec![0u8; SPI_BUF_LEN].into_boxed_slice());
        let di = SpiInterface::new(spi, dc, buffer);

        let display = Builder::new(ST7789, di)
            .display_size(ui::NATIVE_W, ui::NATIVE_H)
            .orientation(Orientation::new().rotate(rotation(cfg.rotation)?))
            // ST7789 panels are normally-black and need inversion on; without
            // it the picture is a photographic negative.
            .invert_colors(ColorInversion::Inverted)
            .reset_pin(rst)
            .init(&mut Delay)
            .map_err(|e| anyhow!("panel init failed: {e:?}"))?;

        Ok(Self {
            display,
            backlight,
            layout: Layout::for_rotation(cfg.rotation),
        })
    }

    /// The layout this panel was opened with.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Turn the backlight on or off.
    ///
    /// GPIO only. The fbtft backlight class device for this panel reports a
    /// `max_brightness` of 0, so there is no dimming to expose even when the
    /// kernel path is in use.
    pub fn backlight(&mut self, on: bool) -> Result<()> {
        if on {
            self.backlight.set_high()
        } else {
            self.backlight.set_low()
        }
        .map_err(|e| anyhow!("setting backlight: {e:?}"))
    }

    /// Redraw the whole screen for the given state.
    pub fn render(
        &mut self,
        state: &PlayerState,
        art: Option<&Art>,
        pane: &TextPane,
    ) -> Result<()> {
        ui::draw(&mut self.display, &self.layout, state, art, pane)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }

    /// Repaint the album art only.
    pub fn render_art(&mut self, art: Option<&Art>) -> Result<()> {
        ui::draw_art(&mut self.display, &self.layout, art).map_err(|e| anyhow!("drawing: {e:?}"))
    }

    /// Draw the status screen shown before the player is available.
    pub fn render_status(&mut self, host: &HostInfo) -> Result<()> {
        ui::draw_status(&mut self.display, &self.layout, host)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }

    /// The size of the art box, so the loader knows what to scale to.
    pub fn art_size(&self) -> u32 {
        self.layout.art.size.width.min(self.layout.art.size.height)
    }

    /// Repaint the text pane only.
    pub fn render_rows(&mut self, pane: &TextPane) -> Result<()> {
        ui::draw_rows(&mut self.display, &self.layout, pane).map_err(|e| anyhow!("drawing: {e:?}"))
    }

    /// Repaint the progress bar only.
    pub fn render_progress(&mut self, state: &PlayerState) -> Result<()> {
        ui::draw_progress(&mut self.display, &self.layout, state)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }

    /// Repaint the volume slider only.
    pub fn render_volume(&mut self, state: &PlayerState) -> Result<()> {
        ui::draw_volume(&mut self.display, &self.layout, state)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }
}

/// Claim one line as an output with a defined initial level.
fn request_output(chip: &mut Chip, offset: u32, initial: u8, what: &str) -> Result<CdevPin> {
    let handle = chip
        .get_line(offset)
        .with_context(|| format!("getting line {offset} for {what}"))?
        .request(LineRequestFlags::OUTPUT, initial, CONSUMER)
        .with_context(|| format!("requesting line {offset} for {what}"))?;
    CdevPin::new(handle).with_context(|| format!("wrapping line {offset} for {what}"))
}

/// Map a configured rotation in degrees to the driver's enum.
fn rotation(degrees: u16) -> Result<Rotation> {
    Ok(match degrees {
        0 => Rotation::Deg0,
        90 => Rotation::Deg90,
        180 => Rotation::Deg180,
        270 => Rotation::Deg270,
        other => anyhow::bail!("rotation must be 0, 90, 180 or 270, got {other}"),
    })
}
