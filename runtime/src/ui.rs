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
//! [`Layout::map`] is applied before hit testing.
//!
//! Sitting S (ADR-0020): resting face is display-only plus large hotspots.
//! Type map, stock faces only: ~13/~11 bold → [`FONT_9X15_BOLD`]; ~9/~8/~7
//! → [`FONT_6X10`]. Status IP uses [`FONT_10X20`] when status text is large.

use embedded_graphics::{
    mono_font::{
        ascii::FONT_10X20, ascii::FONT_6X10, ascii::FONT_9X15_BOLD, MonoFont, MonoTextStyle,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{Circle, Line, PrimitiveStyle, Rectangle, Triangle},
    text::{Alignment, Baseline, Text, TextStyleBuilder},
};

use crate::art::Art;
use crate::config::{BarGap, Config, StatusText, Strip, Theme};
use crate::net::HostInfo;
use crate::state::PlayerState;
use crate::touch::Touch;

/// Panel width in the controller's native frame.
pub const NATIVE_W: u16 = 240;
/// Panel height in the controller's native frame.
pub const NATIVE_H: u16 = 320;

pub(crate) const TITLE_FONT: &MonoFont = &FONT_9X15_BOLD;
pub(crate) const META_FONT: &MonoFont = &FONT_6X10;

/// Cabinet + cone. 12×10. The mark that was signed off on the volume strip.
const SPEAKER_W: i32 = 12;
const SPEAKER_H: i32 = 10;
const SPEAKER_BODY: &[(u8, u8)] = &[
    (0, 3),
    (1, 3),
    (0, 4),
    (1, 4),
    (0, 5),
    (1, 5),
    (0, 6),
    (1, 6),
    (2, 2),
    (2, 3),
    (2, 4),
    (2, 5),
    (2, 6),
    (2, 7),
    (3, 1),
    (3, 2),
    (3, 3),
    (3, 4),
    (3, 5),
    (3, 6),
    (3, 7),
    (3, 8),
    (4, 0),
    (4, 1),
    (4, 2),
    (4, 3),
    (4, 4),
    (4, 5),
    (4, 6),
    (4, 7),
    (4, 8),
    (4, 9),
];
const SPEAKER_WAVES: &[(u8, u8)] = &[
    (6, 2),
    (7, 3),
    (7, 6),
    (6, 7),
    (8, 1),
    (9, 2),
    (10, 3),
    (10, 6),
    (9, 7),
    (8, 8),
];
const SPEAKER_MUTE_X: &[(u8, u8)] = &[
    (6, 1),
    (7, 2),
    (8, 3),
    (9, 4),
    (10, 5),
    (11, 6),
    (6, 2),
    (7, 3),
    (8, 4),
    (9, 5),
    (10, 6),
    (11, 7),
    (11, 1),
    (10, 2),
    (9, 3),
    (8, 4),
    (7, 5),
    (6, 6),
    (11, 2),
    (10, 3),
    (9, 4),
    (8, 5),
    (7, 6),
    (6, 7),
];

/// Map a point on a seek strip to 0..1, padded 12 px each end.
pub fn seek_fraction(slot: Rectangle, p: Point) -> f32 {
    let pad = 12i32;
    let x0 = slot.top_left.x + pad;
    let w = slot.size.width as i32 - pad * 2;
    if w <= 0 {
        return 0.0;
    }
    ((p.x - x0) as f32 / w as f32).clamp(0.0, 1.0)
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
    /// The whole rotated frame.
    ///
    /// Explicit rather than derived from the other rectangles. The status
    /// screen needs the full width: a full IPv6 address is 39 characters,
    /// which is 234 pixels at the meta font and does not fit the player's
    /// text column.
    pub frame: Rectangle,
    /// Album art.
    pub art: Rectangle,
    /// Title / artist / album block. Tap opens metadata.
    pub text: Rectangle,
    /// `i` ring hit target.
    pub info: Rectangle,
    /// Three-slot glyph dock.
    pub dock: Rectangle,
    /// Seek / stream strip. Empty height when `strip` is Off.
    pub progress: Rectangle,
    /// Status-screen type for this orientation.
    pub status_text: StatusText,
    /// What occupies the volume-to-transport slot.
    pub strip: Strip,
    /// Colour tokens for this paint.
    pub theme: Theme,
}

/// Named colours for one theme. Ink matches the shipped CSS constants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Palette {
    pub bg: Rgb565,
    pub title: Rgb565,
    pub meta: Rgb565,
    pub dim: Rgb565,
    pub accent: Rgb565,
    pub danger: Rgb565,
}

/// ADR-0020 table word. The hex is the contract; do not re-derive with `>>`.
const fn word(w: u16) -> Rgb565 {
    Rgb565::new(
        ((w >> 11) & 0x1f) as u8,
        ((w >> 5) & 0x3f) as u8,
        (w & 0x1f) as u8,
    )
}

