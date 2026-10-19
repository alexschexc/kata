//! Inline book image decoding: PNG (png crate) and JPEG (zune-jpeg), both to RGB8 so scaling and
//! the sixel/kitty encoders are shared. Ported from `src/png.zig` / `src/jpeg.zig`. PNG pixel
//! conversion keeps the Zig rules (high byte of 16-bit samples, alpha composited over white
//! paper, interlaced files rejected so callers fall back to the external viewer).

use std::io::Cursor;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    /// RGB8, row-major, `width * height * 3` bytes.
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageError {
    NotPng,
    UnsupportedPng,
    CorruptPng,
    NotJpeg,
    UnsupportedJpeg,
    CorruptJpeg,
}

impl std::fmt::Display for ImageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for ImageError {}

const MAX_PIXELS: u64 = 40_000_000;
const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub depth: u8,
    pub color: u8,
    pub interlaced: bool,
}

/// Header fields from IHDR, without decoding any image data.
pub fn png_info(bytes: &[u8]) -> Result<PngInfo, ImageError> {
    if bytes.len() < 33 || !bytes.starts_with(PNG_SIGNATURE) || &bytes[12..16] != b"IHDR" {
        return Err(ImageError::NotPng);
    }
    let be = |at: usize| u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap());
    Ok(PngInfo {
        width: be(16),
        height: be(20),
        depth: bytes[24],
        color: bytes[25],
        interlaced: bytes[28] != 0,
    })
}

fn channels(color: u8) -> Result<usize, ImageError> {
    match color {
        0 | 3 => Ok(1),
        2 => Ok(3),
        4 => Ok(2),
        6 => Ok(4),
        _ => Err(ImageError::UnsupportedPng),
    }
}

fn valid_depth(color: u8, depth: u8) -> bool {
    match color {
        0 => matches!(depth, 1 | 2 | 4 | 8 | 16),
        3 => matches!(depth, 1 | 2 | 4 | 8),
        2 | 4 | 6 => matches!(depth, 8 | 16),
        _ => false,
    }
}

/// Sample `index` (in samples) of a scanline, scaled to 8 bits (16-bit keeps its high byte).
fn sample(line: &[u8], index: usize, depth: u8) -> u8 {
    match depth {
        8 => line[index],
        16 => line[index * 2],
        _ => {
            let bit = index * usize::from(depth);
            let shift = 8 - usize::from(depth) - bit % 8;
            (line[bit / 8] >> shift) & ((1u8 << depth) - 1)
        }
    }
}

fn blend(c: u8, a: u8) -> u8 {
    ((u32::from(c) * u32::from(a) + 255 * (255 - u32::from(a)) + 127) / 255) as u8
}

pub fn png_decode(bytes: &[u8]) -> Result<Image, ImageError> {
    let header = png_info(bytes)?;
    if header.interlaced || !valid_depth(header.color, header.depth) {
        return Err(ImageError::UnsupportedPng);
    }
    let (width, height) = (header.width, header.height);
    if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_PIXELS {
        return Err(ImageError::UnsupportedPng);
    }
    let n = channels(header.color)?;
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    // Keep samples exactly as stored: unfiltered scanlines only; conversion is ours.
    decoder.set_transformations(png::Transformations::IDENTITY);
    let mut reader = decoder.read_info().map_err(|_| ImageError::CorruptPng)?;
    let (palette, trns) = {
        let info = reader.info();
        (
            info.palette.as_deref().unwrap_or(&[]).to_vec(),
            info.trns.as_deref().unwrap_or(&[]).to_vec(),
        )
    };
    if header.color == 3 && palette.len() < 3 {
        return Err(ImageError::CorruptPng);
    }
    let max_value: u32 = if header.depth >= 8 {
        255
    } else {
        (1u32 << header.depth) - 1
    };
    let mut pixels = vec![0u8; width as usize * height as usize * 3];
    for y in 0..height as usize {
        let row = reader
            .next_row()
            .map_err(|_| ImageError::CorruptPng)?
            .ok_or(ImageError::CorruptPng)?;
        let current = row.data();
        let out = &mut pixels[y * width as usize * 3..][..width as usize * 3];
        for x in 0..width as usize {
            let mut alpha = 255u8;
            let rgb: [u8; 3] = match header.color {
                0 | 4 => {
                    let v =
                        (u32::from(sample(current, x * n, header.depth)) * 255 / max_value) as u8;
                    if header.color == 4 {
                        alpha = sample(current, x * n + 1, header.depth);
                    }
                    [v, v, v]
                }
                2 | 6 => {
                    if header.color == 6 {
                        alpha = sample(current, x * n + 3, header.depth);
                    }
                    [
                        sample(current, x * n, header.depth),
                        sample(current, x * n + 1, header.depth),
                        sample(current, x * n + 2, header.depth),
                    ]
                }
                _ => {
                    let index = usize::from(sample(current, x, header.depth));
                    if index * 3 + 2 >= palette.len() {
                        return Err(ImageError::CorruptPng);
                    }
                    if index < trns.len() {
                        alpha = trns[index];
                    }
                    [
                        palette[index * 3],
                        palette[index * 3 + 1],
                        palette[index * 3 + 2],
                    ]
                }
            };
            for c in 0..3 {
                out[x * 3 + c] = if alpha == 255 {
                    rgb[c]
                } else {
                    blend(rgb[c], alpha)
                };
            }
        }
    }
    Ok(Image {
        width,
        height,
        pixels,
    })
}

/// Decodes by signature.
pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.starts_with(PNG_SIGNATURE) {
        png_decode(bytes)
    } else {
        crate::jpeg::decode(bytes)
    }
}

