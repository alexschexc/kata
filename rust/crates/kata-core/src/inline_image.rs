//! Inline book images (Sixel, or Kitty graphics in Ghostty/kitty/WezTerm), ported from
//! `src/inline_image.zig`. The reader reserves `rows` text lines per image; after drawing a frame
//! it asks `draw` to paint whichever rows of the image are visible. Decoded+scaled bitmaps are
//! cached per (asset, width); encoding the visible band is cheap and done per frame.

use crate::graphics::{Capabilities, Protocol};
use crate::image;
use crate::jpeg;
use crate::sixel;
use std::collections::HashMap;
use std::fs;
use std::io::{self, Read, Write};
use std::path::PathBuf;

/// Paper colour behind images (matches the reader background).
const PAPER: [u8; 3] = [21, 25, 34];
const CACHE_LIMIT: usize = 48 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fit {
    pub width: u32,
    pub height: u32,
    pub rows: u16,
}

/// Scales (w, h) to at most `cols` cells wide and `max_rows` cells tall, never enlarging. `rows`
/// is the number of text lines to reserve.
pub fn fit(w: u32, h: u32, cols: usize, max_rows: usize, cell_w: u16, cell_h: u16) -> Option<Fit> {
    fit_scaled(w, h, cols, max_rows, cell_w, cell_h, false)
}

/// `enlarge`: small images grow to fill the column/height limits.
pub fn fit_scaled(
    w: u32,
    h: u32,
    cols: usize,
    max_rows: usize,
    cell_w: u16,
    cell_h: u16,
    enlarge: bool,
) -> Option<Fit> {
    if w == 0 || h == 0 || cols == 0 || max_rows == 0 || cell_w == 0 || cell_h == 0 {
        return None;
    }
    let column = cols as u64 * u64::from(cell_w);
    let mut width = if enlarge {
        column
    } else {
        u64::from(w).min(column)
    };
    let mut height = (u64::from(h) * width / u64::from(w)).max(1);
    let max_h = max_rows as u64 * u64::from(cell_h);
    if height > max_h {
        height = max_h;
        width = (u64::from(w) * height / u64::from(h)).max(1);
    }
    let rows = height.div_ceil(u64::from(cell_h));
    Some(Fit {
        width: width as u32,
        height: height as u32,
        rows: rows as u16,
    })
}

const BASE64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn base64(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        out.push(BASE64[(n >> 18) as usize & 63]);
        out.push(BASE64[(n >> 12) as usize & 63]);
        out.push(if chunk.len() > 1 {
            BASE64[(n >> 6) as usize & 63]
        } else {
            b'='
        });
        out.push(if chunk.len() > 2 {
            BASE64[n as usize & 63]
        } else {
            b'='
        });
    }
    out
}

/// Kitty graphics: transmits RGB once (chunked base64, quiet), then each frame places the visible
/// source rows. Placements are deleted at the start of every frame, so scrolling never leaves
/// stale images behind.
pub fn kitty_transmit<W: Write>(
    w: &mut W,
    id: u32,
    pixels: &[u8],
    width: u32,
    height: usize,
) -> io::Result<()> {
    const CHUNK: usize = 3072; // multiple of 3 → 4096 base64 bytes per escape
    let (mut at, mut first) = (0, true);
    while at < pixels.len() || first {
        let end = pixels.len().min(at + CHUNK);
        let more = u8::from(end < pixels.len());
        if first {
            write!(
                w,
                "\x1b_Ga=t,q=2,f=24,i={id},s={width},v={height},m={more};"
            )?;
        } else {
            write!(w, "\x1b_Gq=2,m={more};")?;
        }
        w.write_all(&base64(&pixels[at..end]))?;
        w.write_all(b"\x1b\\")?;
        at = end;
        first = false;
    }
    Ok(())
}

/// Removes every Kitty placement from the screen (image data is kept).
pub const KITTY_CLEAR: &str = "\x1b_Ga=d,d=a,q=2\x1b\\";

struct Bitmap {
    fit: Fit,
    /// width × (rows * cell_h) RGB, padded below the image with paper.
    pixels: Vec<u8>,
    /// Kitty image id once transmitted to the terminal (0: not yet).
    kitty_id: u32,
    /// Last sixel encoding (rows top..top+height), reused while the visible part is unchanged so
    /// redraws do not re-encode.
    sixel: Vec<u8>,
    sixel_top: usize,
    sixel_height: usize,
}

pub struct Renderer {
    /// `<library>/<book>.assets`
    dir: PathBuf,
    pub caps: Capabilities,
    /// asset name → image dimensions (None: not displayable inline)
    dims: HashMap<String, Option<(u32, u32)>>,
    bitmaps: HashMap<String, Bitmap>,
    cached_bytes: usize,
    /// Kitty images to delete from terminal memory at the next frame.
    pending_clear: bool,
    next_id: u32,
    /// Let small images grow to the size limits (largest zoom level).
    enlarge: bool,
}