pub fn palette(theme: Theme) -> Palette {
    match theme {
        Theme::Ink => Palette {
            bg: Rgb565::BLACK,
            title: Rgb565::WHITE,
            meta: Rgb565::CSS_LIGHT_GRAY,
            dim: Rgb565::CSS_DIM_GRAY,
            accent: Rgb565::CSS_ORANGE,
            danger: Rgb565::CSS_RED,
        },
        Theme::Dusk => Palette {
            bg: word(0x30C1),
            title: word(0xFEB5),
            meta: word(0xD52E),
            dim: word(0x9B07),
            accent: word(0xFD85),
            danger: word(0xFA87),
        },
        Theme::Studio => Palette {
            bg: word(0x08C7),
            title: word(0xDF5F),
            meta: word(0x959A),
            dim: word(0x5352),
            accent: word(0x3DBF),
            danger: word(0xFA4A),
        },
        Theme::Night => Palette {
            bg: word(0x1040),
            title: word(0xFD89),
            meta: word(0xCC67),
            dim: word(0x7264),
            accent: word(0xFF57),
            danger: word(0xF944),
        },
    }
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

impl Layout {
    pub(crate) fn pal(&self) -> Palette {
        palette(self.theme)
    }

    /// Layout for a rotation in degrees. 0 and 180 are portrait, 90 and 270
    /// landscape. Default gap and status type; the missing-file path goes
    /// through [`Config::default`] and [`Self::from_config`].
    #[cfg(test)]
    pub fn for_rotation(rotation: u16) -> Self {
        Self::compose(
            rotation,
            BarGap::Default,
            StatusText::Normal,
            Strip::Progress,
            Theme::Ink,
        )
    }

    /// Layout for the configured rotation, gap, status type and strip.
    pub fn from_config(cfg: &Config) -> Self {
        Self::compose(
            cfg.rotation,
            cfg.bar_gap(),
            cfg.status_text(),
            cfg.strip(),
            cfg.theme,
        )
    }

    fn compose(
        rotation: u16,
        _gap: BarGap,
        status_text: StatusText,
        strip: Strip,
        theme: Theme,
    ) -> Self {
        // Sitting S redlines supersede bar-gap geometry. The key stays.
        match rotation {
            90 | 270 => Self::landscape(rotation, status_text, strip, theme),
            _ => Self::portrait(rotation, status_text, strip, theme),
        }
    }

    /// ADR-0020 A.1. 240×320. Art 152, dock 52, seek 32. Off grows art
    /// and drops the dock to the bottom edge.
    fn portrait(rotation: u16, status_text: StatusText, strip: Strip, theme: Theme) -> Self {
        let seek_on = strip != Strip::Off;
        let (art_h, text_y, dock_y, seek_y, seek_h) = if seek_on {
            (152, 162, 236, 288, 32)
        } else {
            (184, 194, 268, 320, 0)
        };
        Self {
            rotation,
            frame: rect(0, 0, 240, 320),
            art: rect(44, 0, 152, art_h),
            text: rect(12, text_y, 216, 74),
            info: rect(192, 0, 48, 48),
            dock: rect(0, dock_y, 240, 52),
            progress: rect(0, seek_y, 240, seek_h),
            status_text,
            strip,
            theme,
        }
    }

    /// ADR-0020 A.2. 320×240. Art 200, text column 120, dock 40×44,
    /// seek 40 full width. Off grows art into the seek band.
    fn landscape(rotation: u16, status_text: StatusText, strip: Strip, theme: Theme) -> Self {
        let seek_on = strip != Strip::Off;
        let (art_h, col_h, dock_y, seek_y, seek_h): (u32, u32, i32, i32, u32) = if seek_on {
            (200, 200, 156, 200, 40)
        } else {
            (240, 240, 196, 240, 0)
        };
        Self {
            rotation,
            frame: rect(0, 0, 320, 240),
            art: rect(0, 0, 200, art_h),
            text: rect(200, 0, 120, col_h.saturating_sub(44)),
            info: rect(276, 0, 44, 44),
            dock: rect(200, dock_y, 120, 44),
            progress: rect(0, seek_y, 320, seek_h),
            status_text,
            strip,
            theme,
        }
    }

    /// One of the three dock cells, 0 = controls, 1 = volume, 2 = metadata.
    pub fn dock_cell(&self, i: u8) -> Rectangle {
        let n = 3u32;
        let w = self.dock.size.width / n;
        rect(
            self.dock.top_left.x + w as i32 * i32::from(i),
            self.dock.top_left.y,
            w,
            self.dock.size.height,
        )
    }

    /// Map a raw controller touch into this layout's frame.
    ///
    /// The controller always reports in its native 240x320 portrait frame
    /// regardless of what the display driver was told, so rotating the panel
    /// does not rotate the input. This is where the two are reconciled.
    pub fn map(&self, t: Touch) -> Point {
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

/// Press / move / release after rotation mapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TouchEv {
    Down(Point),
    Move(Point),
    Up(Point),
}

/// Face hotspot. Surfaces have their own hit map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Hotspot {
    Art,
    Info,
    Title,
    DockControls,
    DockVolume,
    DockMeta,
    Seek,
}

/// Map a point on the resting face to a hotspot.
pub fn face_hit(layout: &Layout, p: Point) -> Option<Hotspot> {
    if layout.info.contains(p) {
        return Some(Hotspot::Info);
    }
    if layout.art.contains(p) {
        return Some(Hotspot::Art);
    }
    if layout.text.contains(p) {
        return Some(Hotspot::Title);
    }
    if layout.dock.contains(p) {
        let dx = p.x - layout.dock.top_left.x;
        let third = layout.dock.size.width as i32 / 3;
        return Some(if dx < third {
            Hotspot::DockControls
        } else if dx < 2 * third {
            Hotspot::DockVolume
        } else {
            Hotspot::DockMeta
        });
    }
    if layout.progress.size.height > 0 && layout.progress.contains(p) {
        return Some(Hotspot::Seek);
    }
    None
}

/// Vertical gap between lines of the same field.
const LINE_GAP: i32 = 2;
/// Vertical gap between fields.
const FIELD_GAP: i32 = 8;
/// Pixels below the pinned `starting` footer.
const FOOTER_MARGIN: i32 = 16;

/// Title and fact faces for the status screen.
///
/// Large is `FONT_10X20` for both. That is the biggest stock face; a 2×
/// meta line is wider than the panel, so addresses wrap instead.
fn status_fonts(mode: StatusText) -> (&'static MonoFont<'static>, &'static MonoFont<'static>) {
    match mode {
        StatusText::Normal => (TITLE_FONT, META_FONT),
        StatusText::Large => (&FONT_10X20, &FONT_10X20),
    }
}

/// Append `text` wrapped to `width` with `style`.
fn push_wrapped(
    lines: &mut Vec<(String, MonoTextStyle<'static, Rgb565>)>,
    text: &str,
    width: u32,
    font: &'static MonoFont<'static>,
    style: MonoTextStyle<'static, Rgb565>,
) {
    for part in wrap(text, width, font) {
        lines.push((part, style));
    }
}

/// Greedy word wrap to a pixel width, for a monospaced font.
///
/// A word longer than the line is hard-split rather than left to overflow:
/// long unbroken strings are common in filenames and stream titles, and
/// silently clipping them loses the part most likely to identify the track.
pub(crate) fn wrap(text: &str, width_px: u32, font: &MonoFont) -> Vec<String> {
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

/// Track text, word-wrapped into the column.
///
/// Wrapping rather than a horizontal marquee. Scrolling is the fallback
/// when the wrapped block is taller than the slot: the block moves
/// vertically, which is the direction of the overflow.
#[derive(Debug, Default)]
pub struct TextPane {
    title: Vec<String>,
    artist: Vec<String>,
    album: Vec<String>,
    src: (String, String, String),
    height: i32,
    offset: i32,
    over: i32,
    forward: bool,
    hold: u8,
}

/// Ticks held at each end before reversing.
const HOLD_TICKS: u8 = 20;
/// Pixels moved per tick.
const STEP: i32 = 1;

impl TextPane {
    /// Re-wrap for new content. Unchanged strings keep the scroll.
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

    /// Advance one tick. True when the pane needs a repaint.
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
    fn new(size: Size, bg: Rgb565) -> Self {
        Self {
            px: vec![bg; (size.width * size.height) as usize],
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

/// Draw the status screen shown before the player answers.
///
/// This is what is on the panel from a few seconds after power on until
/// `GET /status` is `ready` and the first `getState` succeeds. It exists
/// because the alternative is a dark screen, and because the address is the
/// one thing someone needs before the player is reachable.
///
/// `footer` is an optional dim line pinned near the bottom, used while
/// the backend is still starting. Title font, so it is readable at arm's
/// length, and kept off the address block so it does not read as another
/// address. One character of spinner so the width does not jump.
///
/// Uses the whole frame rather than the player's text column: a full IPv6
/// address is 39 characters, 234 pixels at the meta font, and the column is
/// only 132 wide.
///
/// Composed in memory and blitted once, same as the text pane, for the same
/// reason: drawing onto a clipped view of the panel forces per-pixel
/// addressing and flickers.
pub fn draw_status<D>(
    target: &mut D,
    layout: &Layout,
    host: &HostInfo,
    footer: Option<&str>,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pal = layout.pal();
    let region = layout.frame;
    let mut buf = RowBuf::new(region.size, pal.bg);

    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Top)
        .alignment(Alignment::Center)
        .build();
    let cx = region.size.width as i32 / 2;

    let (title_font, fact_font) = status_fonts(layout.status_text);
    let width = region.size.width;
    let title = MonoTextStyle::new(title_font, pal.title);
    let section = MonoTextStyle::new(title_font, pal.meta);
    let fact = MonoTextStyle::new(fact_font, pal.meta);
    let status = MonoTextStyle::new(title_font, pal.dim);

    // Hostname first so a long name wraps like an address, then the body.
    // Build the lines first so the block height is known and it can be
    // centred, rather than guessing a starting offset per state.
    let mut head: Vec<(String, MonoTextStyle<Rgb565>)> = Vec::new();
    push_wrapped(&mut head, &host.hostname, width, title_font, title);

    let mut body: Vec<(String, MonoTextStyle<Rgb565>)> = Vec::new();
    if host.addrs.is_empty() && host.hotspot.is_none() {
        push_wrapped(&mut body, "waiting for network", width, title_font, status);
    } else {
        for a in &host.addrs {
            push_wrapped(
                &mut body,
                &format!("{}  {}", a.link.label(), a.addr),
                width,
                fact_font,
                fact,
            );
        }
        if let Some(hs) = &host.hotspot {
            // Instruction, not another LAN line: join this network, then
            // open this address.
            push_wrapped(&mut body, "Wi-Fi setup", width, title_font, section);
            push_wrapped(&mut body, &hs.ssid, width, fact_font, fact);
            push_wrapped(&mut body, &hs.addr, width, fact_font, fact);
        }
    }

    let title_h = title_font.character_size.height as i32;
    let split = if !host.addrs.is_empty() && host.hotspot.is_some() {
        FIELD_GAP
    } else {
        0
    };
    let hotspot_at = if split > 0 {
        // First body line that is the hotspot heading. Address lines may
        // have wrapped, so this is "after the last address line".
        let addr_lines = host
            .addrs
            .iter()
            .map(|a| wrap(&format!("{}  {}", a.link.label(), a.addr), width, fact_font).len())
            .sum();
        Some(addr_lines)
    } else {
        None
    };

    let stacked = |rows: &[(String, MonoTextStyle<Rgb565>)]| {
        if rows.is_empty() {
            return 0;
        }
        rows.iter()
            .map(|(_, s)| s.font.character_size.height as i32)
            .sum::<i32>()
            + (rows.len() as i32 - 1) * LINE_GAP
    };
    let mut body_h = stacked(&body);
    if let Some(i) = hotspot_at {
        if i > 0 && i < body.len() {
            body_h += FIELD_GAP;
        }
    }
    let block = stacked(&head) + FIELD_GAP + body_h;
    let reserved = if footer.is_some() {
        title_h + FOOTER_MARGIN * 2
    } else {
        0
    };
    let mut y = ((region.size.height as i32 - reserved - block) / 2).max(0);

    // Infallible: RowBuf discards out-of-bounds pixels.
    for (text, style) in &head {
        let _ = Text::with_text_style(text, Point::new(cx, y), *style, centred).draw(&mut buf);
        y += style.font.character_size.height as i32 + LINE_GAP;
    }
    y += FIELD_GAP - LINE_GAP;

    for (i, (text, style)) in body.iter().enumerate() {
        if Some(i) == hotspot_at {
            y += FIELD_GAP;
        }
        let _ = Text::with_text_style(text, Point::new(cx, y), *style, centred).draw(&mut buf);
        y += style.font.character_size.height as i32 + LINE_GAP;
    }

    if let Some(footer) = footer {
        let fy = region.size.height as i32 - FOOTER_MARGIN - title_h;
        let _ = Text::with_text_style(footer, Point::new(cx, fy), status, centred).draw(&mut buf);
    }

    target.fill_contiguous(&region, buf.px.iter().copied())
}

/// Draw the whole resting face.
///
/// Clears and repaints, so this is only for a scene change. A full frame is
/// about 39 ms at 32 MHz, and doing that twice a second because `seek`
/// advanced is visible as a flicker. Progress and the text pane are
/// repainted in place.
pub fn draw<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
    art: Option<&Art>,
    pane: &TextPane,
    scrub: Option<f32>,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_face(target, layout, state, art, pane, scrub)
}

/// Resting face: art, i ring, title block, dock, seek strip.
pub fn draw_face<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
    art: Option<&Art>,
    pane: &TextPane,
    scrub: Option<f32>,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pal = layout.pal();
    target.clear(pal.bg)?;
    draw_art(target, layout, art)?;
    draw_text(target, face_text_slot(layout), pane, pal)?;
    draw_info_ring(target, layout, pal)?;
    draw_dock(target, layout, state, pal)?;
    draw_progress(target, layout, state, scrub)?;
    Ok(())
}

/// Repaint only the title block. The ticker uses this so a step does not
/// clear art, dock or seek.
pub fn draw_rows<D>(target: &mut D, layout: &Layout, pane: &TextPane) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_text(target, face_text_slot(layout), pane, layout.pal())
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
    let pal = layout.pal();
    let box_ = layout.art;

    if let Some(art) = art {
        let w = art.w.min(box_.size.width);
        let h = art.h.min(box_.size.height);
        let x = box_.top_left.x + (box_.size.width as i32 - w as i32) / 2;
        let y = box_.top_left.y + (box_.size.height as i32 - h as i32) / 2;
        let placed = Rectangle::new(Point::new(x, y), Size::new(w, h));

        if placed != box_ {
            box_.into_styled(PrimitiveStyle::with_fill(pal.bg))
                .draw(target)?;
        }

        target.fill_contiguous(&placed, art.px.iter().copied())?;
    } else {
        box_.into_styled(PrimitiveStyle::with_fill(pal.bg))
            .draw(target)?;
    }

    Ok(())
}

fn draw_info_ring<D>(target: &mut D, layout: &Layout, pal: Palette) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    // Hit stays 48×48 / 44×44. Paint only the ring: a full-hit fill is a
    // 48 px slab on the cover. Hairline + dim — not meta, not 2 px.
    let c = layout.info.center();
    Circle::with_center(c, INFO_RING + 2)
        .into_styled(PrimitiveStyle::with_fill(pal.bg))
        .draw(target)?;
    Circle::with_center(c, INFO_RING)
        .into_styled(PrimitiveStyle::with_stroke(pal.dim, 1))
        .draw(target)?;
    let style = MonoTextStyle::new(META_FONT, pal.dim);
    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Middle)
        .alignment(Alignment::Center)
        .build();
    Text::with_text_style("IP", c, style, centred).draw(target)?;
    Ok(())
}

