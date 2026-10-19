//! JPEG decoder for inline book images, ported line for line from `src/jpeg.zig`: Huffman
//! sequential (SOF0/SOF1) and progressive (SOF2), 8-bit samples, any sampling factors, restart
//! intervals, grayscale, YCbCr and Adobe CMYK/YCCK. Arithmetic-coded and lossless files return
//! `UnsupportedJpeg`. A third-party decoder (zune-jpeg) was tried first and differed from the Zig
//! pixels by up to 37 levels (different IDCT and chroma upsampling), so the Zig algorithm is kept
//! to preserve identical output.

use crate::image::{Image, ImageError};

const MAX_PIXELS: usize = 40_000_000;

const ZIGZAG: [usize; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20,
    13, 6, 7, 14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59,
    52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JpegInfo {
    pub width: u32,
    pub height: u32,
    pub progressive: bool,
    pub components: u8,
}

fn be16(bytes: &[u8], at: usize) -> usize {
    usize::from(u16::from_be_bytes([bytes[at], bytes[at + 1]]))
}

/// Frame header fields without decoding (for layout before display).
pub fn info(bytes: &[u8]) -> Result<JpegInfo, ImageError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(ImageError::NotJpeg);
    }
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return Err(ImageError::CorruptJpeg);
        }
        let marker = bytes[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        let len = be16(bytes, at + 2);
        if at + 2 + len > bytes.len() {
            return Err(ImageError::CorruptJpeg);
        }
        match marker {
            0xC0..=0xC2 => {
                if len < 8 {
                    return Err(ImageError::CorruptJpeg);
                }
                return Ok(JpegInfo {
                    height: be16(bytes, at + 5) as u32,
                    width: be16(bytes, at + 7) as u32,
                    progressive: marker == 0xC2,
                    components: bytes[at + 9],
                });
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(ImageError::UnsupportedJpeg);
            }
            0xDA | 0xD9 => return Err(ImageError::CorruptJpeg),
            _ => at += 2 + len,
        }
    }
    Err(ImageError::CorruptJpeg)
}

const CORRUPT: ImageError = ImageError::CorruptJpeg;

#[derive(Clone)]
struct Huffman {
    /// Canonical decoding tables (JPEG Annex F.2.2.3).
    maxcode: [i32; 18],
    valptr: [i32; 17],
    mincode: [i32; 17],
    values: [u8; 256],
    defined: bool,
}

impl Default for Huffman {
    fn default() -> Self {
        Huffman {
            maxcode: [-1; 18],
            valptr: [0; 17],
            mincode: [0; 17],
            values: [0; 256],
            defined: false,
        }
    }
}

impl Huffman {
    fn new(counts: [u8; 16], values: &[u8]) -> Result<Huffman, ImageError> {
        let mut h = Huffman {
            defined: true,
            ..Huffman::default()
        };
        let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
        if total > 256 || total > values.len() {
            return Err(CORRUPT);
        }
        h.values[..total].copy_from_slice(&values[..total]);
        let (mut code, mut k) = (0i32, 0i32);
        for l in 1..17 {
            let n = i32::from(counts[l - 1]);
            if n == 0 {
                h.maxcode[l] = -1;
            } else {
                h.valptr[l] = k;
                h.mincode[l] = code;
                code += n;
                k += n;
                h.maxcode[l] = code - 1;
            }
            code <<= 1;
        }
        h.maxcode[17] = i32::MAX;
        Ok(h)
    }
}

struct Bits<'a> {
    data: &'a [u8],
    at: usize,
    acc: u32,
    count: u32,
    /// A marker was reached; further reads yield zero bits.
    hit_marker: bool,
}

impl<'a> Bits<'a> {
    fn new(data: &'a [u8], at: usize) -> Self {
        Bits {
            data,
            at,
            acc: 0,
            count: 0,
            hit_marker: false,
        }
    }

    fn byte(&mut self) -> u8 {
        if self.hit_marker || self.at >= self.data.len() {
            return 0;
        }
        let b = self.data[self.at];
        if b == 0xFF {
            let next = self.data.get(self.at + 1).copied().unwrap_or(0xD9);
            if next == 0x00 {
                self.at += 2;
                return 0xFF;
            }
            self.hit_marker = true;
            return 0;
        }
        self.at += 1;
        b
    }

    fn bit(&mut self) -> u32 {
        if self.count == 0 {
            self.acc = u32::from(self.byte());
            self.count = 8;
        }
        self.count -= 1;
        (self.acc >> self.count) & 1
    }

