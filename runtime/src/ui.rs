//! Screen layout, drawing, and hit regions.
//!
//! Panel-native frame is 240 wide by 320 tall. All coordinates here are in
//! that frame. If a display rotation is applied at init, the touch
//! coordinates still arrive unrotated, so the mapping in [`hit`] must apply
//! the same transform the renderer does. Keeping both in the native frame
//! avoids the problem entirely, which is why nothing here rotates.

use embedded_graphics::{
    mono_font::{ascii::FONT_6X10, ascii::FONT_9X15_BOLD, MonoTextStyle},
    pixelcolor::Rgb565,
    prelude::*,
    primitives::{PrimitiveStyle, Rectangle},
    text::{Alignment, Text},
};

use crate::state::PlayerState;
use crate::touch::Touch;

/// Panel width in the native frame.
pub const WIDTH: u32 = 240;
/// Panel height in the native frame.
pub const HEIGHT: u32 = 320;

/// Height of the art area at the top.
const ART_H: u32 = 240;
/// Top of the transport strip.
const TRANSPORT_Y: u32 = 280;

/// A user action derived from a touch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    /// Previous track.
    Prev,
    /// Toggle play and pause.
    PlayPause,
    /// Next track.
    Next,
    /// Tap on the art area, currently unassigned.
    Art,
}

/// Map a touch to an action, or `None` if it landed on nothing.
///
/// Touch coordinates arrive as `u16` from the controller; the layout constants
/// are `u32` because embedded-graphics `Size` requires it. Widening once here
/// keeps the comparisons honest rather than scattering casts through them.
///
/// The transport strip is split into equal thirds. There is deliberately no
/// dead band between them: at 80 pixels per control on a 2.8 inch panel the
/// targets are already far larger than a fingertip, and a gap would only
/// create places where a deliberate press does nothing.
pub fn hit(t: Touch) -> Option<Action> {
    let (x, y) = (u32::from(t.x), u32::from(t.y));

    if y < ART_H {
        return Some(Action::Art);
    }
    if y < TRANSPORT_Y {
        return None;
    }
    match x {
        x if x < WIDTH / 3 => Some(Action::Prev),
        x if x < 2 * WIDTH / 3 => Some(Action::PlayPause),
        _ => Some(Action::Next),
    }
}

/// Draw the whole screen.
///
/// Redraws everything each frame. At 32 MHz a full 240x320 RGB565 frame is
/// about 39 ms, so a half-second poll interval leaves plenty of headroom and
/// partial redraw is not worth the complexity yet. Revisit if the poll
/// interval drops or a progress bar needs to move smoothly.
pub fn draw<D>(target: &mut D, state: &PlayerState) -> Result<(), D::Error>
where
    D: DrawTarget<Color = Rgb565>,
{
    target.clear(Rgb565::BLACK)?;

    let title_style = MonoTextStyle::new(&FONT_9X15_BOLD, Rgb565::WHITE);
    let meta_style = MonoTextStyle::new(&FONT_6X10, Rgb565::CSS_LIGHT_GRAY);

    // TODO: album art. Needs a JPEG decoder on device; zune-jpeg is the light
    // option. Until then the art area carries the text block.
    let centre = WIDTH as i32 / 2;

    Text::with_alignment(
        state.title.as_deref().unwrap_or(""),
        Point::new(centre, 120),
        title_style,
        Alignment::Center,
    )
    .draw(target)?;

    Text::with_alignment(
        state.artist.as_deref().unwrap_or(""),
        Point::new(centre, 145),
        meta_style,
        Alignment::Center,
    )
    .draw(target)?;

    Text::with_alignment(
        state.album.as_deref().unwrap_or(""),
        Point::new(centre, 160),
        meta_style,
        Alignment::Center,
    )
    .draw(target)?;

    // Format strip: sample rate and bit depth, the thing a person actually
    // wants confirmed at a glance on a music player.
    let format = match (state.samplerate.as_deref(), state.bitdepth.as_deref()) {
        (Some(sr), Some(bd)) => format!("{sr}  {bd}"),
        (Some(sr), None) => sr.to_string(),
        (None, Some(bd)) => bd.to_string(),
        (None, None) => String::new(),
    };
    Text::with_alignment(
        &format,
        Point::new(centre, 200),
        meta_style,
        Alignment::Center,
    )
    .draw(target)?;

    // Progress bar. The seek/duration unit mismatch is handled in
    // PlayerState::progress, not here.
    if let Some(frac) = state.progress() {
        let filled = (WIDTH as f32 * frac) as u32;

        Rectangle::new(Point::new(0, 255), Size::new(WIDTH, 4))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::CSS_DIM_GRAY))
            .draw(target)?;
        Rectangle::new(Point::new(0, 255), Size::new(filled, 4))
            .into_styled(PrimitiveStyle::with_fill(Rgb565::WHITE))
            .draw(target)?;
    }

    // Transport labels. Drawn as text rather than glyphs so there is no font
    // asset to ship; swap for icons once there is a reason to.
    let third = WIDTH as i32 / 3;
    let label_y = TRANSPORT_Y as i32 + 20;
    let play_label = if state.is_playing() { "||" } else { ">" };

    for (i, label) in ["|<", play_label, ">|"].iter().enumerate() {
        Text::with_alignment(
            label,
            Point::new(third * i as i32 + third / 2, label_y),
            title_style,
            Alignment::Center,
        )
        .draw(target)?;
    }

    Ok(())
}