/// A.1 / A.2 status ring. Hit stays 48×48 / 44×44.
const INFO_RING: u32 = 18;

/// Inset from the art box and the column edges. A.2 called this pad 10;
/// without it landscape title sits on the art's right edge.
const FACE_TEXT_PAD: i32 = 10;

/// Top-left of the title block. Landscape's IP ring sits in the same
/// column; the first line starts under that hit. Always inset from art.
fn face_text_origin(layout: &Layout) -> Point {
    let region = layout.text;
    let overlap = layout.info.intersection(&region);
    let under_info = overlap.size.width > 0 && layout.info.top_left.y <= region.top_left.y + 4;
    let y = if under_info {
        layout.info.top_left.y + layout.info.size.height as i32
    } else {
        region.top_left.y + FACE_TEXT_PAD
    };
    Point::new(region.top_left.x + FACE_TEXT_PAD, y)
}

fn face_text_width(layout: &Layout) -> u32 {
    layout
        .text
        .size
        .width
        .saturating_sub((FACE_TEXT_PAD * 2) as u32)
}

/// Clip box for title / artist / album. Starts under the IP hit on
/// landscape; inset from art on both orientations.
pub fn face_text_slot(layout: &Layout) -> Rectangle {
    let origin = face_text_origin(layout);
    let bottom = layout.text.top_left.y + layout.text.size.height as i32;
    let height = (bottom - origin.y).max(0) as u32;
    Rectangle::new(origin, Size::new(face_text_width(layout), height))
}

