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

/// Turn an `albumart` value from the state API into a URL to fetch.
///
/// Relative paths join to the origin. Absolute ones are fetched directly.
///
/// Volumio's `/albumart?url=` looked like a proxy for remote art and is not:
/// it returns the default artwork regardless of the `url` parameter, encoded
/// or not, and returns JPEG even when the source is PNG. The browser UI shows
/// station logos because the browser loads the absolute URL itself. Since
/// stream art is served over https, that is why this binary carries TLS.
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
    decode(&bytes, size)
}

/// Turn fetched bytes into a scaled cover.
///
/// Raster formats are dispatched on the magic bytes rather than on
/// Content-Type: art is served by whatever server the stream points at, and
/// the declared type is not always what the bytes are. SVG is tried only
/// after that, since it has no magic number worth trusting.
fn decode(bytes: &[u8], size: u32) -> Result<Art> {
    match image::guess_format(bytes) {
        Ok(format) => decode_raster(bytes, format, size),
        Err(e) => {
            if looks_like_svg(bytes) {
                decode_svg(bytes, size)
            } else {
                Err(anyhow::Error::new(e).context("unrecognised image format"))
            }
        }
    }
}

/// True if the bytes plausibly start an SVG document.
///
/// Checked rather than assumed, so a truncated download or an HTML error page
/// is reported as an unrecognised format instead of being handed to the XML
/// parser to fail obscurely. Gzipped SVG (`.svgz`) starts with the gzip magic
/// and is handled by usvg itself.
fn looks_like_svg(bytes: &[u8]) -> bool {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        return true;
    }
    let head = &bytes[..bytes.len().min(512)];
    let text = String::from_utf8_lossy(head);
    let text = text.trim_start();
    text.starts_with("<?xml") || text.starts_with("<svg") || text.contains("<svg")
}

fn decode_raster(bytes: &[u8], format: image::ImageFormat, size: u32) -> Result<Art> {
    let img = image::load_from_memory_with_format(bytes, format).context("decoding")?;

    // Fit inside the box preserving aspect: a stretched cover looks worse than
    // a smaller one. Triangle rather than nearest because covers are
    // photographic and nearest produces visible aliasing at this size.
    let img = img.resize(size, size, FilterType::Triangle).to_rgba8();
    let (w, h) = img.dimensions();

    // Composite over black rather than discarding alpha. Station logos are
    // routinely transparent PNGs drawn for a light background; dropping the
    // alpha channel leaves whatever colour happens to sit under it, which is
    // often black on black.
    let px = img.pixels().map(|p| blend_on_black(p.0)).collect();

    Ok(Art { w, h, px })
}

fn decode_svg(bytes: &[u8], size: u32) -> Result<Art> {
    let opt = resvg::usvg::Options::default();
    let tree = resvg::usvg::Tree::from_data(bytes, &opt).context("parsing svg")?;

    let svg_size = tree.size();
    if svg_size.width() <= 0.0 || svg_size.height() <= 0.0 {
        anyhow::bail!("svg has no size");
    }

    // Rasterise straight to the target size. Unlike a bitmap there is no
    // resampling loss: the vector is drawn at the size it will be shown.
    let scale = (size as f32 / svg_size.width()).min(size as f32 / svg_size.height());
    let w = (svg_size.width() * scale).round().max(1.0) as u32;
    let h = (svg_size.height() * scale).round().max(1.0) as u32;

    let mut pixmap = resvg::tiny_skia::Pixmap::new(w, h).context("allocating svg raster target")?;
    resvg::render(
        &tree,
        resvg::tiny_skia::Transform::from_scale(scale, scale),
        &mut pixmap.as_mut(),
    );

    // Pixmap data is premultiplied RGBA, so compositing over black is just
    // taking the colour channels as they stand.
    let px = pixmap
        .data()
        .chunks_exact(4)
        .map(|c| Rgb565::new(c[0] >> 3, c[1] >> 2, c[2] >> 3))
        .collect();

    Ok(Art { w, h, px })
}

/// Composite a straight-alpha RGBA pixel over black.
fn blend_on_black(p: [u8; 4]) -> Rgb565 {
    let a = u32::from(p[3]);
    let ch = |v: u8| ((u32::from(v) * a) / 255) as u8;
    Rgb565::new(ch(p[0]) >> 3, ch(p[1]) >> 2, ch(p[2]) >> 3)
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
        assert_eq!(
            join("http://localhost:3000", "https://cdn.example/logo.png"),
            "https://cdn.example/logo.png"
        );
    }

    #[test]
    fn recognises_svg() {
        assert!(looks_like_svg(
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"/>"
        ));
        assert!(looks_like_svg(b"  \n<?xml version=\"1.0\"?><svg/>"));
        assert!(looks_like_svg(&[0x1f, 0x8b, 0x08, 0x00]));
    }

    #[test]
    fn rejects_non_svg_text() {
        assert!(!looks_like_svg(b"<html><body>404</body></html>"));
        assert!(!looks_like_svg(b"not markup at all"));
    }

    #[test]
    fn blends_alpha_over_black() {
        // Expected values built with `new` rather than the RgbColor trait
        // constants: this asserts the actual 5-6-5 encoding the function
        // produces, not that two named constants happen to agree.
        assert_eq!(
            blend_on_black([255, 255, 255, 255]),
            Rgb565::new(31, 63, 31)
        );
        assert_eq!(blend_on_black([255, 255, 255, 0]), Rgb565::new(0, 0, 0));
        // Half alpha halves each channel before the bit-depth reduction.
        assert_eq!(
            blend_on_black([255, 255, 255, 128]),
            Rgb565::new(16, 32, 16)
        );
    }
}
