//! Screen layout, drawing, and hit regions.
//!
//! Two layouts, selected from the configured rotation: portrait 240x320 and
//! landscape 320x240. Everything is expressed as rectangles in a [`Layout`] so
//! drawing and hit testing cannot disagree about where anything is, which is
//! the usual way a touch UI ends up with controls that do not do what they
//! look like they do.
//!
//! Coordinates are in the rotated frame, the same one the display driver
//! presents. Touch coordinates arrive from the controller unrotated, so
//! [`hit`] applies the inverse transform before testing.

use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, ascii::FONT_9X15_BOLD, MonoFont, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};

use crate::art::Art;
use crate::state::PlayerState;
use crate::touch::Touch;

/// Panel width in the controller's native frame.
pub const NATIVE_W: u16 = 240;
/// Panel height in the controller's native frame.
pub const NATIVE_H: u16 = 320;

const TITLE_FONT: &MonoFont = &FONT_9X15_BOLD;
const META_FONT: &MonoFont = &FONT_6X10;

/// Font used for the title row, exposed so the ticker can measure text.
pub fn title_font() -> &'static MonoFont<'static> {
    TITLE_FONT
}

/// Font used for the artist row, exposed so the ticker can measure text.
pub fn meta_font() -> &'static MonoFont<'static> {
    META_FONT
}

/// Where everything sits, for one orientation.
///
/// No overall width or height: every element is an explicit rectangle, the
/// driver is initialised from the controller's native size, and `clear` covers
/// the whole frame. A frame size here would only be a second place for the
/// truth to live.
#[derive(Debug, Clone, Copy)]
pub struct Layout {
    /// Rotation in degrees, needed to map touch coordinates back.
    pub rotation: u16,
    /// Album art.
    pub art: Rectangle,
    /// Track title, scrolling if too long.
    pub title: Rectangle,
    /// Artist, scrolling if too long.
    pub artist: Rectangle,
    /// Volume slider.
    pub volume: Rectangle,
    /// Playback progress bar.
    pub progress: Rectangle,
    /// Transport strip, split into equal thirds.
    pub transport: Rectangle,
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

impl Layout {
    /// Layout for a rotation in degrees. 0 and 180 are portrait, 90 and 270
    /// landscape.
    pub fn for_rotation(rotation: u16) -> Self {
        match rotation {
            90 | 270 => Self::landscape(rotation),
            _ => Self::portrait(rotation),
        }
    }

    /// 240 wide by 320 tall. Art on top, everything else stacked beneath.
    fn portrait(rotation: u16) -> Self {
        Self {
            rotation,
            art: rect(20, 8, 200, 200),
            title: rect(0, 214, 240, 16),
            artist: rect(0, 234, 240, 11),
            volume: rect(10, 252, 220, 6),
            progress: rect(0, 266, 240, 4),
            transport: rect(0, 278, 240, 42),
        }
    }

    /// 320 wide by 240 tall. Art on the left, a control column on the right.
    ///
    /// The column is 104 px wide, which is enough for 11 characters of the
    /// title font. Almost every title scrolls at this width, which is why the
    /// ticker is not optional.
    fn landscape(rotation: u16) -> Self {
        Self {
            rotation,
            art: rect(12, 20, 200, 200),
            title: rect(216, 26, 104, 16),
            artist: rect(216, 46, 104, 11),
            volume: rect(216, 72, 104, 6),
            progress: rect(216, 86, 104, 4),
            transport: rect(216, 110, 104, 110),
        }
    }

    /// Map a raw controller touch into this layout's frame.
    ///
    /// The controller always reports in its native 240x320 portrait frame
    /// regardless of what the display driver was told, so rotating the panel
    /// does not rotate the input. This is where the two are reconciled.
    fn map(&self, t: Touch) -> Point {
        let (x, y) = (i32::from(t.x), i32::from(t.y));
        let (nw, nh) = (i32::from(NATIVE_W), i32::from(NATIVE_H));

        match self.rotation {
            90 => Point::new(y, nw - 1 - x),
            180 => Point::new(nw - 1 - x, nh - 1 - y),
            270 => Point::new(nh - 1 - y, x),
            _ => Point::new(x, y),
        }
    }
}

/// A user action derived from a touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Previous track.
    Prev,
    /// Toggle play and pause.
    PlayPause,
    /// Next track.
    Next,
    /// Set volume to a percentage.
    Volume(u8),
    /// Tap on the art area, currently unassigned.
    Art,
}