/// Title, artist, album. Left-aligned in the slot. Centred vertically
/// when the block fits; top-aligned and scrolled when it does not.
fn draw_text<D>(
    target: &mut D,
    region: Rectangle,
    pane: &TextPane,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let mut buf = RowBuf::new(region.size, pal.bg);
    let left = TextStyleBuilder::new()
        .baseline(Baseline::Top)
        .alignment(Alignment::Left)
        .build();
    let mut y = if pane.over == 0 {
        (region.size.height as i32 - pane.height) / 2
    } else {
        -pane.offset
    };
    let fields: [(&Vec<String>, &MonoFont, Rgb565); 3] = [
        (&pane.title, TITLE_FONT, pal.title),
        (&pane.artist, META_FONT, pal.meta),
        (&pane.album, META_FONT, pal.dim),
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
            let _ = Text::with_text_style(line, Point::new(0, y), style, left).draw(&mut buf);
            y += font.character_size.height as i32 + LINE_GAP;
        }
        y -= LINE_GAP;
    }
    target.fill_contiguous(&region, buf.px.iter().copied())
}

fn draw_dock<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    layout
        .dock
        .into_styled(PrimitiveStyle::with_fill(pal.bg))
        .draw(target)?;
    Line::new(
        Point::new(layout.dock.top_left.x, layout.dock.top_left.y),
        Point::new(
            layout.dock.top_left.x + layout.dock.size.width as i32 - 1,
            layout.dock.top_left.y,
        ),
    )
    .into_styled(PrimitiveStyle::with_stroke(pal.dim, 1))
    .draw(target)?;
    let cells = [
        layout.dock_cell(0),
        layout.dock_cell(1),
        layout.dock_cell(2),
    ];
    draw_dock_transport(target, cells[0], state.is_playing(), pal)?;
    draw_speaker_mark(
        target,
        cells[1],
        state.is_muted() || state.volume.unwrap_or(0) == 0,
        pal,
    )?;
    draw_list_icon(target, cells[2], pal.meta)?;
    Ok(())
}