/// Area-averaging resize (box filter); each source pixel is read about once.
pub fn scale(image: &Image, width: u32, height: u32) -> Image {
    let (w, h) = (width.max(1) as usize, height.max(1) as usize);
    let (sw, sh) = (image.width as usize, image.height as usize);
    let mut pixels = vec![0u8; w * h * 3];
    for y in 0..h {
        let y0 = y * sh / h;
        let y1 = (y0 + 1).max((y + 1) * sh / h);
        for x in 0..w {
            let x0 = x * sw / w;
            let x1 = (x0 + 1).max((x + 1) * sw / w);
            let mut sum = [0u32; 3];
            for sy in y0..y1 {
                for sx in x0..x1 {
                    let p = (sy * sw + sx) * 3;
                    for (c, total) in sum.iter_mut().enumerate() {
                        *total += u32::from(image.pixels[p + c]);
                    }
                }
            }
            let count = ((y1 - y0) * (x1 - x0)) as u32;
            for (c, total) in sum.iter().enumerate() {
                pixels[(y * w + x) * 3 + c] = ((total + count / 2) / count) as u8;
            }
        }
    }
    Image {
        width: w as u32,
        height: h as u32,
        pixels,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = !0u32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xEDB8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    fn chunk(out: &mut Vec<u8>, kind: &[u8], body: &[u8]) {
        out.extend_from_slice(&(body.len() as u32).to_be_bytes());
        out.extend_from_slice(kind);
        out.extend_from_slice(body);
        let mut covered = kind.to_vec();
        covered.extend_from_slice(body);
        out.extend_from_slice(&crc32(&covered).to_be_bytes());
    }

    fn adler32(data: &[u8]) -> u32 {
        let (mut a, mut b) = (1u32, 0u32);
        for &x in data {
            a = (a + u32::from(x)) % 65521;
            b = (b + a) % 65521;
        }
        (b << 16) | a
    }

    /// PNG built by hand with a stored (uncompressed) deflate block, as in the Zig tests.
    fn test_png(
        width: u32,
        height: u32,
        depth: u8,
        color: u8,
        raw: &[u8],
        plte: &[u8],
        trns: &[u8],
    ) -> Vec<u8> {
        let mut out = PNG_SIGNATURE.to_vec();
        let mut ihdr = Vec::new();
        ihdr.extend_from_slice(&width.to_be_bytes());
        ihdr.extend_from_slice(&height.to_be_bytes());
        ihdr.extend_from_slice(&[depth, color, 0, 0, 0]);
        chunk(&mut out, b"IHDR", &ihdr);
        if !plte.is_empty() {
            chunk(&mut out, b"PLTE", plte);
        }
        if !trns.is_empty() {
            chunk(&mut out, b"tRNS", trns);
        }
        let mut z = vec![0x78, 0x01, 0x01];
        z.extend_from_slice(&(raw.len() as u16).to_le_bytes());
        z.extend_from_slice(&(!(raw.len() as u16)).to_le_bytes());
        z.extend_from_slice(raw);
        z.extend_from_slice(&adler32(raw).to_be_bytes());
        chunk(&mut out, b"IDAT", &z);
        chunk(&mut out, b"IEND", b"");
        out
    }

    #[test]
    fn all_five_filters_reconstruct_rgb_scanlines() {
        let raw = [
            0, 10, 20, 30, 40, 50, 60, // none
            1, 10, 20, 30, 30, 30, 30, // sub
            2, 0, 0, 0, 0, 0, 0, // up
            3, 5, 10, 15, 15, 15, 15, // average
            4, 0, 0, 0, 0, 0, 0, // paeth picks up
        ];
        let image = png_decode(&test_png(2, 5, 8, 2, &raw, &[], &[])).unwrap();
        for y in 0..5 {
            assert_eq!(&image.pixels[y * 6..y * 6 + 6], [10, 20, 30, 40, 50, 60]);
        }
    }

    #[test]
    fn palette_low_bit_depth_transparency_and_grayscale_alpha() {
        let plte = [0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255];
        let image = png_decode(&test_png(4, 1, 2, 3, &[0, 0b00011011], &plte, &[255, 0])).unwrap();
        assert_eq!(image.pixels, [0, 0, 0, 255, 255, 255, 0, 255, 0, 0, 0, 255]);
        let bits = png_decode(&test_png(3, 1, 1, 0, &[0, 0b10100000], &[], &[])).unwrap();
        assert_eq!(bits.pixels, [255, 255, 255, 0, 0, 0, 255, 255, 255]);
        let ga = png_decode(&test_png(1, 1, 8, 4, &[0, 0, 0], &[], &[])).unwrap();
        assert_eq!(ga.pixels, [255, 255, 255]);
        assert_eq!(
            png_decode(b"GIF89a not a png at all......................"),
            Err(ImageError::NotPng)
        );
    }

    #[test]
    fn interlaced_and_invalid_depths_are_rejected_and_truncation_is_corrupt() {
        let mut interlaced = test_png(1, 1, 8, 0, &[0, 7], &[], &[]);
        interlaced[28] = 1;
        assert_eq!(png_decode(&interlaced), Err(ImageError::UnsupportedPng));
        assert_eq!(
            png_decode(&test_png(1, 1, 3, 0, &[0, 7], &[], &[])),
            Err(ImageError::UnsupportedPng)
        );
        let good = test_png(2, 2, 8, 0, &[0, 1, 2, 0, 3, 4], &[], &[]);
        assert_eq!(
            png_decode(&good[..good.len() - 26]),
            Err(ImageError::CorruptPng)
        );
    }

    #[test]
    fn area_scaling_averages_blocks() {
        let image = Image {
            width: 2,
            height: 2,
            pixels: vec![0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0],
        };
        assert_eq!(scale(&image, 1, 1).pixels, [128, 128, 128]);
    }
}
