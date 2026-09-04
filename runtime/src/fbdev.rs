//! Framebuffer drawing surface.
//!
//! Used when fbtft already owns spi0 cs0, which is the Plymouth path: the
//! overlay is firmware-applied, `/dev/spidev0.0` does not exist, and this
//! process must not claim the DC, reset or backlight lines.
//!
//! fbtft deferred IO only flushes pages dirtied via mmap. A `write` on the
//! device node updates `screen_base` without marking those pages, so the
//! panel never changes.

use anyhow::{anyhow, Context, Result};
use embedded_graphics::pixelcolor::Rgb565;
use embedded_graphics::prelude::*;
use embedded_graphics::primitives::Rectangle;
use embedded_graphics::Pixel;
use memmap2::MmapMut;
use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};

use crate::config::fbtft_rotate;
use crate::ui::Layout;

/// Sysfs `name` of the fbtft ST7789V node. HDMI and bcm2708_fb/KMS
/// register other names on other `/dev/fbN` indices; those are not this panel.
pub const PANEL_FB_NAME: &str = "fb_st7789v";

/// mmap'd 16-bit RGB565 framebuffer.
pub struct FbDev {
    map: MmapMut,
    width: u32,
    height: u32,
    stride: usize,
    /// Held so the mapping stays associated with a live fd.
    _file: File,
}

impl FbDev {
    /// Open `path`, check it matches `layout`, and mmap it.
    #[allow(unsafe_code)]
    pub fn open(path: &str, layout: &Layout) -> Result<Self> {
        let sys = graphics_sysfs(path);
        let name = read_fb_name(&sys).with_context(|| format!("reading {path} name"))?;
        if name != PANEL_FB_NAME {
            anyhow::bail!(
                "{path} is {name}, not {PANEL_FB_NAME}. \
                 bcm2708_fb/KMS generic framebuffers are not this panel."
            );
        }
        let (width, height) = read_virtual_size(&sys)?;
        let bits = read_sysfs_u32(&sys.join("bits_per_pixel"), "bits_per_pixel")?;
        if bits != 16 {
            anyhow::bail!("{path} is {bits} bpp; only 16-bit RGB565 (fbtft st7789v) is supported");
        }

        let stride = read_sysfs_u32(&sys.join("stride"), "stride")? as usize;
        if stride < width as usize * 2 {
            anyhow::bail!(
                "{path} stride {stride} is smaller than {width}x16 bpp ({} bytes)",
                width as usize * 2
            );
        }

        let expected = layout.frame.size;
        if width != expected.width || height != expected.height {
            anyhow::bail!(
                "{path} is {width}x{height}, layout for rotation={} is {}x{}. \
                 fbtft rotate is counter-clockwise; this crate's rotation is clockwise. \
                 rotation={} needs dtparam=rotate={}.",
                layout.rotation,
                expected.width,
                expected.height,
                layout.rotation,
                fbtft_rotate(layout.rotation)
            );
        }

        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .with_context(|| {
                format!(
                    "opening {path}. The volumio user needs the video group, and \
                     an fbtft overlay must have created this device before Plymouth started."
                )
            })?;

        let map_len = stride * height as usize;
        // SAFETY: `file` is opened read-write on a character device. `map_len`
        // is stride × height from the kernel's own sysfs for this fb, not a
        // guess. The mapping is dropped before `_file`.
        let map = unsafe { memmap2::MmapOptions::new().len(map_len).map_mut(&file) }
            .with_context(|| format!("mmap {path} ({map_len} bytes)"))?;

        Ok(Self {
            map,
            width,
            height,
            stride,
            _file: file,
        })
    }

    fn put(&mut self, x: u32, y: u32, color: Rgb565) {
        let off = y as usize * self.stride + x as usize * 2;
        if let Some(dst) = self.map.get_mut(off..off + 2) {
            dst.copy_from_slice(&color.into_storage().to_le_bytes());
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        self.map.flush()
    }
}

impl OriginDimensions for FbDev {
    fn size(&self) -> Size {
        Size::new(self.width, self.height)
    }
}

impl DrawTarget for FbDev {
    type Color = Rgb565;
    type Error = anyhow::Error;

    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Pixel<Self::Color>>,
    {
        let (w, h) = (self.width as i32, self.height as i32);
        for Pixel(p, c) in pixels {
            if p.x >= 0 && p.y >= 0 && p.x < w && p.y < h {
                self.put(p.x as u32, p.y as u32, c);
            }
        }
        self.flush().context("flushing framebuffer")?;
        Ok(())
    }