/// One optical box for play, speaker and list. 22 is the A.1 glyph
/// (~22 in an 80×52 cell). Landscape cells are 40 px: 36 filled them
/// and left no gap.
const DOCK_GLYPH: i32 = 22;

fn dock_glyph_center(cell: Rectangle) -> Point {
    cell.center()
}

/// Two bars as one box centered on `center`. The old pair used
/// `c.x - (w + 2)` and `c.x + 2`, so `center()` was one pixel left
/// of the ring (`Circle::with_center` / `Rectangle::with_center`).
fn dock_pause_bars(center: Point) -> [Rectangle; 2] {
    let w = 3.max(DOCK_GLYPH / 10);
    let gap = 4;
    let h = (DOCK_GLYPH * 12 / 28).max(1);
    let pair = Rectangle::with_center(center, Size::new((2 * w + gap) as u32, h as u32));
    let left = pair.top_left;
    [
        Rectangle::new(left, Size::new(w as u32, h as u32)),
        Rectangle::new(
            Point::new(left.x + w + gap, left.y),
            Size::new(w as u32, h as u32),
        ),
    ]
}

/// Ringed play / pause. Circle + mark, same language as the reference dock.
fn draw_dock_transport<D>(
    target: &mut D,
    cell: Rectangle,
    playing: bool,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let c = dock_glyph_center(cell);
    let d = DOCK_GLYPH as u32;
    Circle::with_center(c, d)
        .into_styled(PrimitiveStyle::with_stroke(pal.title, 2))
        .draw(target)?;
    if playing {
        for bar in dock_pause_bars(c) {
            bar.into_styled(PrimitiveStyle::with_fill(pal.title))
                .draw(target)?;
        }
    } else {
        let r = DOCK_GLYPH / 2;
        Triangle::new(
            Point::new(c.x - r * 5 / 14, c.y - r * 7 / 14),
            Point::new(c.x - r * 5 / 14, c.y + r * 7 / 14),
            Point::new(c.x + r * 7 / 14, c.y),
        )
        .into_styled(PrimitiveStyle::with_fill(pal.title))
        .draw(target)?;
    }
    Ok(())
}

fn draw_list_icon<D>(target: &mut D, cell: Rectangle, colour: Rgb565) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let c = dock_glyph_center(cell);
    let w = DOCK_GLYPH;
    let gap = DOCK_GLYPH / 5;
    let block = gap * 2 + 2;
    let top = c.y - block / 2;
    for i in 0..3 {
        Rectangle::new(
            Point::new(c.x - w / 2, top + i * gap),
            Size::new(w as u32, 2),
        )
        .into_styled(PrimitiveStyle::with_fill(colour))
        .draw(target)?;
    }
    Ok(())
}