impl Renderer {
    pub fn new(dir: impl Into<PathBuf>, caps: Capabilities) -> Renderer {
        Renderer {
            dir: dir.into(),
            caps,
            dims: HashMap::new(),
            bitmaps: HashMap::new(),
            cached_bytes: 0,
            pending_clear: false,
            next_id: 1,
            enlarge: false,
        }
    }

    pub fn enabled(&self) -> bool {
        self.caps.protocol != Protocol::None
            && self.caps.cell_width > 0
            && self.caps.cell_height > 0
    }

    /// Updates the cell size (window resized or font changed); drops bitmaps sized for the old
    /// cells. Returns true when it changed.
    pub fn set_cell(&mut self, width: u16, height: u16) -> bool {
        if width == 0
            || height == 0
            || (width == self.caps.cell_width && height == self.caps.cell_height)
        {
            return false;
        }
        self.caps.cell_width = width;
        self.caps.cell_height = height;
        self.clear();
        true
    }

    /// Switches enlargement of small images; drops bitmaps at the old size.
    pub fn set_enlarge(&mut self, on: bool) {
        if on == self.enlarge {
            return;
        }
        self.enlarge = on;
        self.clear();
    }

    fn clear(&mut self) {
        // Forget transmitted kitty images too (they are re-sent if needed).
        if self.caps.protocol == Protocol::Kitty {
            self.pending_clear = true;
        }
        self.bitmaps.clear();
        self.cached_bytes = 0;
    }

    fn dimensions(&mut self, name: &str) -> Option<(u32, u32)> {
        if let Some(known) = self.dims.get(name) {
            return *known;
        }
        let value = (|| {
            // JPEG frame headers follow EXIF/ICC, so read a generous prefix.
            let mut header = Vec::new();
            fs::File::open(self.dir.join(name))
                .ok()?
                .take(64 << 10)
                .read_to_end(&mut header)
                .ok()?;
            if let Ok(info) = image::png_info(&header) {
                return if info.interlaced {
                    None
                } else {
                    Some((info.width, info.height))
                };
            }
            let info = jpeg::info(&header).ok()?;
            Some((info.width, info.height))
        })();
        self.dims.insert(name.to_string(), value);
        value
    }

    /// Text lines to reserve for `name` at this width, or None to show the placeholder line.
    pub fn rows(&mut self, name: &str, cols: usize, max_rows: usize) -> Option<u16> {
        if !self.enabled() {
            return None;
        }
        let (w, h) = self.dimensions(name)?;
        fit_scaled(
            w,
            h,
            cols,
            max_rows,
            self.caps.cell_width,
            self.caps.cell_height,
            self.enlarge,
        )
        .map(|f| f.rows)
    }

    /// Start-of-frame commands: clear kitty placements (and freed images).
    pub fn begin_frame<W: Write>(&mut self, w: &mut W) -> io::Result<()> {
        if self.caps.protocol != Protocol::Kitty {
            return Ok(());
        }
        if self.pending_clear {
            w.write_all(b"\x1b_Ga=d,d=A,q=2\x1b\\")?; // delete placements and image data
            self.pending_clear = false;
        } else {
            w.write_all(KITTY_CLEAR.as_bytes())?;
        }
        Ok(())
    }

    /// Decodes and scales `name` for this width into the cache; returns its cache key.
    fn bitmap(&mut self, name: &str, cols: usize, max_rows: usize) -> Option<String> {
        let key = format!("{name}\0{cols}\0{max_rows}");
        if self.bitmaps.contains_key(&key) {
            return Some(key);
        }
        let (w, h) = self.dimensions(name)?;
        let f = fit_scaled(
            w,
            h,
            cols,
            max_rows,
            self.caps.cell_width,
            self.caps.cell_height,
            self.enlarge,
        )?;
        let bytes = fs::read(self.dir.join(name))
            .ok()
            .filter(|b| b.len() <= 64 << 20)?;
        let decoded = if bytes.starts_with(&[0xff, 0xd8]) {
            jpeg::decode(&bytes).ok()?
        } else {
            image::png_decode(&bytes).ok()?
        };
        let scaled = image::scale(&decoded, f.width, f.height);
        let total_h = usize::from(f.rows) * usize::from(self.caps.cell_height);
        let mut pixels = vec![0u8; f.width as usize * total_h * 3];
        pixels[..scaled.pixels.len()].copy_from_slice(&scaled.pixels);
        for rgb in pixels[scaled.pixels.len()..].as_chunks_mut::<3>().0 {
            *rgb = PAPER;
        }
        if self.cached_bytes + pixels.len() > CACHE_LIMIT {
            self.clear();
        }
        self.cached_bytes += pixels.len();
        self.bitmaps.insert(
            key.clone(),
            Bitmap {
                fit: f,
                pixels,
                kitty_id: 0,
                sixel: Vec::new(),
                sixel_top: 0,
                sixel_height: 0,
            },
        );
        Some(key)
    }