    fn receive(&mut self, n: u32) -> i32 {
        let mut v = 0i32;
        for _ in 0..n {
            v = (v << 1) | self.bit() as i32;
        }
        v
    }

    fn decode(&mut self, h: &Huffman) -> Result<u8, ImageError> {
        let mut code = self.bit() as i32;
        for l in 1..=16 {
            if code <= h.maxcode[l] {
                let index = h.valptr[l] + code - h.mincode[l];
                return usize::try_from(index)
                    .ok()
                    .and_then(|i| h.values.get(i))
                    .copied()
                    .ok_or(CORRUPT);
            }
            code = (code << 1) | self.bit() as i32;
        }
        Err(CORRUPT)
    }

    /// Skips to just past an RSTn marker and resets the bit buffer.
    fn restart(&mut self) {
        self.count = 0;
        self.hit_marker = false;
        while self.at + 1 < self.data.len() {
            if self.data[self.at] == 0xFF && (0xD0..=0xD7).contains(&self.data[self.at + 1]) {
                self.at += 2;
                return;
            }
            self.at += 1;
        }
    }
}

fn extend(v: i32, n: u32) -> i32 {
    if n == 0 {
        return 0;
    }
    if v < (1 << (n - 1)) {
        v - (1 << n) + 1
    } else {
        v
    }
}

#[derive(Default)]
struct Component {
    id: u8,
    h: u8,
    v: u8,
    tq: u8,
    td: usize,
    ta: usize,
    dc: i32,
    /// Blocks across/down the padded plane.
    bw: usize,
    bh: usize,
    plane: Vec<u8>,
    /// Dequantization-free coefficients per block (natural order), kept across scans so
    /// progressive refinement can accumulate.
    coefs: Vec<i16>,
}

fn put(coef: &mut i16, v: i32) {
    *coef = v.clamp(-32768, 32767) as i16;
}

struct Scan {
    ss: usize,
    se: usize,
    ah: u8,
    al: u32,
    progressive: bool,
}

/// One block of one scan (baseline or any progressive pass).
fn decode_block(
    bits: &mut Bits,
    c: &mut Component,
    block: &mut [i16; 64],
    dc_tables: &[Huffman; 4],
    ac_tables: &[Huffman; 4],
    scan: &Scan,
    eobrun: &mut u32,
) -> Result<(), ImageError> {
    let Scan {
        ss,
        se,
        ah,
        al,
        progressive,
    } = *scan;
    if !progressive || ss == 0 {
        if ah == 0 || !progressive {
            let t = u32::from(bits.decode(&dc_tables[c.td])?);
            if t > 16 {
                return Err(CORRUPT);
            }
            c.dc = c.dc.wrapping_add(extend(bits.receive(t), t));
            put(&mut block[0], c.dc.wrapping_mul(1 << al));
        } else if bits.bit() == 1 {
            block[0] |= 1 << al;
        }
        if progressive {
            return Ok(());
        }
        // Baseline: AC follows in the same block.
        let mut k = 1usize;
        while k < 64 {
            let rs = bits.decode(&ac_tables[c.ta])?;
            let r = usize::from(rs >> 4);
            let sz = u32::from(rs & 15);
            if sz == 0 {
                if r != 15 {
                    break;
                }
                k += 16;
                continue;
            }
            k += r;
            if k > 63 {
                return Err(CORRUPT);
            }
            put(&mut block[ZIGZAG[k]], extend(bits.receive(sz), sz));
            k += 1;
        }
        return Ok(());
    }
    if ah == 0 {
        // AC first pass.
        if *eobrun > 0 {
            *eobrun -= 1;
            return Ok(());
        }
        let mut k = ss;
        while k <= se {
            let rs = bits.decode(&ac_tables[c.ta])?;
            let r = usize::from(rs >> 4);
            let sz = u32::from(rs & 15);
            if sz != 0 {
                k += r;
                if k > 63 {
                    return Err(CORRUPT);
                }
                put(
                    &mut block[ZIGZAG[k]],
                    extend(bits.receive(sz), sz).wrapping_mul(1 << al),
                );
            } else if r == 15 {
                k += 15;
            } else {
                *eobrun = (1u32 << r) - 1;
                if r > 0 {
                    *eobrun += bits.receive(r as u32) as u32;
                }
                break;
            }
            k += 1;
        }
        return Ok(());
    }
    // AC refinement (libjpeg decode_mcu_AC_refine).
    let p1: i32 = 1 << al;
    let m1: i32 = -p1;
    let mut k = ss;
    let refine = |bits: &mut Bits, z: &mut i16| {
        if bits.bit() == 1 && (i32::from(*z) & p1) == 0 {
            put(z, i32::from(*z) + if *z >= 0 { p1 } else { m1 });
        }
    };
    if *eobrun == 0 {
        while k <= se {
            let rs = bits.decode(&ac_tables[c.ta])?;
            let mut r = i32::from(rs >> 4);
            let mut value = 0i32;
            if rs & 15 != 0 {
                if rs & 15 != 1 {
                    return Err(CORRUPT);
                }
                value = if bits.bit() == 1 { p1 } else { m1 };
            } else if r != 15 {
                *eobrun = 1u32 << r;
                if r > 0 {
                    *eobrun += bits.receive(r as u32) as u32;
                }
                break;
            }
            while k <= se {
                let z = &mut block[ZIGZAG[k]];
                if *z != 0 {
                    refine(bits, z);
                } else {
                    r -= 1;
                    if r < 0 {
                        break;
                    }
                }
                k += 1;
            }
            if value != 0 {
                if k > 63 {
                    return Err(CORRUPT);
                }
                put(&mut block[ZIGZAG[k]], value);
            }
            k += 1;
        }
    }
    if *eobrun > 0 {
        while k <= se {
            let z = &mut block[ZIGZAG[k]];
            if *z != 0 {
                refine(bits, z);
            }
            k += 1;
        }
        *eobrun -= 1;
    }
    Ok(())
}