/// The signed-off 12×10 speaker, 2×. 3× filled a 40 px landscape cell.
/// Do not replace with a free triangle — that reads as an arrow.
fn draw_speaker_mark<D>(
    target: &mut D,
    cell: Rectangle,
    silent: bool,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_speaker_at(target, dock_glyph_center(cell), silent, pal, 2)
}

/// Signed-off speaker, origin from `Rectangle::with_center` so it shares
/// a center with the rest of a row.
pub fn draw_speaker_at<D>(
    target: &mut D,
    center: Point,
    silent: bool,
    pal: Palette,
    scale: i32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let colour = speaker_colour(silent, if silent { 0 } else { 1 }, pal);
    let box_ = speaker_box(center, scale);
    let origin = box_.top_left;
    let scale = scale.max(1);
    plot_icon_px_scaled(target, origin, SPEAKER_BODY, colour, scale)?;
    if silent {
        plot_icon_px_scaled(target, origin, SPEAKER_MUTE_X, colour, scale)?;
    } else {
        plot_icon_px_scaled(target, origin, SPEAKER_WAVES, colour, scale)?;
    }
    Ok(())
}

/// Bounding box of the 12×10 speaker at `scale`, centered on `center`.
pub fn speaker_box(center: Point, scale: i32) -> Rectangle {
    let scale = scale.max(1);
    Rectangle::with_center(
        center,
        Size::new((SPEAKER_W * scale) as u32, (SPEAKER_H * scale) as u32),
    )
}

fn plot_icon_px_scaled<D>(
    target: &mut D,
    origin: Point,
    pixels: &[(u8, u8)],
    colour: Rgb565,
    scale: i32,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let scale = scale.max(1);
    for &(x, y) in pixels {
        Rectangle::new(
            Point::new(
                origin.x + i32::from(x) * scale,
                origin.y + i32::from(y) * scale,
            ),
            Size::new(scale as u32, scale as u32),
        )
        .into_styled(PrimitiveStyle::with_fill(colour))
        .draw(target)?;
    }
    Ok(())
}

/// Repaint the seek / stream slot.
///
/// Progress uses seek/duration. Stream paints IN fields the source wrote.
/// Off, or a source that published nothing for that mode, is black. The
/// seek/duration unit mismatch is handled in `PlayerState::progress`.
pub fn draw_progress<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
    scrub: Option<f32>,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pal = layout.pal();
    match layout.strip {
        Strip::Off => Ok(()),
        Strip::Stream => draw_stream_info(target, layout.progress, state, pal),
        Strip::Progress => draw_seek_strip(target, layout.progress, state, pal, scrub),
    }
}

fn blank_strip<D>(target: &mut D, bar: Rectangle, pal: Palette) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    bar.into_styled(PrimitiveStyle::with_fill(pal.bg))
        .draw(target)
}

/// One centred line of IN format. Too long is clipped, not wrapped: the
/// slot is one face tall.
fn draw_stream_info<D>(
    target: &mut D,
    bar: Rectangle,
    state: &PlayerState,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let Some(text) = state.stream_info() else {
        return blank_strip(target, bar, pal);
    };

    let cols = (bar.size.width / META_FONT.character_size.width).max(1) as usize;
    let shown: String = text.chars().take(cols).collect();

    let mut buf = RowBuf::new(bar.size, pal.bg);
    let style = MonoTextStyle::new(META_FONT, pal.meta);
    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Middle)
        .alignment(Alignment::Center)
        .build();
    let _ = Text::with_text_style(
        &shown,
        Point::new(bar.size.width as i32 / 2, bar.size.height as i32 / 2),
        style,
        centred,
    )
    .draw(&mut buf);
    target.fill_contiguous(&bar, buf.px.iter().copied())
}