    fn fill_contiguous<I>(&mut self, area: &Rectangle, colors: I) -> Result<(), Self::Error>
    where
        I: IntoIterator<Item = Self::Color>,
    {
        let clipped = area.intersection(&self.bounding_box());
        if clipped.size == Size::zero() {
            return Ok(());
        }
        // A clipped area would consume the iterator out of step with the
        // rectangle the caller described. The UI draws inside the frame; if
        // something does not, fall back to per-pixel writes.
        if clipped != *area {
            return self.draw_iter(area.points().zip(colors).map(|(p, c)| Pixel(p, c)));
        }

        let x0 = area.top_left.x as u32;
        let y0 = area.top_left.y as u32;
        let w = area.size.width as usize;
        let mut colors = colors.into_iter();

        for row in 0..area.size.height {
            let off = (y0 + row) as usize * self.stride + x0 as usize * 2;
            let dst = self
                .map
                .get_mut(off..off + w * 2)
                .ok_or_else(|| anyhow!("framebuffer write out of range at y={}", y0 + row))?;
            for chunk in dst.chunks_exact_mut(2) {
                let c = colors
                    .next()
                    .ok_or_else(|| anyhow!("fill_contiguous ran out of pixels"))?;
                chunk.copy_from_slice(&c.into_storage().to_le_bytes());
            }
        }
        self.flush().context("flushing framebuffer")?;
        Ok(())
    }