/// Map a touch to an action, or `None` if it landed on nothing.
///
/// The transport strip is split into equal thirds with no dead band between
/// them: the targets are already far larger than a fingertip, and a gap only
/// creates places where a deliberate press does nothing.
pub fn hit(layout: &Layout, t: Touch) -> Option<Action> {
    let p = layout.map(t);

    // Generous vertical slop on the slider. It is only a few pixels tall, and
    // demanding that precision from a finger would make it unusable.
    let slider = layout.volume;
    let slider_zone = Rectangle::new(
        slider.top_left - Point::new(0, 10),
        Size::new(slider.size.width, slider.size.height + 20),
    );
    if slider_zone.contains(p) {
        let dx = (p.x - slider.top_left.x).max(0) as u32;
        let pct = (dx * 100 / slider.size.width.max(1)).min(100);
        return Some(Action::Volume(pct as u8));
    }

    if layout.transport.contains(p) {
        let third = layout.transport.size.width as i32 / 3;
        let dx = p.x - layout.transport.top_left.x;
        return Some(match dx {
            d if d < third => Action::Prev,
            d if d < 2 * third => Action::PlayPause,
            _ => Action::Next,
        });
    }

    if layout.art.contains(p) {
        return Some(Action::Art);
    }

    None
}

/// A back-and-forth scroller for text wider than its box.
///
/// Ping-pong rather than wrap-around: a wrapping marquee reads as a stream of
/// characters with no beginning, whereas bouncing keeps the start of the title
/// identifiable, which is what someone glancing at a player actually wants.
#[derive(Debug, Default)]
pub struct Ticker {
    text: String,
    /// Pixels scrolled from the left.
    offset: i32,
    /// Pixels of overflow, zero when the text fits.
    over: i32,
    /// Direction of travel.
    forward: bool,
    /// Ticks left to hold at an end before reversing.
    hold: u8,
}

/// Ticks held at each end before reversing.
const HOLD_TICKS: u8 = 20;
/// Pixels moved per tick.
const STEP: i32 = 1;

impl Ticker {
    /// Point at new text. Resets position only if the text actually changed,
    /// so a redraw does not restart a scroll mid-title.
    pub fn set(&mut self, text: &str, box_w: u32, font: &MonoFont) {
        if self.text == text {
            return;
        }
        let px = font.character_size.width as i32 * text.chars().count() as i32;
        self.text = text.to_string();
        self.over = (px - box_w as i32).max(0);
        self.offset = 0;
        self.forward = true;
        self.hold = HOLD_TICKS;
    }

    /// Advance one tick. Returns true if the position changed and the row
    /// needs repainting.
    pub fn step(&mut self) -> bool {
        if self.over == 0 {
            return false;
        }
        if self.hold > 0 {
            self.hold -= 1;
            return false;
        }
        if self.forward {
            self.offset += STEP;
            if self.offset >= self.over {
                self.offset = self.over;
                self.forward = false;
                self.hold = HOLD_TICKS;
            }
        } else {
            self.offset -= STEP;
            if self.offset <= 0 {
                self.offset = 0;
                self.forward = true;
                self.hold = HOLD_TICKS;
            }
        }
        true
    }
}

/// An in-memory RGB565 buffer, used to compose a row before sending it.
///
/// Drawing text straight onto a `clipped()` view of the panel is what caused
/// the ticker to flicker. A clipped target must bounds-check every pixel, so
/// `fill_contiguous` degrades to `draw_iter`, and mipidsi's `draw_iter` sets an
/// address window per pixel. A 104x11 row is over a thousand SPI transactions,
/// repeated every tick. Composing here and blitting once is a single window
/// write, and it is atomic on the glass rather than painting progressively.
struct RowBuf {
    size: Size,
    px: Vec<Rgb565>,
}

impl RowBuf {
    fn new(size: Size) -> Self {
        Self {
            px: vec![Rgb565::BLACK; (size.width * size.height) as usize],
            size,
        }
    }
}

impl OriginDimensions for RowBuf {
    fn size(&self) -> Size {
        self.size
    }
}

impl DrawTarget for RowBuf {
    type Color = Rgb565;
    type Error = core::convert::Infallible;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let (w, h) = (self.size.width as i32, self.size.height as i32);
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < w && p.y < h {
                self.px[(p.y * w + p.x) as usize] = c;
            }
        }
        Ok(())
    }
}

/// Draw one text row, clipped to its box, scrolled by the ticker.
///
/// Text that fits is centred; text that overflows is left aligned and
/// scrolled, because centring a scrolling string makes the motion look like a
/// glitch rather than a deliberate scroll.
///
/// Composed in memory and blitted in one write. See [`RowBuf`] for why.
fn draw_row<D>(
    target: &mut D,
    boxr: Rectangle,
    ticker: &Ticker,
    font: &MonoFont,
    colour: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut buf = RowBuf::new(boxr.size);
    let style = MonoTextStyle::new(font, colour);

    // Coordinates are buffer-relative, so the box origin drops out.
    let (origin, text_style) = if ticker.over == 0 {
        (
            Point::new(boxr.size.width as i32 / 2, 0),
            TextStyleBuilder::new()
                .baseline(Baseline::Top)
                .alignment(Alignment::Center)
                .build(),
        )
    } else {
        (
            Point::new(-ticker.offset, 0),
            TextStyleBuilder::new().baseline(Baseline::Top).build(),
        )
    };

    // Infallible: RowBuf discards out-of-bounds pixels rather than erroring.
    let _ = Text::with_text_style(&ticker.text, origin, style, text_style).draw(&mut buf);

    target.fill_contiguous(&boxr, buf.px.iter().copied())
}