/// `m:ss` under an hour, `h:mm:ss` at or above. Matches the Web player's
/// left/right counters, not a single `elapsed / total` string.
fn fmt_clock(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// Seek strip: clocks, title-coloured fill, 4×12 knob. `scrub` overrides
/// the live fraction while a finger is down.
pub(crate) fn draw_seek_strip<D>(
    target: &mut D,
    slot: Rectangle,
    state: &PlayerState,
    pal: Palette,
    scrub: Option<f32>,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    if slot.size.height == 0 {
        return Ok(());
    }
    let frac = scrub.or_else(|| state.progress());
    let Some(frac) = frac else {
        return blank_strip(target, slot, pal);
    };
    let total = state.duration.unwrap_or(0);
    let elapsed = if let Some(s) = scrub {
        (s.clamp(0.0, 1.0) * total as f32) as u64
    } else {
        state.seek.unwrap_or(0) / 1000
    };
    let left = fmt_clock(elapsed);
    let right = fmt_clock(total);
    let char_w = META_FONT.character_size.width;
    let clock_w = left.chars().count().max(right.chars().count()).max(4) as u32 * char_w;
    let pad = 8u32;
    let gap = 10u32;
    let track_x = pad + clock_w + gap;
    let inner_w = slot.size.width.saturating_sub(track_x * 2);
    let bar_h = 6u32.min(slot.size.height.saturating_sub(4));
    let bar_y = (slot.size.height.saturating_sub(bar_h)) / 2;

    let mut buf = RowBuf::new(slot.size, pal.bg);
    let style = MonoTextStyle::new(META_FONT, pal.meta);
    let left_align = TextStyleBuilder::new()
        .baseline(Baseline::Middle)
        .alignment(Alignment::Left)
        .build();
    let right_align = TextStyleBuilder::new()
        .baseline(Baseline::Middle)
        .alignment(Alignment::Right)
        .build();
    let cy = slot.size.height as i32 / 2;
    let _ =
        Text::with_text_style(&left, Point::new(pad as i32, cy), style, left_align).draw(&mut buf);
    let _ = Text::with_text_style(
        &right,
        Point::new(slot.size.width as i32 - pad as i32, cy),
        style,
        right_align,
    )
    .draw(&mut buf);

    if inner_w > 0 {
        let track = Rectangle::new(
            Point::new(track_x as i32, bar_y as i32),
            Size::new(inner_w, bar_h),
        );
        let _ = fill_bar(&mut buf, track, frac, pal.dim, pal.title);
        let kx = track.top_left.x + (track.size.width as f32 * frac.clamp(0.0, 1.0)) as i32;
        let knob = Rectangle::new(Point::new(kx - 2, cy - 6), Size::new(4, 12));
        let _ = knob
            .into_styled(PrimitiveStyle::with_fill(pal.title))
            .draw(&mut buf);
    }
    target.fill_contiguous(&slot, buf.px.iter().copied())
}

/// Title-colour cabinet/cone/waves with level; danger cabinet/cone/cross when silent.
fn speaker_colour(muted: bool, volume: u8, pal: Palette) -> Rgb565 {
    if muted || volume == 0 {
        pal.danger
    } else {
        pal.title
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(r: Rectangle) -> (i32, i32, u32, u32) {
        (r.top_left.x, r.top_left.y, r.size.width, r.size.height)
    }

    #[test]
    fn portrait_matches_adr0020_a1() {
        let l = Layout::for_rotation(0);
        assert_eq!(origin(l.art), (44, 0, 152, 152));
        assert_eq!(origin(l.text), (12, 162, 216, 74));
        assert_eq!(origin(l.info), (192, 0, 48, 48));
        assert_eq!(origin(l.dock), (0, 236, 240, 52));
        assert_eq!(origin(l.progress), (0, 288, 240, 32));
        assert_eq!(l.theme, Theme::Ink);
    }

    #[test]
    fn landscape_matches_adr0020_a2() {
        let l = Layout::for_rotation(270);
        assert_eq!(origin(l.art), (0, 0, 200, 200));
        assert_eq!(origin(l.info), (276, 0, 44, 44));
        assert_eq!(origin(l.dock), (200, 156, 120, 44));
        assert_eq!(origin(l.progress), (0, 200, 320, 40));
        assert_eq!(origin(l.dock_cell(0)), (200, 156, 40, 44));
        let a = l.dock_cell(0).center();
        let b = l.dock_cell(1).center();
        let c = l.dock_cell(2).center();
        assert_eq!(b.x - a.x, c.x - b.x);
        assert_eq!(a.y, b.y);
        assert_eq!(b.y, c.y);
        assert!(
            l.dock_cell(0).size.width as i32 >= DOCK_GLYPH + 8,
            "landscape cells must leave a gap around the glyph"
        );
    }

    #[test]
    fn pause_bars_share_the_ring_center() {
        let c = Point::new(20, 22);
        let ring = Circle::with_center(c, DOCK_GLYPH as u32);
        let [a, b] = dock_pause_bars(c);
        let right = b.top_left.x + b.size.width as i32;
        let pair = Rectangle::new(
            a.top_left,
            Size::new((right - a.top_left.x) as u32, a.size.height),
        );
        assert_eq!(pair.center(), ring.center());
        assert_eq!(ring.center(), c);
        assert_eq!(a.size, b.size);
        assert_eq!(a.top_left.y, b.top_left.y);
    }

    #[test]
    fn landscape_title_starts_below_the_info_ring() {
        let l = Layout::for_rotation(270);
        let o = face_text_origin(&l);
        assert_eq!(o.x, 210);
        assert_eq!(o.y, 44);
        let p = Layout::for_rotation(0);
        assert_eq!(face_text_origin(&p), Point::new(22, 172));
        assert_eq!(INFO_RING, 18);
    }

    #[test]
    fn long_title_scrolls_the_block_vertically() {
        let l = Layout::for_rotation(0);
        let slot = face_text_slot(&l);
        let mut pane = TextPane::default();
        pane.set(
            "A long title that must wrap more than twice so the block is taller than the slot",
            "An artist name that also wraps on the portrait column",
            "And an album title that adds a third field",
            slot,
        );
        assert!(pane.over > 0);
        let mut moved = false;
        for _ in 0..(HOLD_TICKS as usize + 3) {
            if pane.step() {
                moved = true;
            }
        }
        assert!(moved);
        assert!(pane.offset > 0);
        pane.set(
            "A long title that must wrap more than twice so the block is taller than the slot",
            "An artist name that also wraps on the portrait column",
            "And an album title that adds a third field",
            slot,
        );
        assert!(pane.offset > 0, "same strings must not restart the scroll");
    }

    #[test]
    fn large_status_wraps_ipv6_instead_of_one_wide_line() {
        let v6 = "fd12:3456:789a:bcde:0123:4567:89ab:cdef";
        assert_eq!(v6.chars().count(), 39);
        let parts = wrap(v6, 240, &FONT_10X20);
        assert!(parts.len() >= 2, "{parts:?}");
        assert!(parts.iter().all(|p| p.chars().count() * 10 <= 240));
    }

    #[test]
    fn from_config_uses_the_orientation_keys() {
        let cfg = Config {
            rotation: 270,
            status_text_landscape: StatusText::Large,
            bar_gap_landscape: BarGap::Roomy,
            status_text_portrait: StatusText::Normal,
            bar_gap_portrait: BarGap::Tight,
            strip_landscape: Strip::Stream,
            strip_portrait: Strip::Off,
            theme: Theme::Studio,
            ..Config::default()
        };
        let l = Layout::from_config(&cfg);
        assert_eq!(l.status_text, StatusText::Large);
        assert_eq!(l.art.size.width, 200);
        assert_eq!(l.strip, Strip::Stream);
        assert_eq!(l.theme, Theme::Studio);
        assert_eq!(origin(l.progress), (0, 200, 320, 40));
    }

    #[test]
    fn off_hides_seek_and_grows_art() {
        let p = Layout::compose(
            0,
            BarGap::Default,
            StatusText::Normal,
            Strip::Off,
            Theme::Ink,
        );
        assert_eq!(p.art.size.height, 184);
        assert_eq!(p.progress.size.height, 0);
        assert_eq!(origin(p.dock), (0, 268, 240, 52));
    }

    #[test]
    fn seek_fraction_pads_twelve_pixels() {
        let slot = rect(0, 0, 240, 32);
        assert_eq!(seek_fraction(slot, Point::new(12, 16)), 0.0);
        assert_eq!(seek_fraction(slot, Point::new(228, 16)), 1.0);
        let mid = seek_fraction(slot, Point::new(120, 16));
        assert!((mid - 0.5).abs() < 0.02, "{mid}");
    }

    #[test]
    fn face_hit_prefers_the_info_ring() {
        let l = Layout::for_rotation(0);
        assert_eq!(face_hit(&l, Point::new(200, 10)), Some(Hotspot::Info));
        assert_eq!(face_hit(&l, Point::new(100, 40)), Some(Hotspot::Art));
        assert_eq!(
            face_hit(&l, Point::new(40, 250)),
            Some(Hotspot::DockControls)
        );
        assert_eq!(face_hit(&l, Point::new(120, 300)), Some(Hotspot::Seek));
    }

    #[test]
    fn clock_matches_the_web_player() {
        assert_eq!(fmt_clock(0), "0:00");
        assert_eq!(fmt_clock(323), "5:23");
        assert_eq!(fmt_clock(269), "4:29");
        assert_eq!(fmt_clock(3600), "1:00:00");
        assert_eq!(fmt_clock(3661), "1:01:01");
    }

    #[test]
    fn speaker_is_red_when_silent() {
        let ink = palette(Theme::Ink);
        assert_eq!(speaker_colour(false, 48, ink), Rgb565::WHITE);
        assert_eq!(speaker_colour(true, 48, ink), Rgb565::CSS_RED);
        assert_eq!(speaker_colour(false, 0, ink), Rgb565::CSS_RED);
        assert_eq!(speaker_colour(true, 0, ink), Rgb565::CSS_RED);
    }

    #[test]
    fn ink_matches_the_shipped_css() {
        let ink = palette(Theme::Ink);
        assert_eq!(ink.bg, Rgb565::BLACK);
        assert_eq!(ink.title, Rgb565::WHITE);
        assert_eq!(ink.meta, Rgb565::CSS_LIGHT_GRAY);
        assert_eq!(ink.dim, Rgb565::CSS_DIM_GRAY);
        assert_eq!(ink.accent, Rgb565::CSS_ORANGE);
        assert_eq!(ink.danger, Rgb565::CSS_RED);
    }

    #[test]
    fn adr0020_roster_paints_the_table_words() {
        let ink = palette(Theme::Ink);
        let dusk = palette(Theme::Dusk);
        let studio = palette(Theme::Studio);
        let night = palette(Theme::Night);
        assert_eq!(ink.bg, word(0x0000));
        assert_eq!(ink.title, word(0xFFFF));
        assert_eq!(ink.meta, word(0xD69A));
        assert_eq!(ink.dim, word(0x6B4D));
        assert_eq!(ink.accent, word(0xFD20));
        assert_eq!(ink.danger, word(0xF800));
        assert_eq!(dusk.bg, word(0x30C1));
        assert_eq!(dusk.title, word(0xFEB5));
        assert_eq!(dusk.meta, word(0xD52E));
        assert_eq!(dusk.dim, word(0x9B07));
        assert_eq!(dusk.accent, word(0xFD85));
        assert_eq!(dusk.danger, word(0xFA87));
        assert_eq!(studio.bg, word(0x08C7));
        assert_eq!(studio.title, word(0xDF5F));
        assert_eq!(studio.meta, word(0x959A));
        assert_eq!(studio.dim, word(0x5352));
        assert_eq!(studio.accent, word(0x3DBF));
        assert_eq!(studio.danger, word(0xFA4A));
        assert_eq!(night.bg, word(0x1040));
        assert_eq!(night.title, word(0xFD89));
        assert_eq!(night.meta, word(0xCC67));
        assert_eq!(night.dim, word(0x7264));
        assert_eq!(night.accent, word(0xFF57));
        assert_eq!(night.danger, word(0xF944));
        assert_ne!(dusk.bg, ink.bg);
        assert_ne!(night.bg, ink.bg);
        assert_ne!(dusk.accent, studio.accent);
    }
}