    fn fill_solid(&mut self, area: &Rectangle, color: Self::Color) -> Result<(), Self::Error> {
        let area = area.intersection(&self.bounding_box());
        if area.size == Size::zero() {
            return Ok(());
        }
        let pix = color.into_storage().to_le_bytes();
        let x0 = area.top_left.x as u32;
        let y0 = area.top_left.y as u32;
        let w = area.size.width as usize;
        for row in 0..area.size.height {
            let off = (y0 + row) as usize * self.stride + x0 as usize * 2;
            let dst = self
                .map
                .get_mut(off..off + w * 2)
                .ok_or_else(|| anyhow!("framebuffer write out of range at y={}", y0 + row))?;
            for chunk in dst.chunks_exact_mut(2) {
                chunk.copy_from_slice(&pix);
            }
        }
        self.flush().context("flushing framebuffer")?;
        Ok(())
    }
}

/// Prefer `preferred` when that node is `fb_st7789v`. Otherwise scan
/// `/sys/class/graphics` for that name. Never fall through to a KMS node.
pub fn resolve_fb_dev(preferred: &str) -> Result<String> {
    let found = resolve_fb_dev_in(Path::new("/sys/class/graphics"), preferred)?;
    if found != preferred {
        tracing::info!(
            preferred,
            found,
            "using live {PANEL_FB_NAME} instead of configured fb_dev"
        );
    }
    Ok(found)
}

fn resolve_fb_dev_in(sys_graphics: &Path, preferred: &str) -> Result<String> {
    if fb_name_in(sys_graphics, preferred).as_deref() == Some(PANEL_FB_NAME) {
        return Ok(preferred.to_string());
    }
    if let Some(found) = find_named_fb(sys_graphics, PANEL_FB_NAME) {
        return Ok(found);
    }
    match fb_name_in(sys_graphics, preferred) {
        Some(name) => anyhow::bail!(
            "{preferred} is {name}, not {PANEL_FB_NAME}. No panel framebuffer found. \
             bcm2708_fb/KMS generic framebuffers are not this panel."
        ),
        None => anyhow::bail!("no {PANEL_FB_NAME} framebuffer found (looked at {preferred})"),
    }
}

fn graphics_sysfs(dev: &str) -> PathBuf {
    PathBuf::from("/sys/class/graphics").join(Path::new(dev).file_name().unwrap_or_default())
}

fn fb_name_in(sys_graphics: &Path, dev: &str) -> Option<String> {
    let node = sys_graphics.join(Path::new(dev).file_name().unwrap_or_default());
    read_fb_name(&node).ok()
}

fn find_named_fb(sys_graphics: &Path, want: &str) -> Option<String> {
    let mut matches = Vec::new();
    for ent in fs::read_dir(sys_graphics).ok()? {
        let ent = ent.ok()?;
        let fname = ent.file_name();
        let fname = fname.to_string_lossy();
        let Some(idx) = fname.strip_prefix("fb") else {
            continue;
        };
        let Ok(n) = idx.parse::<u32>() else {
            continue;
        };
        let name = fs::read_to_string(ent.path().join("name")).ok()?;
        if name.trim() == want {
            matches.push((n, format!("/dev/{fname}")));
        }
    }
    matches.sort_by_key(|(n, _)| *n);
    matches.into_iter().next().map(|(_, p)| p)
}

fn read_fb_name(sys: &Path) -> Result<String> {
    let text = fs::read_to_string(sys.join("name"))
        .with_context(|| format!("reading {}/name", sys.display()))?;
    let name = text.trim();
    if name.is_empty() {
        anyhow::bail!("empty framebuffer name in {}/name", sys.display());
    }
    Ok(name.to_string())
}

fn read_virtual_size(sys: &Path) -> Result<(u32, u32)> {
    let text = fs::read_to_string(sys.join("virtual_size"))
        .with_context(|| format!("reading {}/virtual_size", sys.display()))?;
    parse_virtual_size(&text)
        .ok_or_else(|| anyhow!("unrecognised virtual_size {:?}: expected W,H", text.trim()))
}

fn parse_virtual_size(text: &str) -> Option<(u32, u32)> {
    let (w, h) = text.trim().split_once(',')?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn read_sysfs_u32(path: &Path, what: &str) -> Result<u32> {
    let text = fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    text.trim()
        .parse()
        .map_err(|_| anyhow!("unrecognised {what} {:?}", text.trim()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_virtual_size_accepts_the_sysfs_form() {
        assert_eq!(parse_virtual_size("320,240\n"), Some((320, 240)));
        assert_eq!(parse_virtual_size("240,320"), Some((240, 320)));
        assert_eq!(parse_virtual_size("nope"), None);
    }

    fn write_name(root: &Path, fb: &str, name: &str) {
        let dir = root.join(fb);
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("name"), format!("{name}\n")).unwrap();
    }

    #[test]
    fn resolve_keeps_preferred_when_it_is_the_panel() {
        let tmp = tempfile::tempdir().unwrap();
        write_name(tmp.path(), "fb0", "BCM2708 FB");
        write_name(tmp.path(), "fb1", PANEL_FB_NAME);
        assert_eq!(
            resolve_fb_dev_in(tmp.path(), "/dev/fb1").unwrap(),
            "/dev/fb1"
        );
    }

    #[test]
    fn resolve_skips_kms_and_finds_the_panel() {
        let tmp = tempfile::tempdir().unwrap();
        write_name(tmp.path(), "fb0", "BCM2708 FB");
        write_name(tmp.path(), "fb1", PANEL_FB_NAME);
        assert_eq!(
            resolve_fb_dev_in(tmp.path(), "/dev/fb0").unwrap(),
            "/dev/fb1"
        );
    }

    #[test]
    fn resolve_finds_the_panel_on_fb0() {
        let tmp = tempfile::tempdir().unwrap();
        write_name(tmp.path(), "fb0", PANEL_FB_NAME);
        assert_eq!(
            resolve_fb_dev_in(tmp.path(), "/dev/fb1").unwrap(),
            "/dev/fb0"
        );
    }

    #[test]
    fn resolve_refuses_kms_when_the_panel_is_absent() {
        let tmp = tempfile::tempdir().unwrap();
        write_name(tmp.path(), "fb0", "BCM2708 FB");
        let err = resolve_fb_dev_in(tmp.path(), "/dev/fb0")
            .unwrap_err()
            .to_string();
        assert!(err.contains("BCM2708 FB"), "{err}");
        assert!(err.contains(PANEL_FB_NAME), "{err}");
    }
}