/// Draw the whole screen.
///
/// Clears and repaints, so this is only for a scene change: a different track,
/// or a transport state change. A full frame is about 39 ms at 32 MHz, and
/// doing that twice a second because `seek` advanced is visible as a flicker.
/// Progress, volume and the scrolling rows are repainted individually.
pub fn draw<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
    art: Option<&Art>,
    title: &Ticker,
    artist: &Ticker,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(Rgb565::BLACK)?;

    draw_art(target, layout, art)?;

    draw_row(target, layout.title, title, TITLE_FONT, Rgb565::WHITE)?;
    draw_row(
        target,
        layout.artist,
        artist,
        META_FONT,
        Rgb565::CSS_LIGHT_GRAY,
    )?;

    draw_volume(target, layout, state)?;
    draw_progress(target, layout, state)?;
    draw_transport(target, layout, state)?;

    Ok(())
}

/// Draw the cover, centred in the art box.
///
/// The picture is scaled to fit rather than to fill, so a non-square cover
/// leaves margins. Those are painted black rather than left holding whatever
/// the previous track's art put there.
pub fn draw_art<D>(target: &mut D, layout: &Layout, art: Option<&Art>) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let box_ = layout.art;

    let Some(art) = art else {
        return box_
            .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(target);
    };

    let w = art.w.min(box_.size.width);
    let h = art.h.min(box_.size.height);
    let x = box_.top_left.x + (box_.size.width as i32 - w as i32) / 2;
    let y = box_.top_left.y + (box_.size.height as i32 - h as i32) / 2;
    let placed = Rectangle::new(Point::new(x, y), Size::new(w, h));

    if placed != box_ {
        box_.into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(target)?;
    }

    target.fill_contiguous(&placed, art.px.iter().copied())
}

/// Repaint the scrolling rows only.
pub fn draw_rows<D>(
    target: &mut D,
    layout: &Layout,
    title: &Ticker,
    artist: &Ticker,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_row(target, layout.title, title, TITLE_FONT, Rgb565::WHITE)?;
    draw_row(
        target,
        layout.artist,
        artist,
        META_FONT,
        Rgb565::CSS_LIGHT_GRAY,
    )
}

/// Repaint the progress bar only.
///
/// The seek/duration unit mismatch is handled in `PlayerState::progress`.
pub fn draw_progress<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let bar = layout.progress;

    // Nothing to show for a stream with no duration, which is the normal case
    // for internet radio. Leave the row blank rather than drawing an empty
    // trough that looks like a stalled track.
    let Some(frac) = state.progress() else {
        bar.into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
            .draw(target)?;
        return Ok(());
    };

    fill_bar(target, bar, frac, Rgb565::CSS_DIM_GRAY, Rgb565::WHITE)
}

/// Repaint the volume slider only.
pub fn draw_volume<D>(target: &mut D, layout: &Layout, state: &PlayerState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let frac = if state.is_muted() {
        0.0
    } else {
        f32::from(state.volume.unwrap_or(0)) / 100.0
    };

    fill_bar(
        target,
        layout.volume,
        frac,
        Rgb565::CSS_DIM_GRAY,
        Rgb565::CSS_ORANGE,
    )
}

/// Repaint the transport labels only.
pub fn draw_transport<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let strip = layout.transport;
    strip
        .into_styled(PrimitiveStyle::with_fill(Rgb565::BLACK))
        .draw(target)?;

    let style = MonoTextStyle::new(TITLE_FONT, Rgb565::WHITE);
    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Middle)
        .alignment(Alignment::Center)
        .build();

    let third = strip.size.width as i32 / 3;
    let y = strip.top_left.y + strip.size.height as i32 / 2;
    let play = if state.is_playing() { "||" } else { ">" };

    for (i, label) in ["|<", play, ">|"].iter().enumerate() {
        Text::with_text_style(
            label,
            Point::new(strip.top_left.x + third * i as i32 + third / 2, y),
            style,
            centred,
        )
        .draw(target)?;
    }

    Ok(())
}

/// Draw a horizontal fill bar: trough, then the filled portion.
fn fill_bar<D>(
    target: &mut D,
    bar: Rectangle,
    frac: f32,
    trough: Rgb565,
    fill: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    bar.into_styled(PrimitiveStyle::with_fill(trough))
        .draw(target)?;

    let filled = (bar.size.width as f32 * frac.clamp(0.0, 1.0)) as u32;
    if filled > 0 {
        Rectangle::new(bar.top_left, Size::new(filled, bar.size.height))
            .into_styled(PrimitiveStyle::with_fill(fill))
            .draw(target)?;
    }

    Ok(())
}