/// cosines[x][u] = c(u) * cos((2x+1) u pi / 16) / 2, in f32 like the Zig comptime table.
fn cosines() -> [[f32; 8]; 8] {
    let mut table = [[0f32; 8]; 8];
    for (x, row) in table.iter_mut().enumerate() {
        for (u, cell) in row.iter_mut().enumerate() {
            let cu = if u == 0 {
                (1.0f64 / 2.0f64.sqrt()) as f32
            } else {
                1.0f32
            };
            let arg = ((2 * x + 1) * u) as f32 * std::f32::consts::PI / 16.0;
            *cell = cu * f64::from(arg).cos() as f32 / 2.0;
        }
    }
    table
}

fn idct(cos: &[[f32; 8]; 8], coef: &[i32; 64], q: &[u16; 64], out: &mut [u8], stride: usize) {
    let mut f = [0f32; 64];
    for i in 0..64 {
        f[i] = coef[i].wrapping_mul(i32::from(q[i])) as f32;
    }
    let mut tmp = [0f32; 64];
    for y in 0..8 {
        for x in 0..8 {
            let mut s = 0f32;
            for u in 0..8 {
                s += cos[x][u] * f[y * 8 + u];
            }
            tmp[y * 8 + x] = s;
        }
    }
    for x in 0..8 {
        for y in 0..8 {
            let mut s = 0f32;
            for v in 0..8 {
                s += cos[y][v] * tmp[v * 8 + x];
            }
            out[y * stride + x] = (s + 128.0).round().clamp(0.0, 255.0) as u8;
        }
    }
}

fn clamp8(v: f32) -> u8 {
    v.round().clamp(0.0, 255.0) as u8
}

fn ycc(y: f32, cb: f32, cr: f32) -> [u8; 3] {
    [
        clamp8(y + 1.402 * (cr - 128.0)),
        clamp8(y - 0.344136 * (cb - 128.0) - 0.714136 * (cr - 128.0)),
        clamp8(y + 1.772 * (cb - 128.0)),
    ]
}

