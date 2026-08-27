//! Volumio playback state.
//!
//! Polls the REST endpoint rather than holding a socket.io connection. The
//! trade is latency against dependency weight: socket.io in Rust means either
//! a heavy client or hand-rolling the protocol, and a half-second poll is
//! indistinguishable from push on a display a person glances at.
//!
//! If this ever needs to be push-driven, MPD's idle command over a plain TCP
//! socket is the lighter route, not socket.io.

use serde::Deserialize;
use std::time::Duration;

use crate::http;

/// The subset of Volumio's state we render.
///
/// Field names match the API. Everything is optional because the endpoint
/// omits fields rather than nulling them, and a missing title should show an
/// empty line rather than fail the poll.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
pub struct PlayerState {
    /// play, pause, stop.
    pub status: Option<String>,
    /// Track title.
    pub title: Option<String>,
    /// Artist name.
    pub artist: Option<String>,
    /// Album name.
    pub album: Option<String>,
    /// Album art path, relative to the Volumio host or absolute.
    #[serde(rename = "albumart")]
    pub album_art: Option<String>,
    /// Elapsed position. Volumio reports this in milliseconds.
    pub seek: Option<u64>,
    /// Track length in seconds.
    pub duration: Option<u64>,
    /// Sample rate as a display string, e.g. "44.1 kHz".
    pub samplerate: Option<String>,
    /// Bit depth as a display string, e.g. "16 bit".
    pub bitdepth: Option<String>,
    /// Output volume, 0 to 100.
    pub volume: Option<u8>,
    /// Mute state.
    pub mute: Option<bool>,
}

impl PlayerState {
    /// True when the player is actively playing.
    pub fn is_playing(&self) -> bool {
        self.status.as_deref() == Some("play")
    }

    /// True when output is muted.
    pub fn is_muted(&self) -> bool {
        self.mute.unwrap_or(false)
    }

    /// Elapsed fraction of the track, 0.0 to 1.0, if both fields are present.
    ///
    /// Note the unit mismatch in the API: `seek` is milliseconds and
    /// `duration` is seconds. Getting this wrong makes the progress bar jump
    /// to full within the first second, which is exactly how the bug presents.
    pub fn progress(&self) -> Option<f32> {
        let (seek_ms, dur_s) = (self.seek?, self.duration?);
        if dur_s == 0 {
            return None;
        }
        let dur_ms = dur_s.saturating_mul(1000);
        Some((seek_ms.min(dur_ms) as f32) / (dur_ms as f32))
    }

    /// True when two states describe the same screen apart from the parts
    /// that are repainted individually.
    ///
    /// `seek` advances every second while playing and volume can change at any
    /// time, so comparing whole states makes them differ on nearly every poll.
    /// Clearing and repainting the panel that often is visible as a flicker.
    /// Everything here changes only when the track or transport state does, so
    /// this is what gates a full redraw; progress and volume are repainted in
    /// place.
    pub fn same_scene(&self, other: &Self) -> bool {
        self.status == other.status
            && self.title == other.title
            && self.artist == other.artist
            && self.album == other.album
            && self.duration == other.duration
            && self.samplerate == other.samplerate
            && self.bitdepth == other.bitdepth
    }
}

/// Polling client for the Volumio state endpoint.
pub struct StateSource {
    url: String,
    timeout: Duration,
}

impl StateSource {
    /// Build a client. The timeout should be shorter than the poll interval so
    /// a stalled request cannot queue up behind the next one.
    pub fn new(url: impl Into<String>, timeout: Duration) -> Self {
        Self {
            url: url.into(),
            timeout,
        }
    }

    /// Fetch current state.
    ///
    /// A failed poll is not fatal. The caller should keep displaying the last
    /// good state rather than blanking the panel, because a transient failure
    /// during a Volumio restart is expected and a flickering display is worse
    /// than a slightly stale one.
    pub fn poll(&self) -> anyhow::Result<PlayerState> {
        let body = http::get(&self.url, self.timeout)?;
        let state: PlayerState = serde_json::from_str(&body)?;
        Ok(state)
    }
}

/// Playback commands the panel can issue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// Previous track.
    Prev,
    /// Toggle between play and pause.
    Toggle,
    /// Next track.
    Next,
    /// Set volume to a percentage.
    Volume(u8),
}

impl Command {
    /// The query string Volumio expects, after the base URL.
    fn query(self) -> String {
        match self {
            Command::Prev => "cmd=prev".into(),
            Command::Toggle => "cmd=toggle".into(),
            Command::Next => "cmd=next".into(),
            Command::Volume(v) => format!("cmd=volume&volume={}", v.min(100)),
        }
    }
}

/// Command client for the Volumio control endpoint.
pub struct CommandSink {
    base: String,
    timeout: Duration,
}

impl CommandSink {
    /// Build a client against the command base URL.
    pub fn new(base: impl Into<String>, timeout: Duration) -> Self {
        Self {
            base: base.into(),
            timeout,
        }
    }

    /// Send a command. The response body is discarded; only success matters.
    pub fn send(&self, cmd: Command) -> anyhow::Result<()> {
        let url = format!("{}?{}", self.base, cmd.query());
        http::get(&url, self.timeout)?;
        Ok(())
    }
}
