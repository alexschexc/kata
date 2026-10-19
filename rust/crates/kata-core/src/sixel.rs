//! Sixel encoder for RGB8 bitmaps, ported from `src/sixel.zig`. The palette is built per image
//! from its own colours: pixels are bucketed at 5 bits per channel, the most frequent buckets
//! (up to 256) become palette entries at their average colour, and every other bucket maps to its
//! nearest entry. No dithering keeps text sharp. Runs of four or more identical sixels use DECGRI.

use std::io::{self, Write};

const BUCKETS: usize = 1 << 15;

fn bucket_of(p: &[u8]) -> usize {
    (usize::from(p[0] >> 3) << 10) | (usize::from(p[1] >> 3) << 5) | usize::from(p[2] >> 3)
}

pub struct Palette {
    pub colors: Vec<[u8; 3]>,
    /// bucket → palette index
    pub map: Vec<u8>,
}

/// Builds the palette for rows [top, top + height) of the bitmap.
pub fn palette(pixels: &[u8], width: usize, top: usize, height: usize) -> Palette {
    let mut counts = vec![0u32; BUCKETS];
    let mut sums = vec![[0u64; 3]; BUCKETS];
    for p in pixels[top * width * 3..(top + height) * width * 3]
        .as_chunks::<3>()
        .0
    {
        let b = bucket_of(p);
        counts[b] += 1;
        for c in 0..3 {
            sums[b][c] += u64::from(p[c]);
        }
    }
    let mut order: Vec<usize> = (0..BUCKETS).filter(|&b| counts[b] > 0).collect();
    order.sort_unstable_by(|&a, &b| counts[b].cmp(&counts[a]).then(a.cmp(&b)));
    let len = order.len().min(256);
    let average = |b: usize, c: usize| (sums[b][c] / u64::from(counts[b])) as u8;
    let mut map = vec![0u8; BUCKETS];
    let mut colors: Vec<[u8; 3]> = Vec::with_capacity(len);
    for (i, &b) in order[..len].iter().enumerate() {
        colors.push([average(b, 0), average(b, 1), average(b, 2)]);
        map[b] = i as u8;
    }
    for &b in &order[len..] {
        let (mut best, mut best_d) = (0usize, u32::MAX);
        for (i, color) in colors.iter().enumerate() {
            let mut d = 0u32;
            for (c, &component) in color.iter().enumerate() {
                let delta = i32::from(average(b, c)) - i32::from(component);
                d += (delta * delta) as u32;
            }
            if d < best_d {
                best_d = d;
                best = i;
            }
        }
        map[b] = best as u8;
    }
    Palette { colors, map }
}

fn run<W: Write>(w: &mut W, ch: u8, count: usize) -> io::Result<()> {
    if count >= 4 {
        write!(w, "!{count}{}", ch as char)
    } else {
        for _ in 0..count {
            w.write_all(&[ch])?;
        }
        Ok(())
    }
}

/// Encodes rows [top, top + height) of an RGB8 bitmap `width` pixels wide as a complete sixel
/// sequence (DCS … ST). `height` should be a multiple of 6 so the image never extends below the
/// rows reserved for it.
pub fn encode<W: Write>(
    w: &mut W,
    pixels: &[u8],
    width: usize,
    top: usize,
    height: usize,
) -> io::Result<()> {
    let pal = palette(pixels, width, top, height);
    let mut indexes = vec![0u8; width * height];
    for (i, index) in indexes.iter_mut().enumerate() {
        *index = pal.map[bucket_of(&pixels[(top * width + i) * 3..])];
    }
    write!(w, "\x1bP0;1;0q\"1;1;{width};{height}")?;
    for (i, c) in pal.colors.iter().enumerate() {
        let pct = |v: u8| (u32::from(v) * 100 + 127) / 255;
        write!(w, "#{i};2;{};{};{}", pct(c[0]), pct(c[1]), pct(c[2]))?;
    }
    let mut band = 0;
    while band < height {
        let rows = 6.min(height - band);
        let mut present = [false; 256];
        for r in 0..rows {
            for &index in &indexes[(band + r) * width..(band + r + 1) * width] {
                present[usize::from(index)] = true;
            }
        }
        let mut first = true;
        for color in (0..256).filter(|&c| present[c]) {
            if !first {
                w.write_all(b"$")?;
            }
            first = false;
            write!(w, "#{color}")?;
            let (mut previous, mut count) = (0u8, 0usize);
            for x in 0..width {
                let mut bits = 0u8;
                for r in 0..rows {
                    if usize::from(indexes[(band + r) * width + x]) == color {
                        bits |= 1 << r;
                    }
                }
                let ch = 63 + bits;
                if count > 0 && ch == previous {
                    count += 1;
                } else {
                    if count > 0 {
                        run(w, previous, count)?;
                    }
                    previous = ch;
                    count = 1;
                }
            }
            // Trailing blank sixels draw nothing; omit them.
            if previous != 63 {
                run(w, previous, count)?;
            }
        }
        w.write_all(b"-")?;
        band += 6;
    }
    w.write_all(b"\x1b\\")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_reproduces_the_images_own_colours_most_frequent_first() {
        let px = [
            178, 212, 235, 178, 212, 235, 178, 212, 235, 0, 0, 0, 0, 0, 0,
        ];
        let pal = palette(&px, 5, 0, 1);
        assert_eq!(pal.colors, [[178, 212, 235], [0, 0, 0]]);
    }

    #[test]
    fn encoder_emits_header_palette_run_length_bands_and_terminator() {
        // 8×6: left half black, right half pure red.
        let mut px = [0u8; 8 * 6 * 3];
        for y in 0..6 {
            for x in 4..8 {
                px[(y * 8 + x) * 3] = 255;
            }
        }
        let mut out = Vec::new();
        encode(&mut out, &px, 8, 0, 6).unwrap();
        let s = String::from_utf8(out).unwrap();
        assert!(s.starts_with("\x1bP0;1;0q\"1;1;8;6"));
        assert!(s.ends_with("-\x1b\\"));
        assert!(s.contains(";2;100;0;0"));
        assert!(s.contains("!4~")); // 4 full black columns
        assert!(s.contains("!4?!4~")); // red: 4 blank then 4 full
    }
}