pub fn decode(bytes: &[u8]) -> Result<Image, ImageError> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return Err(ImageError::NotJpeg);
    }
    let mut quant = [[1u16; 64]; 4];
    let mut dc_tables: [Huffman; 4] = Default::default();
    let mut ac_tables: [Huffman; 4] = Default::default();
    let mut comps: Vec<Component> = Vec::new();
    let (mut width, mut height) = (0usize, 0usize);
    let (mut hmax, mut vmax) = (1u8, 1u8);
    let mut restart_interval = 0usize;
    let mut adobe: Option<u8> = None;
    let mut frame = false;
    let mut progressive = false;
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            return Err(CORRUPT);
        }
        let marker = bytes[at + 1];
        if marker == 0xFF {
            at += 1;
            continue;
        }
        if marker == 0xD9 {
            break;
        }
        let len = be16(bytes, at + 2);
        if len < 2 || at + 2 + len > bytes.len() {
            return Err(CORRUPT);
        }
        let seg = &bytes[at + 4..at + 2 + len];
        at += 2 + len;
        match marker {
            0xDB => {
                let mut i = 0;
                while i < seg.len() {
                    let precision = seg[i] >> 4;
                    let id = usize::from(seg[i] & 15);
                    if id > 3 {
                        return Err(CORRUPT);
                    }
                    i += 1;
                    for &z in &ZIGZAG {
                        if precision == 0 {
                            if i >= seg.len() {
                                return Err(CORRUPT);
                            }
                            quant[id][z] = u16::from(seg[i]);
                            i += 1;
                        } else {
                            if i + 1 >= seg.len() {
                                return Err(CORRUPT);
                            }
                            quant[id][z] = u16::from_be_bytes([seg[i], seg[i + 1]]);
                            i += 2;
                        }
                    }
                }
            }
            0xC4 => {
                let mut i = 0;
                while i + 17 <= seg.len() {
                    let class = seg[i] >> 4;
                    let id = usize::from(seg[i] & 15);
                    if id > 3 || class > 1 {
                        return Err(CORRUPT);
                    }
                    let counts: [u8; 16] = seg[i + 1..i + 17].try_into().unwrap();
                    let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
                    if i + 17 + total > seg.len() {
                        return Err(CORRUPT);
                    }
                    let table = Huffman::new(counts, &seg[i + 17..i + 17 + total])?;
                    if class == 0 {
                        dc_tables[id] = table;
                    } else {
                        ac_tables[id] = table;
                    }
                    i += 17 + total;
                }
            }
            0xDD => {
                if seg.len() < 2 {
                    return Err(CORRUPT);
                }
                restart_interval = be16(seg, 0);
            }
            0xEE => {
                if seg.len() >= 12 && seg.starts_with(b"Adobe") {
                    adobe = Some(seg[11]);
                }
            }
            0xC0..=0xC2 => {
                if seg.len() < 6 || seg[0] != 8 {
                    return Err(ImageError::UnsupportedJpeg);
                }
                progressive = marker == 0xC2;
                height = be16(seg, 1);
                width = be16(seg, 3);
                let n = usize::from(seg[5]);
                if width == 0 || height == 0 || width * height > MAX_PIXELS {
                    return Err(ImageError::UnsupportedJpeg);
                }
                if n != 1 && n != 3 && n != 4 {
                    return Err(ImageError::UnsupportedJpeg);
                }
                if seg.len() < 6 + n * 3 {
                    return Err(CORRUPT);
                }
                comps.clear();
                for k in 0..n {
                    let s = &seg[6 + k * 3..9 + k * 3];
                    let (h, v) = (s[1] >> 4, s[1] & 15);
                    if !(1..=4).contains(&h) || !(1..=4).contains(&v) || s[2] > 3 {
                        return Err(ImageError::UnsupportedJpeg);
                    }
                    comps.push(Component {
                        id: s[0],
                        h,
                        v,
                        tq: s[2],
                        ..Component::default()
                    });
                    hmax = hmax.max(h);
                    vmax = vmax.max(v);
                }
                let mcux = width.div_ceil(8 * usize::from(hmax));
                let mcuy = height.div_ceil(8 * usize::from(vmax));
                for c in &mut comps {
                    c.bw = mcux * usize::from(c.h);
                    c.bh = mcuy * usize::from(c.v);
                    c.plane = vec![0; c.bw * c.bh * 64];
                    c.coefs = vec![0; c.bw * c.bh * 64];
                }
                frame = true;
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                return Err(ImageError::UnsupportedJpeg);
            }
            0xDA => {
                if !frame || seg.is_empty() {
                    return Err(CORRUPT);
                }
                let ns = usize::from(seg[0]);
                if ns == 0 || ns > comps.len() || seg.len() < 1 + ns * 2 + 3 {
                    return Err(CORRUPT);
                }
                let tail = &seg[1 + ns * 2..];
                let (ss, se, ah, al) = (
                    usize::from(tail[0]),
                    usize::from(tail[1]),
                    tail[2] >> 4,
                    u32::from(tail[2] & 15),
                );
                if progressive
                    && (se > 63
                        || ss > se
                        || (ss > 0 && ns != 1)
                        || (ss == 0 && se != 0)
                        || al > 13)
                {
                    return Err(CORRUPT);
                }
                let mut scan: Vec<usize> = Vec::with_capacity(ns);
                for k in 0..ns {
                    let id = seg[1 + k * 2];
                    let tables = seg[2 + k * 2];
                    let index = comps.iter().position(|c| c.id == id).ok_or(CORRUPT)?;
                    let c = &mut comps[index];
                    c.td = usize::from(tables >> 4);
                    c.ta = usize::from(tables & 15);
                    if c.td > 3 || c.ta > 3 {
                        return Err(CORRUPT);
                    }
                    let needs_dc = !progressive || (ss == 0 && ah == 0);
                    let needs_ac = !progressive || ss > 0;
                    if (needs_dc && !dc_tables[c.td].defined)
                        || (needs_ac && !ac_tables[c.ta].defined)
                    {
                        return Err(CORRUPT);
                    }
                    c.dc = 0;
                    scan.push(index);
                }
                let parameters = Scan {
                    ss,
                    se,
                    ah,
                    al,
                    progressive,
                };
                let mut bits = Bits::new(bytes, at);
                let mut eobrun = 0u32;
                let mcux = width.div_ceil(8 * usize::from(hmax));
                let mcuy = height.div_ceil(8 * usize::from(vmax));
                // Non-interleaved scans cover only the component's real blocks.
                let (units_x, units_y) = if ns == 1 {
                    let c = &comps[scan[0]];
                    (
                        (width * usize::from(c.h)).div_ceil(8 * usize::from(hmax)),
                        (height * usize::from(c.v)).div_ceil(8 * usize::from(vmax)),
                    )
                } else {
                    (mcux, mcuy)
                };
                let mut count = 0usize;
                for my in 0..units_y {
                    for mx in 0..units_x {
                        if restart_interval > 0
                            && count > 0
                            && count.is_multiple_of(restart_interval)
                        {
                            bits.restart();
                            eobrun = 0;
                            for &i in &scan {
                                comps[i].dc = 0;
                            }
                        }
                        count += 1;
                        for &i in &scan {
                            let c = &mut comps[i];
                            let bh = if ns == 1 { 1 } else { usize::from(c.v) };
                            let bw = if ns == 1 { 1 } else { usize::from(c.h) };
                            for by in 0..bh {
                                for bx in 0..bw {
                                    let (col, row) = (mx * bw + bx, my * bh + by);
                                    let mut scratch = [0i16; 64];
                                    if col < c.bw && row < c.bh {
                                        let start = (row * c.bw + col) * 64;
                                        let mut block: [i16; 64] =
                                            c.coefs[start..start + 64].try_into().unwrap();
                                        let result = decode_block(
                                            &mut bits,
                                            c,
                                            &mut block,
                                            &dc_tables,
                                            &ac_tables,
                                            &parameters,
                                            &mut eobrun,
                                        );
                                        c.coefs[start..start + 64].copy_from_slice(&block);
                                        result?;
                                    } else {
                                        decode_block(
                                            &mut bits,
                                            c,
                                            &mut scratch,
                                            &dc_tables,
                                            &ac_tables,
                                            &parameters,
                                            &mut eobrun,
                                        )?;
                                    }
                                }
                            }
                        }
                    }
                }
                // Continue after the entropy data (next marker).
                at = bits.at;
                while at + 1 < bytes.len()
                    && !(bytes[at] == 0xFF
                        && bytes[at + 1] != 0
                        && !(0xD0..=0xD7).contains(&bytes[at + 1]))
                {
                    at += 1;
                }
            }
            _ => {}
        }
    }
    if !frame {
        return Err(CORRUPT);
    }
    let cos = cosines();
    for c in &mut comps {
        let stride = c.bw * 8;
        let mut coef = [0i32; 64];
        for row in 0..c.bh {
            for col in 0..c.bw {
                let start = (row * c.bw + col) * 64;
                for (i, &v) in c.coefs[start..start + 64].iter().enumerate() {
                    coef[i] = i32::from(v);
                }
                idct(
                    &cos,
                    &coef,
                    &quant[usize::from(c.tq)],
                    &mut c.plane[row * 8 * stride + col * 8..],
                    stride,
                );
            }
        }
    }
    let mut pixels = vec![0u8; width * height * 3];
    for y in 0..height {
        for x in 0..width {
            let mut s = [0f32; 4];
            for (k, c) in comps.iter().enumerate() {
                let sx = x * usize::from(c.h) / usize::from(hmax);
                let sy = y * usize::from(c.v) / usize::from(vmax);
                s[k] = f32::from(c.plane[sy * c.bw * 8 + sx]);
            }
            let out = &mut pixels[(y * width + x) * 3..][..3];
            match comps.len() {
                1 => out.copy_from_slice(&[clamp8(s[0]), clamp8(s[0]), clamp8(s[0])]),
                3 => {
                    if adobe == Some(0) {
                        out.copy_from_slice(&[clamp8(s[0]), clamp8(s[1]), clamp8(s[2])]);
                    } else {
                        out.copy_from_slice(&ycc(s[0], s[1], s[2]));
                    }
                }
                _ => {
                    // Adobe CMYK/YCCK is stored inverted: value = 255 - ink. YCCK (transform 2):
                    // YCbCr→RGB gives ink, so invert it back.
                    let cmy: [u8; 3] = if adobe == Some(2) {
                        let rgb = ycc(s[0], s[1], s[2]);
                        [255 - rgb[0], 255 - rgb[1], 255 - rgb[2]]
                    } else {
                        [clamp8(s[0]), clamp8(s[1]), clamp8(s[2])]
                    };
                    for i in 0..3 {
                        out[i] = clamp8(f32::from(cmy[i]) * s[3] / 255.0);
                    }
                }
            }
        }
    }
    Ok(Image {
        width: width as u32,
        height: height as u32,
        pixels,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn huffman_canonical_codes_decode() {
        // 1 code of length 1 ('a' = 0) and 2 of length 2 ('b' = 10, 'c' = 11).
        let mut counts = [0u8; 16];
        counts[0] = 1;
        counts[1] = 2;
        let h = Huffman::new(counts, b"abc").unwrap();
        let data = [0b0101_1000_u8];
        let mut bits = Bits::new(&data, 0);
        assert_eq!(bits.decode(&h), Ok(b'a'));
        assert_eq!(bits.decode(&h), Ok(b'b'));
        assert_eq!(bits.decode(&h), Ok(b'c'));
    }

    #[test]
    fn constant_dc_block_decodes_to_a_flat_value() {
        let mut coef = [0i32; 64];
        coef[0] = 80; // DC 80 x q1 -> +10 over 128
        let mut out = [0u8; 64];
        idct(&cosines(), &coef, &[1; 64], &mut out, 8);
        assert!(out.iter().all(|&v| v == 138));
    }

    #[test]
    fn header_reads_without_decoding_and_garbage_is_refused() {
        // SOI, SOF2 (progressive) 3x2, one component, no scan: decodes to flat mid-grey.
        let header = [0xFF, 0xD8, 0xFF, 0xC2, 0, 11, 8, 0, 2, 0, 3, 1, 1, 0x11, 0];
        let i = info(&header).unwrap();
        assert_eq!(
            (i.width, i.height, i.progressive, i.components),
            (3, 2, true, 1)
        );
        assert_eq!(decode(&header).unwrap().pixels, [128; 18]);
        assert_eq!(info(b"nope"), Err(ImageError::NotJpeg));
        assert_eq!(decode(&[0xFF, 0xD8, 0xFF]), Err(ImageError::NotJpeg));
        assert_eq!(
            decode(&[0xFF, 0xD8, 0xFF, 0xD9, 0, 0]),
            Err(ImageError::CorruptJpeg)
        );
        // Arithmetic-coded (SOF9) is unsupported, not corrupt.
        assert_eq!(
            info(&[0xFF, 0xD8, 0xFF, 0xC9, 0, 2, 0, 0]),
            Err(ImageError::UnsupportedJpeg)
        );
    }

    #[test]
    fn truncated_and_garbled_streams_never_panic() {
        let header = [
            0xFF, 0xD8, 0xFF, 0xC0, 0, 11, 8, 0, 9, 0, 9, 1, 1, 0x11, 0, 0xFF, 0xDA, 0, 8, 1, 1, 0,
            0, 63, 0, 0xFF, 0xFF, 0x00,
        ];
        for cut in 0..header.len() {
            let _ = decode(&header[..cut]);
        }
        let mut noisy = header.to_vec();
        noisy.extend((0..200u32).map(|i| (i * 37 % 251) as u8));
        let _ = decode(&noisy);
    }
}
