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
use crate::net::{HostInfo, NetState};
use crate::state::PlayerState;
use crate::touch::Touch;

/// Panel width in the controller's native frame.
pub const NATIVE_W: u16 = 240;
/// Panel height in the controller's native frame.
pub const NATIVE_H: u16 = 320;

const TITLE_FONT: &MonoFont = &FONT_9X15_BOLD;
const META_FONT: &MonoFont = &FONT_6X10;

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
    /// The whole rotated frame.
    ///
    /// Explicit rather than derived from the other rectangles. The status
    /// screen needs the full width: a full IPv6 address is 39 characters,
    /// which is 234 pixels at the meta font and does not fit the player's
    /// text column.
    pub frame: Rectangle,
    /// Album art.
    pub art: Rectangle,
    /// Track text: title, artist, album, wrapped and centred.
    pub text: Rectangle,
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
            frame: rect(0, 0, 240, 320),
            art: rect(20, 8, 200, 200),
            text: rect(4, 214, 232, 48),
            volume: rect(10, 268, 220, 6),
            progress: rect(10, 280, 220, 4),
            transport: rect(0, 292, 240, 28),
        }
    }

    /// 320 wide by 240 tall. Art on the left, text column on the right,
    /// transport across the full width at the bottom.
    ///
    /// The transport is not in the column, which is the difference that
    /// matters. At roughly 143 ppi a fingertip contact patch is 40 to 50
    /// pixels, so three buttons in a 104 px column are 34 px wide: below the
    /// point where they can be hit reliably, while occupying vertical space
    /// they do not need. Full width makes them 106 by 40.
    ///
    /// The volume slider gains the same way. At 104 px one percent is one
    /// pixel and the control is only good for coarse jumps; at 300 px it is
    /// three pixels per percent and can actually be set.
    ///
    /// The cost is album art at 168 rather than 200. It is still by far the
    /// largest element, and a slider that cannot be landed on is a worse
    /// daily annoyance than 32 pixels of cover.
    fn landscape(rotation: u16) -> Self {
        Self {
            rotation,
            frame: rect(0, 0, 320, 240),
            art: rect(10, 4, 168, 168),
            text: rect(184, 4, 132, 168),
            volume: rect(10, 178, 300, 6),
            progress: rect(10, 190, 300, 4),
            transport: rect(0, 200, 320, 40),
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

/// Track text, word-wrapped into the column and centred.
///
/// Wrapping rather than a horizontal marquee. Three lines of fourteen
/// characters covers most titles, and static text you can read at a glance is
/// better than text that moves. Scrolling remains only as the fallback for
/// content that still does not fit, and it scrolls the block vertically,
/// which is the direction the overflow is in.
#[derive(Debug, Default)]
pub struct TextPane {
    title: Vec<String>,
    artist: Vec<String>,
    album: Vec<String>,
    /// Source strings, kept to detect a genuine change. Re-wrapping on every
    /// poll would restart the scroll twice a second.
    src: (String, String, String),
    /// Total height of the composed block in pixels.
    height: i32,
    /// Pixels scrolled from the top.
    offset: i32,
    /// Pixels of overflow, zero when the block fits.
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
/// Vertical gap between lines of the same field.
const LINE_GAP: i32 = 2;
/// Vertical gap between fields.
const FIELD_GAP: i32 = 8;

/// Greedy word wrap to a pixel width, for a monospaced font.
///
/// A word longer than the line is hard-split rather than left to overflow:
/// long unbroken strings are common in filenames and stream titles, and
/// silently clipping them loses the part most likely to identify the track.
fn wrap(text: &str, width_px: u32, font: &MonoFont) -> Vec<String> {
    let cols = (width_px / font.character_size.width).max(1) as usize;
    let mut out = Vec::new();
    let mut line = String::new();

    for word in text.split_whitespace() {
        let mut word = word;

        while word.chars().count() > cols {
            if !line.is_empty() {
                out.push(std::mem::take(&mut line));
            }
            let split = word
                .char_indices()
                .nth(cols)
                .map(|(i, _)| i)
                .unwrap_or(word.len());
            out.push(word[..split].to_string());
            word = &word[split..];
        }

        if line.is_empty() {
            line.push_str(word);
        } else if line.chars().count() + 1 + word.chars().count() <= cols {
            line.push(' ');
            line.push_str(word);
        } else {
            out.push(std::mem::take(&mut line));
            line.push_str(word);
        }
    }

    if !line.is_empty() {
        out.push(line);
    }
    out
}

/// Height of a wrapped field in pixels, zero when it has no lines.
fn block_height(lines: &[String], font: &MonoFont) -> i32 {
    if lines.is_empty() {
        return 0;
    }
    let line_h = font.character_size.height as i32;
    lines.len() as i32 * line_h + (lines.len() as i32 - 1) * LINE_GAP
}

impl TextPane {
    /// Re-wrap for new content. Does nothing if the strings are unchanged, so
    /// a scroll in progress is not restarted by an ordinary poll.
    pub fn set(&mut self, title: &str, artist: &str, album: &str, region: Rectangle) {
        let next = (title.to_string(), artist.to_string(), album.to_string());
        if self.src == next {
            return;
        }
        self.src = next;

        let w = region.size.width;
        self.title = wrap(title, w, TITLE_FONT);
        self.artist = wrap(artist, w, META_FONT);
        self.album = wrap(album, w, META_FONT);

        let mut h = 0;
        for (lines, font) in [
            (&self.title, TITLE_FONT),
            (&self.artist, META_FONT),
            (&self.album, META_FONT),
        ] {
            let bh = block_height(lines, font);
            if bh > 0 {
                if h > 0 {
                    h += FIELD_GAP;
                }
                h += bh;
            }
        }

        self.height = h;
        self.over = (h - region.size.height as i32).max(0);
        self.offset = 0;
        self.forward = true;
        self.hold = HOLD_TICKS;
    }

    /// Advance one tick. Returns true if the position changed and the pane
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

/// An in-memory RGB565 buffer, used to compose a region before sending it.
///
/// Drawing text straight onto a `clipped()` view of the panel is what caused
/// the text to flicker. A clipped target must bounds-check every pixel, so
/// `fill_contiguous` degrades to `draw_iter`, and mipidsi's `draw_iter` sets an
/// address window per pixel. A 132x168 region is over twenty thousand SPI
/// transactions. Composing here and blitting once is a single window write,
/// and it is atomic on the glass rather than painting progressively.
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

/// Draw the text pane: title, artist and album, wrapped, centred both ways.
///
/// Vertically centred when the block fits, top-aligned and scrolled when it
/// does not, because centring something that is moving reads as a fault
/// rather than as a deliberate scroll.
///
/// Composed in memory and blitted in one write. See [`RowBuf`] for why.
fn draw_text<D>(target: &mut D, region: Rectangle, pane: &TextPane) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut buf = RowBuf::new(region.size);

    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Top)
        .alignment(Alignment::Center)
        .build();
    let cx = region.size.width as i32 / 2;

    // Centre the block when it fits; otherwise start at the top and let the
    // scroll offset move it.
    let mut y = if pane.over == 0 {
        (region.size.height as i32 - pane.height) / 2
    } else {
        -pane.offset
    };

    let fields: [(&Vec<String>, &MonoFont, Rgb565); 3] = [
        (&pane.title, TITLE_FONT, Rgb565::WHITE),
        (&pane.artist, META_FONT, Rgb565::CSS_LIGHT_GRAY),
        (&pane.album, META_FONT, Rgb565::CSS_DIM_GRAY),
    ];

    let mut first = true;
    for (lines, font, colour) in fields {
        if lines.is_empty() {
            continue;
        }
        if !first {
            y += FIELD_GAP;
        }
        first = false;

        let style = MonoTextStyle::new(font, colour);
        for line in lines {
            // Infallible: RowBuf discards out-of-bounds pixels.
            let _ = Text::with_text_style(line, Point::new(cx, y), style, centred).draw(&mut buf);
            y += font.character_size.height as i32 + LINE_GAP;
        }
        y -= LINE_GAP;
    }

    target.fill_contiguous(&region, buf.px.iter().copied())
}

/// Draw the status screen shown before the player answers.
///
/// This is what is on the panel from a few seconds after power on until
/// Volumio's node process responds, which is most of a minute. It exists
/// because the alternative is a dark screen, and because the address is the
/// one thing someone needs before the player is reachable.
///
/// Uses the whole frame rather than the player's text column: a full IPv6
/// address is 39 characters, 234 pixels at the meta font, and the column is
/// only 132 wide.
///
/// Composed in memory and blitted once, same as the text pane, for the same
/// reason: drawing onto a clipped view of the panel forces per-pixel
/// addressing and flickers.
pub fn draw_status<D>(target: &mut D, layout: &Layout, host: &HostInfo) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let region = layout.frame;
    let mut buf = RowBuf::new(region.size);

    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Top)
        .alignment(Alignment::Center)
        .build();
    let cx = region.size.width as i32 / 2;

    let title = MonoTextStyle::new(TITLE_FONT, Rgb565::WHITE);
    let meta = MonoTextStyle::new(META_FONT, Rgb565::CSS_LIGHT_GRAY);
    let dim = MonoTextStyle::new(META_FONT, Rgb565::CSS_DIM_GRAY);

    // Build the lines first so the block height is known and it can be
    // centred, rather than guessing a starting offset per state.
    let mut lines: Vec<(String, MonoTextStyle<Rgb565>)> = Vec::new();

    match &host.state {
        NetState::Waiting => {
            lines.push(("waiting for network".into(), dim));
        }
        NetState::Hotspot { ssid, addr } => {
            // Here the address is not somewhere to connect to over an existing
            // network, it is the second half of an instruction: join this
            // network, then open this address.
            lines.push(("Wi-Fi setup".into(), dim));
            lines.push((ssid.clone(), meta));
            lines.push((addr.clone(), meta));
        }
        NetState::Connected(addrs) => {
            for a in addrs {
                lines.push((format!("{}  {}", a.iface, a.addr), meta));
            }
        }
    }

    let title_h = TITLE_FONT.character_size.height as i32;
    let meta_h = META_FONT.character_size.height as i32;
    let block = title_h + FIELD_GAP + lines.len() as i32 * (meta_h + LINE_GAP) - LINE_GAP;

    let mut y = ((region.size.height as i32 - block) / 2).max(0);

    // Infallible: RowBuf discards out-of-bounds pixels.
    let _ = Text::with_text_style(&host.hostname, Point::new(cx, y), title, centred).draw(&mut buf);
    y += title_h + FIELD_GAP;

    for (text, style) in &lines {
        let _ = Text::with_text_style(text, Point::new(cx, y), *style, centred).draw(&mut buf);
        y += meta_h + LINE_GAP;
    }

    target.fill_contiguous(&region, buf.px.iter().copied())
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
    pane: &TextPane,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(Rgb565::BLACK)?;

    draw_art(target, layout, art)?;
    draw_text(target, layout.text, pane)?;

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

/// Repaint the text pane only.
pub fn draw_rows<D>(target: &mut D, layout: &Layout, pane: &TextPane) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_text(target, layout.text, pane)
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
