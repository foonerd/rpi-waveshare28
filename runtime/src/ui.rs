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
    mono_font::{
        ascii::FONT_10X20, ascii::FONT_6X10, ascii::FONT_9X15_BOLD, MonoFont, MonoTextStyle,
    },
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
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

const TITLE_FONT: &MonoFont = &FONT_9X15_BOLD;
const META_FONT: &MonoFont = &FONT_6X10;

/// Speaker mark to the left of the volume track. ASCII fonts have no
/// speaker glyph. 12×10: cabinet + cone, then waves or a mute cross.
const VOL_ICON_W: u32 = 12;
const VOL_ICON_H: u32 = 10;
const VOL_ICON_GAP: u32 = 2;

/// Cabinet and filled cone. Coordinates in the 12×10 icon.
const SPEAKER_BODY: &[(u8, u8)] = &[
    // cabinet
    (0, 3),
    (1, 3),
    (0, 4),
    (1, 4),
    (0, 5),
    (1, 5),
    (0, 6),
    (1, 6),
    // cone
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

/// Two arcs to the right of the cone, the usual “has sound” mark.
const SPEAKER_WAVES: &[(u8, u8)] = &[
    // inner
    (6, 2),
    (7, 3),
    (7, 6),
    (6, 7),
    // outer
    (8, 1),
    (9, 2),
    (10, 3),
    (10, 6),
    (9, 7),
    (8, 8),
];

/// Cross in the wave area when muted or at zero.
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

/// The orange track, after the speaker. Hit mapping uses this so 0% is
/// the start of the bar, not the icon.
fn volume_track(slot: Rectangle) -> Rectangle {
    let inset = VOL_ICON_W + VOL_ICON_GAP;
    Rectangle::new(
        slot.top_left + Point::new(inset as i32, 0),
        Size::new(slot.size.width.saturating_sub(inset), slot.size.height),
    )
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
    /// Track text: title, artist, album, wrapped and centred.
    pub text: Rectangle,
    /// Volume slider.
    pub volume: Rectangle,
    /// Playback progress bar.
    pub progress: Rectangle,
    /// Transport strip, split into equal thirds.
    pub transport: Rectangle,
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

const fn rgb(r: u8, g: u8, b: u8) -> Rgb565 {
    Rgb565::new(r >> 3, g >> 2, b >> 3)
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
        // RGB565 on this glass. Near-black and cream-on-white do not read.
        // These steps are for arm's length, not sRGB taste.
        Theme::Dusk => Palette {
            bg: rgb(80, 36, 12),
            title: rgb(255, 220, 160),
            meta: rgb(220, 168, 96),
            dim: rgb(160, 96, 48),
            accent: rgb(255, 200, 48),
            danger: rgb(255, 72, 48),
        },
        Theme::Studio => Palette {
            bg: rgb(12, 28, 72),
            title: rgb(200, 220, 255),
            meta: rgb(140, 168, 200),
            dim: rgb(64, 88, 128),
            accent: rgb(48, 160, 255),
            danger: Rgb565::CSS_RED,
        },
    }
}

fn rect(x: i32, y: i32, w: u32, h: u32) -> Rectangle {
    Rectangle::new(Point::new(x, y), Size::new(w, h))
}

impl Layout {
    fn pal(&self) -> Palette {
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
        gap: BarGap,
        status_text: StatusText,
        strip: Strip,
        theme: Theme,
    ) -> Self {
        match rotation {
            90 | 270 => Self::landscape(rotation, gap, status_text, strip, theme),
            _ => Self::portrait(rotation, gap, status_text, strip, theme),
        }
    }

    /// 240 wide by 320 tall. Art on top, everything else stacked beneath.
    ///
    /// Transport stays at y 292. Extra bar gap moves the slider up; roomy
    /// also shortens the art box by 4 px so the text block still fits.
    fn portrait(
        rotation: u16,
        gap: BarGap,
        status_text: StatusText,
        strip: Strip,
        theme: Theme,
    ) -> Self {
        let (art_h, text_y, vol_y, prog_y) = match gap {
            BarGap::Tight => (200, 214, 270, 280),
            BarGap::Default => (200, 214, 268, 280),
            BarGap::Roomy => (196, 210, 260, 280),
        };
        let (prog_y, prog_h) = match strip {
            Strip::Off => (prog_y, 4),
            _ => (prog_y - 4, 12),
        };
        Self {
            rotation,
            frame: rect(0, 0, 240, 320),
            art: rect(20, 8, 200, art_h),
            text: rect(4, text_y, 232, 48),
            volume: rect(10, vol_y, 220, 6),
            progress: rect(10, prog_y, 220, prog_h),
            transport: rect(0, 292, 240, 28),
            status_text,
            strip,
            theme,
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
    fn landscape(
        rotation: u16,
        gap: BarGap,
        status_text: StatusText,
        strip: Strip,
        theme: Theme,
    ) -> Self {
        // Transport is pinned at y 200, height 40. Roomy steals 12 px from
        // the art box; tight and default keep the 168 cover.
        let (art_s, vol_y, prog_y) = match gap {
            BarGap::Tight => (168, 178, 188),
            BarGap::Default => (168, 178, 190),
            BarGap::Roomy => (156, 168, 188),
        };
        let (prog_y, prog_h) = match strip {
            Strip::Off => (prog_y, 4),
            _ => (prog_y - 4, 12),
        };
        Self {
            rotation,
            frame: rect(0, 0, 320, 240),
            art: rect(10, 4, art_s, art_s),
            text: rect(184, 4, 132, art_s),
            volume: rect(10, vol_y, 300, 6),
            progress: rect(10, prog_y, 300, prog_h),
            transport: rect(0, 200, 320, 40),
            status_text,
            strip,
            theme,
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
    /// Tap on the art area: show host addresses.
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
        let track = volume_track(slider);
        let dx = (p.x - track.top_left.x).max(0) as u32;
        let pct = (dx * 100 / track.size.width.max(1)).min(100);
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

/// Draw the text pane: title, artist and album, wrapped, centred both ways.
///
/// Vertically centred when the block fits, top-aligned and scrolled when it
/// does not, because centring something that is moving reads as a fault
/// rather than as a deliberate scroll.
///
/// Composed in memory and blitted in one write. See [`RowBuf`] for why.
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
    target.clear(layout.pal().bg)?;

    draw_art(target, layout, art)?;
    draw_text(target, layout.text, pane, layout.pal())?;

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

    // Affordance: the whole art box opens the address overlay. A chip so
    // that is findable on a cover that would otherwise hide it.
    draw_info_mark(target, box_, pal)
}

/// Small `i` in the art-box corner. The hit target is the whole box.
fn draw_info_mark<D>(target: &mut D, box_: Rectangle, pal: Palette) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    const CHIP: u32 = 16;
    let chip = Rectangle::new(
        Point::new(
            box_.top_left.x + 2,
            box_.top_left.y + box_.size.height as i32 - CHIP as i32 - 2,
        ),
        Size::new(CHIP, CHIP),
    );
    chip.into_styled(PrimitiveStyle::with_fill(pal.dim))
        .draw(target)?;

    let style = MonoTextStyle::new(META_FONT, pal.title);
    let centred = TextStyleBuilder::new()
        .baseline(Baseline::Middle)
        .alignment(Alignment::Center)
        .build();
    Text::with_text_style(
        "i",
        chip.top_left + Point::new(CHIP as i32 / 2, CHIP as i32 / 2),
        style,
        centred,
    )
    .draw(target)?;
    Ok(())
}

/// Repaint the text pane only.
pub fn draw_rows<D>(target: &mut D, layout: &Layout, pane: &TextPane) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    draw_text(target, layout.text, pane, layout.pal())
}

/// Repaint the volume-to-transport slot.
///
/// Progress uses seek/duration. Stream paints IN fields the source wrote.
/// Off, or a source that published nothing for that mode, is black. The
/// seek/duration unit mismatch is handled in `PlayerState::progress`.
pub fn draw_progress<D>(
    target: &mut D,
    layout: &Layout,
    state: &PlayerState,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pal = layout.pal();
    match layout.strip {
        Strip::Off => blank_strip(target, layout.progress, pal),
        Strip::Stream => draw_stream_info(target, layout.progress, state, pal),
        Strip::Progress => draw_progress_times(target, layout.progress, state, pal),
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

/// Elapsed on the left, total on the right, 6 px bar between — same
/// thickness as the volume track, so the two slots are the same weight
/// and the clocks say which is seek.
fn draw_progress_times<D>(
    target: &mut D,
    slot: Rectangle,
    state: &PlayerState,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let Some(frac) = state.progress() else {
        return blank_strip(target, slot, pal);
    };
    let elapsed = state.seek.unwrap_or(0) / 1000;
    let total = state.duration.unwrap_or(0);
    let left = fmt_clock(elapsed);
    let right = fmt_clock(total);
    let char_w = META_FONT.character_size.width;
    let side = left.chars().count().max(right.chars().count()).max(4) as u32 * char_w;
    let gap = 2u32;
    let inner_w = slot.size.width.saturating_sub(side * 2 + gap * 2);
    let bar_h = 6u32.min(slot.size.height);
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
    let _ = Text::with_text_style(&left, Point::new(0, cy), style, left_align).draw(&mut buf);
    let _ = Text::with_text_style(
        &right,
        Point::new(slot.size.width as i32, cy),
        style,
        right_align,
    )
    .draw(&mut buf);

    if inner_w > 0 {
        let track = Rectangle::new(
            Point::new((side + gap) as i32, bar_y as i32),
            Size::new(inner_w, bar_h),
        );
        let _ = fill_bar(&mut buf, track, frac, pal.dim, pal.title);
    }
    target.fill_contiguous(&slot, buf.px.iter().copied())
}

/// Repaint the volume slider only.
pub fn draw_volume<D>(target: &mut D, layout: &Layout, state: &PlayerState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let pal = layout.pal();
    let slot = layout.volume;
    slot.into_styled(PrimitiveStyle::with_fill(pal.bg))
        .draw(target)?;

    let muted = state.is_muted();
    let volume = state.volume.unwrap_or(0);
    draw_speaker(target, slot, muted || volume == 0, pal)?;

    let frac = if muted {
        0.0
    } else {
        f32::from(volume) / 100.0
    };

    fill_bar(target, volume_track(slot), frac, pal.dim, pal.accent)
}

/// Title-colour cabinet/cone/waves with level; danger cabinet/cone/cross when silent.
fn speaker_colour(muted: bool, volume: u8, pal: Palette) -> Rgb565 {
    if muted || volume == 0 {
        pal.danger
    } else {
        pal.title
    }
}

fn plot_icon_px<D>(
    target: &mut D,
    origin: Point,
    pixels: &[(u8, u8)],
    colour: Rgb565,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    for &(x, y) in pixels {
        Pixel(
            Point::new(origin.x + i32::from(x), origin.y + i32::from(y)),
            colour,
        )
        .draw(target)?;
    }
    Ok(())
}

fn draw_speaker<D>(
    target: &mut D,
    slot: Rectangle,
    silent: bool,
    pal: Palette,
) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    let colour = speaker_colour(silent, if silent { 0 } else { 1 }, pal);
    let origin = Point::new(
        slot.top_left.x,
        slot.top_left.y + (slot.size.height as i32 - VOL_ICON_H as i32) / 2,
    );
    Rectangle::new(origin, Size::new(VOL_ICON_W, VOL_ICON_H))
        .into_styled(PrimitiveStyle::with_fill(pal.bg))
        .draw(target)?;
    plot_icon_px(target, origin, SPEAKER_BODY, colour)?;
    if silent {
        plot_icon_px(target, origin, SPEAKER_MUTE_X, colour)?;
    } else {
        plot_icon_px(target, origin, SPEAKER_WAVES, colour)?;
    }
    Ok(())
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
    let pal = layout.pal();
    let strip = layout.transport;
    strip
        .into_styled(PrimitiveStyle::with_fill(pal.bg))
        .draw(target)?;

    let style = MonoTextStyle::new(TITLE_FONT, pal.title);
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

#[cfg(test)]
mod tests {
    use super::*;

    fn origin(r: Rectangle) -> (i32, i32, u32, u32) {
        (r.top_left.x, r.top_left.y, r.size.width, r.size.height)
    }

    #[test]
    fn default_portrait_matches_the_shipped_layout() {
        let l = Layout::for_rotation(0);
        assert_eq!(origin(l.art), (20, 8, 200, 200));
        assert_eq!(origin(l.text), (4, 214, 232, 48));
        assert_eq!(origin(l.volume), (10, 268, 220, 6));
        assert_eq!(origin(l.progress), (10, 276, 220, 12));
        assert_eq!(origin(l.transport), (0, 292, 240, 28));
        assert_eq!(l.status_text, StatusText::Normal);
        assert_eq!(l.strip, Strip::Progress);
        assert_eq!(l.theme, Theme::Ink);
    }

    #[test]
    fn default_landscape_matches_the_shipped_layout() {
        let l = Layout::for_rotation(270);
        assert_eq!(origin(l.art), (10, 4, 168, 168));
        assert_eq!(origin(l.text), (184, 4, 132, 168));
        assert_eq!(origin(l.volume), (10, 178, 300, 6));
        assert_eq!(origin(l.progress), (10, 186, 300, 12));
        assert_eq!(origin(l.transport), (0, 200, 320, 40));
    }

    #[test]
    fn tight_keeps_art_and_closes_the_bar_gap() {
        let p = Layout::compose(
            0,
            BarGap::Tight,
            StatusText::Normal,
            Strip::Progress,
            Theme::Ink,
        );
        assert_eq!(origin(p.art), (20, 8, 200, 200));
        assert_eq!(origin(p.volume), (10, 270, 220, 6));
        assert_eq!(origin(p.progress), (10, 276, 220, 12));
        assert_eq!(origin(p.transport), (0, 292, 240, 28));
        let l = Layout::compose(
            90,
            BarGap::Tight,
            StatusText::Normal,
            Strip::Progress,
            Theme::Ink,
        );
        assert_eq!(origin(l.art), (10, 4, 168, 168));
        assert_eq!(origin(l.volume), (10, 178, 300, 6));
        assert_eq!(origin(l.progress), (10, 184, 300, 12));
        assert_eq!(origin(l.transport), (0, 200, 320, 40));
    }

    #[test]
    fn transport_does_not_move_when_the_gap_opens() {
        for gap in [BarGap::Tight, BarGap::Default, BarGap::Roomy] {
            for strip in [Strip::Progress, Strip::Stream, Strip::Off] {
                let p = Layout::compose(0, gap, StatusText::Normal, strip, Theme::Ink);
                assert_eq!(origin(p.transport), (0, 292, 240, 28));
                let l = Layout::compose(90, gap, StatusText::Normal, strip, Theme::Ink);
                assert_eq!(origin(l.transport), (0, 200, 320, 40));
            }
        }
    }

    #[test]
    fn roomy_portrait_opens_the_bar_gap_from_art() {
        let l = Layout::compose(
            0,
            BarGap::Roomy,
            StatusText::Normal,
            Strip::Progress,
            Theme::Ink,
        );
        assert_eq!(origin(l.volume), (10, 260, 220, 6));
        assert_eq!(origin(l.progress), (10, 276, 220, 12));
        assert_eq!(l.art.size.height, 196);
        let between = l.progress.top_left.y - (l.volume.top_left.y + l.volume.size.height as i32);
        assert_eq!(between, 10);
    }

    #[test]
    fn roomy_landscape_steals_from_art_not_transport() {
        let l = Layout::compose(
            90,
            BarGap::Roomy,
            StatusText::Normal,
            Strip::Progress,
            Theme::Ink,
        );
        assert_eq!(origin(l.art), (10, 4, 156, 156));
        assert_eq!(origin(l.volume), (10, 168, 300, 6));
        assert_eq!(origin(l.progress), (10, 184, 300, 12));
        assert_eq!(origin(l.transport), (0, 200, 320, 40));
        let between = l.progress.top_left.y - (l.volume.top_left.y + l.volume.size.height as i32);
        assert_eq!(between, 10);
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
        assert_eq!(l.art.size.width, 156);
        assert_eq!(l.strip, Strip::Stream);
        assert_eq!(l.theme, Theme::Studio);
        assert_eq!(origin(l.progress), (10, 184, 300, 12));
    }

    #[test]
    fn stream_grows_the_slot_without_moving_neighbours() {
        let p = Layout::compose(
            0,
            BarGap::Default,
            StatusText::Normal,
            Strip::Stream,
            Theme::Ink,
        );
        assert_eq!(origin(p.art), (20, 8, 200, 200));
        assert_eq!(origin(p.volume), (10, 268, 220, 6));
        assert_eq!(origin(p.progress), (10, 276, 220, 12));
        assert_eq!(origin(p.transport), (0, 292, 240, 28));
        let l = Layout::compose(
            90,
            BarGap::Default,
            StatusText::Normal,
            Strip::Stream,
            Theme::Ink,
        );
        assert_eq!(origin(l.art), (10, 4, 168, 168));
        assert_eq!(origin(l.volume), (10, 178, 300, 6));
        assert_eq!(origin(l.progress), (10, 186, 300, 12));
        assert_eq!(origin(l.transport), (0, 200, 320, 40));
    }

    #[test]
    fn off_keeps_the_progress_rect() {
        let p = Layout::compose(
            0,
            BarGap::Default,
            StatusText::Normal,
            Strip::Off,
            Theme::Ink,
        );
        assert_eq!(origin(p.progress), (10, 280, 220, 4));
        assert_eq!(origin(p.transport), (0, 292, 240, 28));
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
    fn volume_track_starts_after_the_speaker() {
        let l = Layout::for_rotation(270);
        let track = volume_track(l.volume);
        assert_eq!(track.top_left.x, l.volume.top_left.x + 14);
        assert_eq!(track.size.height, 6);
        assert_eq!(track.size.width, 286);
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
    fn dusk_and_studio_are_named_swaps() {
        let ink = palette(Theme::Ink);
        let dusk = palette(Theme::Dusk);
        let studio = palette(Theme::Studio);
        assert_ne!(dusk.bg, ink.bg);
        assert_ne!(dusk.title, ink.title);
        assert_ne!(dusk.accent, ink.accent);
        assert_ne!(studio.bg, ink.bg);
        assert_ne!(studio.title, ink.title);
        assert_ne!(studio.accent, ink.accent);
        assert_ne!(dusk.accent, studio.accent);
        assert_ne!(dusk.bg, studio.bg);
        assert_eq!(dusk.bg, rgb(80, 36, 12));
        assert_eq!(dusk.accent, rgb(255, 200, 48));
        assert_eq!(studio.bg, rgb(12, 28, 72));
        assert_eq!(studio.accent, rgb(48, 160, 255));
    }
}
