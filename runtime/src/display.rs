//! Panel bring-up and drawing surface.
//!
//! Two backends, one owner of the glass:
//!
//! - `spi` opens `/dev/spidev0.0` and the DC, reset and backlight lines.
//! - `framebuffer` mmaps an existing `/dev/fbN` and claims none of those
//!   lines, so it can run while fbtft holds the bus for Plymouth.

use anyhow::{anyhow, Context, Result};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_hal::digital::OutputPin;
use gpio_cdev::{Chip, LineRequestFlags};
use linux_embedded_hal::spidev::{SpiModeFlags, SpidevOptions};
use linux_embedded_hal::{CdevPin, Delay, SpidevDevice};
use mipidsi::interface::SpiInterface;
use mipidsi::models::ST7789;
use mipidsi::options::{ColorInversion, Orientation, Rotation};
use mipidsi::{Builder, Display};

use crate::art::Art;
use crate::config::{Backend, Config};
use crate::fbdev::{resolve_fb_dev, FbDev};
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

enum Surface {
    Spi(SpiPanel),
    Fb(FbDev),
}

/// mipidsi error type is not `anyhow`. Wrap so both backends share one.
struct SpiPanel {
    display: PanelDisplay,
}

impl OriginDimensions for SpiPanel {
    fn size(&self) -> Size {
        self.display.size()
    }
}

impl DrawTarget for SpiPanel {
    type Color = Rgb565;
    type Error = anyhow::Error;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        self.display
            .draw_iter(pixels)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }

    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        self.display
            .fill_contiguous(area, colors)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        self.display
            .fill_solid(area, color)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }

    fn clear(&mut self, color: Self::Color) -> Result<(), Self::Error> {
        self.display
            .clear(color)
            .map_err(|e| anyhow!("drawing: {e:?}"))
    }
}

/// DrawTarget is not dyn-safe (generic fill_contiguous). Expand to both
/// concrete surfaces rather than a trait object.
macro_rules! on_surface {
    ($self:ident, |$s:ident| $body:expr) => {
        match &mut $self.surface {
            Surface::Spi($s) => $body,
            Surface::Fb($s) => $body,
        }
    };
}

/// The display, plus the backlight line when this process owns it.
pub struct Panel {
    surface: Surface,
    backlight: Option<CdevPin>,
    layout: Layout,
}

impl Panel {
    /// Open the configured backend and initialise the drawing surface.
    pub fn open(cfg: &Config) -> Result<Self> {
        let layout = Layout::for_rotation(cfg.rotation);
        match cfg.backend {
            Backend::Spi => open_spi(cfg, layout),
            Backend::Framebuffer => open_fb(cfg, layout),
        }
    }

    /// The layout this panel was opened with.
    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    /// Turn the backlight on or off.
    ///
    /// GPIO only, and only on the SPI backend. fbtft already owns GPIO18; its
    /// backlight class device reports `max_brightness` of 0, so there is
    /// nothing to write. The panel is already lit by the time Plymouth quits.
    pub fn backlight(&mut self, on: bool) -> Result<()> {
        let Some(pin) = self.backlight.as_mut() else {
            return Ok(());
        };
        if on { pin.set_high() } else { pin.set_low() }
            .map_err(|e| anyhow!("setting backlight: {e:?}"))
    }

    /// Redraw the whole screen for the given state.
    pub fn render(
        &mut self,
        state: &PlayerState,
        art: Option<&Art>,
        pane: &TextPane,
    ) -> Result<()> {
        on_surface!(self, |s| ui::draw(s, &self.layout, state, art, pane))
    }

    /// Repaint the album art only.
    pub fn render_art(&mut self, art: Option<&Art>) -> Result<()> {
        on_surface!(self, |s| ui::draw_art(s, &self.layout, art))
    }

    /// Draw the status screen shown before the player is available.
    pub fn render_status(&mut self, host: &HostInfo, footer: Option<&str>) -> Result<()> {
        on_surface!(self, |s| ui::draw_status(s, &self.layout, host, footer))
    }

    /// The size of the art box, so the loader knows what to scale to.
    pub fn art_size(&self) -> u32 {
        self.layout.art.size.width.min(self.layout.art.size.height)
    }

    /// Repaint the text pane only.
    pub fn render_rows(&mut self, pane: &TextPane) -> Result<()> {
        on_surface!(self, |s| ui::draw_rows(s, &self.layout, pane))
    }

    /// Repaint the progress bar only.
    pub fn render_progress(&mut self, state: &PlayerState) -> Result<()> {
        on_surface!(self, |s| ui::draw_progress(s, &self.layout, state))
    }

    /// Repaint the volume slider only.
    pub fn render_volume(&mut self, state: &PlayerState) -> Result<()> {
        on_surface!(self, |s| ui::draw_volume(s, &self.layout, state))
    }
}

fn open_fb(cfg: &Config, layout: Layout) -> Result<Panel> {
    let path = resolve_fb_dev(&cfg.fb_dev)?;
    let fb = FbDev::open(&path, &layout).context("opening framebuffer")?;
    Ok(Panel {
        surface: Surface::Fb(fb),
        backlight: None,
        layout,
    })
}

fn open_spi(cfg: &Config, layout: Layout) -> Result<Panel> {
    let mut spi = SpidevDevice::open(&cfg.spi_dev).with_context(|| {
        format!(
            "opening {}. If a display overlay (fbtft, mipi-dbi-spi) is loaded \
             on spi0 cs0 the spidev node is disabled; use backend = \"framebuffer\".",
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

    let mut chip = Chip::new(&cfg.gpiochip).with_context(|| format!("opening {}", cfg.gpiochip))?;

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

    Ok(Panel {
        surface: Surface::Spi(SpiPanel { display }),
        backlight: Some(backlight),
        layout,
    })
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