    /// Paints image rows [first_row, first_row + count) at the cursor's current cell. Never paints
    /// below the reserved rows. Returns false if the image could not be decoded (caller keeps the
    /// blank rows).
    pub fn draw<W: Write>(
        &mut self,
        w: &mut W,
        name: &str,
        cols: usize,
        max_rows: usize,
        first_row: usize,
        count: usize,
    ) -> io::Result<bool> {
        let Some(key) = self.bitmap(name, cols, max_rows) else {
            return Ok(false);
        };
        let cell_h = usize::from(self.caps.cell_height);
        let fit = self.bitmaps[&key].fit;
        let top = first_row * cell_h;
        if top >= usize::from(fit.rows) * cell_h {
            return Ok(true);
        }
        let rows_left = usize::from(fit.rows) - first_row;
        if self.caps.protocol == Protocol::Kitty {
            let rows_shown = count.min(rows_left);
            let height = rows_shown * cell_h;
            if height == 0 {
                return Ok(true);
            }
            let total_h = usize::from(fit.rows) * cell_h;
            let bitmap = self.bitmaps.get_mut(&key).expect("just cached");
            if bitmap.kitty_id == 0 {
                bitmap.kitty_id = self.next_id;
                self.next_id += 1;
                kitty_transmit(w, bitmap.kitty_id, &bitmap.pixels, fit.width, total_h)?;
            }
            // Place the visible source rows over exactly `rows_shown` cells; C=1 keeps the cursor
            // still so text drawing is unaffected.
            write!(
                w,
                "\x1b_Ga=p,q=2,i={},y={top},w={},h={height},r={rows_shown},C=1\x1b\\",
                bitmap.kitty_id, fit.width
            )?;
            return Ok(true);
        }
        // Whole sixel bands only, so nothing spills into the next text line.
        let height = (count.min(rows_left) * cell_h) / 6 * 6;
        if height == 0 {
            return Ok(true);
        }
        let bitmap = self.bitmaps.get_mut(&key).expect("just cached");
        if bitmap.sixel.is_empty() || bitmap.sixel_top != top || bitmap.sixel_height != height {
            let mut out = Vec::new();
            sixel::encode(&mut out, &bitmap.pixels, fit.width as usize, top, height)?;
            self.cached_bytes = self.cached_bytes - bitmap.sixel.len() + out.len();
            bitmap.sixel = out;
            bitmap.sixel_top = top;
            bitmap.sixel_height = height;
        }
        w.write_all(&bitmap.sixel)?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_never_enlarges_respects_column_and_row_limits_and_rounds_rows_up() {
        assert_eq!(
            fit(100, 50, 80, 20, 10, 20),
            Some(Fit {
                width: 100,
                height: 50,
                rows: 3
            })
        );
        assert_eq!(
            fit(2000, 1000, 80, 30, 10, 20),
            Some(Fit {
                width: 800,
                height: 400,
                rows: 20
            })
        );
        let tall = fit(400, 2000, 80, 10, 10, 20).unwrap();
        assert_eq!((tall.rows, tall.height, tall.width), (10, 200, 40));
        assert_eq!(fit(100, 100, 80, 20, 0, 20), None);
    }

    #[test]
    fn kitty_transmit_chunks_base64_with_continuation_flags() {
        let mut out = Vec::new();
        kitty_transmit(&mut out, 5, &vec![7u8; 4000 * 3], 4000, 1).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1b_Ga=t,q=2,f=24,i=5,s=4000,v=1,m=1;"));
        assert_eq!(s.matches("\x1b\\").count(), 4); // 12000 bytes / 3072
        assert!(s.contains("\x1b_Gq=2,m=0;"));
    }

    #[test]
    fn enlarge_grows_small_images_to_the_column() {
        assert_eq!(
            fit_scaled(100, 50, 80, 30, 10, 20, true),
            Some(Fit {
                width: 800,
                height: 400,
                rows: 20
            })
        );
        assert_eq!(
            fit_scaled(100, 50, 80, 30, 10, 20, false).unwrap().width,
            100
        );
    }

    #[test]
    fn base64_matches_the_standard_alphabet_with_padding() {
        assert_eq!(base64(b""), b"");
        assert_eq!(base64(b"f"), b"Zg==");
        assert_eq!(base64(b"fo"), b"Zm8=");
        assert_eq!(base64(b"foo"), b"Zm9v");
        assert_eq!(base64(b"foobar"), b"Zm9vYmFy");
    }

    #[test]
    fn missing_or_undecodable_assets_fall_back_to_the_placeholder() {
        let caps = Capabilities {
            protocol: Protocol::Sixel,
            cell_width: 10,
            cell_height: 20,
            answered: true,
            ..Capabilities::default()
        };
        let mut renderer = Renderer::new(std::env::temp_dir().join("kata-no-such-assets"), caps);
        assert_eq!(renderer.rows("x.png", 80, 20), None);
        let mut out = Vec::new();
        assert!(!renderer.draw(&mut out, "x.png", 80, 20, 0, 5).unwrap());
        assert!(out.is_empty());
        renderer.caps.protocol = Protocol::None;
        assert!(!renderer.enabled());
    }
}
