//! Album art: fetch, decode, scale, on a background thread.
//!
//! Off the main loop because both halves are slow. A cover can be several
//! hundred kilobytes, and decoding a 1000x1000 PNG on a Pi 3A+ takes long
//! enough to stall the ticker and delay a touch. The main loop asks for a URL
//! and carries on; the picture arrives when it arrives.
//!
//! Only one request is in flight at a time. Skipping through a playlist
//! generates a URL per track, and the useful behaviour is to render the one
//! the user settled on rather than to queue and decode every cover they passed
//! through.

use anyhow::{Context, Result};
use embedded_graphics::pixelcolor::Rgb565;
use image::imageops::FilterType;
use std::sync::mpsc::{Receiver, Sender, TryRecvError};
use std::thread;
use std::time::Duration;

use crate::http;

/// A decoded cover, scaled to fit and converted to the panel's colour format.
///
/// Converting to Rgb565 here rather than at draw time means the main loop only
/// copies pixels.
pub struct Art {
    /// Width in pixels.
    pub w: u32,
    /// Height in pixels.
    pub h: u32,
    /// Row-major pixels, `w * h` of them.
    pub px: Vec<Rgb565>,
}

/// Handle to the art thread.
pub struct ArtLoader {
    tx: Sender<String>,
    rx: Receiver<Option<Art>>,
    /// The URL most recently asked for, to avoid refetching the same cover
    /// every time the scene is redrawn.
    pending: Option<String>,
}

impl ArtLoader {
    /// Start the thread.
    ///
    /// `base` is the origin the relative `albumart` path is joined to, and
    /// `size` the box the picture is scaled to fit.
    pub fn spawn(base: String, size: u32, timeout: Duration) -> Result<Self> {
        let (req_tx, req_rx) = std::sync::mpsc::channel::<String>();
        let (res_tx, res_rx) = std::sync::mpsc::channel::<Option<Art>>();

        thread::Builder::new()
            .name("art".into())
            .spawn(move || worker(base, size, timeout, req_rx, res_tx))
            .context("spawning art thread")?;

        Ok(Self {
            tx: req_tx,
            rx: res_rx,
            pending: None,
        })
    }

    /// Ask for a cover, unless it is the one already requested.
    pub fn request(&mut self, path: &str) {
        if self.pending.as_deref() == Some(path) {
            return;
        }
        self.pending = Some(path.to_string());
        let _ = self.tx.send(path.to_string());
    }

    /// Collect a decoded cover if one is ready. Never blocks.
    ///
    /// `Some(None)` means the fetch or decode failed and the caller should
    /// clear whatever it was showing, rather than leaving the previous track's
    /// cover under the new track's title.
    pub fn poll(&self) -> Option<Option<Art>> {
        match self.rx.try_recv() {
            Ok(art) => Some(art),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => None,
        }
    }
}

fn worker(
    base: String,
    size: u32,
    timeout: Duration,
    rx: Receiver<String>,
    tx: Sender<Option<Art>>,
) {
    while let Ok(mut path) = rx.recv() {
        // Drain anything queued behind this one. Skipping through a playlist
        // should render where the user stopped, not every cover on the way.
        while let Ok(newer) = rx.try_recv() {
            path = newer;
        }

        let url = join(&base, &path);
        let art = match fetch(&url, size, timeout) {
            Ok(a) => Some(a),
            Err(e) => {
                tracing::debug!(error = %e, url = %url, "album art");
                None
            }
        };

        if tx.send(art).is_err() {
            return;
        }
    }
}

/// Join an origin and a path from the state API, which may be either relative
/// or already absolute.
fn join(base: &str, path: &str) -> String {
    if path.starts_with("http://") || path.starts_with("https://") {
        return path.to_string();
    }
    format!(
        "{}/{}",
        base.trim_end_matches('/'),
        path.trim_start_matches('/')
    )
}

fn fetch(url: &str, size: u32, timeout: Duration) -> Result<Art> {
    let bytes = http::get_bytes(url, timeout)?;

    // Dispatch on the magic bytes rather than on Content-Type. Volumio proxies
    // art for streams, so the declared type is whatever the upstream server
    // said, and that is not always what the bytes are.
    let format = image::guess_format(&bytes).context("unrecognised image format")?;
    let img = image::load_from_memory_with_format(&bytes, format).context("decoding")?;

    // Fit inside the box preserving aspect: a stretched cover looks worse than
    // a smaller one. Triangle rather than nearest because covers are
    // photographic and nearest produces visible aliasing on a 200 px box.
    let img = img.resize(size, size, FilterType::Triangle).to_rgb8();
    let (w, h) = img.dimensions();

    let px = img
        .pixels()
        .map(|p| Rgb565::new(p[0] >> 3, p[1] >> 2, p[2] >> 3))
        .collect();

    Ok(Art { w, h, px })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn joins_relative_path() {
        assert_eq!(
            join("http://localhost:3000", "/albumart?x=1"),
            "http://localhost:3000/albumart?x=1"
        );
    }

    #[test]
    fn tolerates_trailing_and_leading_slashes() {
        assert_eq!(join("http://h/", "/a"), "http://h/a");
        assert_eq!(join("http://h", "a"), "http://h/a");
    }

    #[test]
    fn passes_absolute_urls_through() {
        assert_eq!(join("http://localhost", "http://x/y.jpg"), "http://x/y.jpg");
    }
}
