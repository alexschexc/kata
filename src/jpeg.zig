//! Baseline JPEG decoder for inline book images: Huffman-coded sequential
//! DCT (SOF0/SOF1) and progressive (SOF2), 8-bit samples, any sampling factors, restart intervals,
//! grayscale, YCbCr, and Adobe CMYK/YCCK. Arithmetic-coded and lossless
//! files return error.UnsupportedJpeg (the reader keeps the `i` fallback).
//! Output reuses png.Image (RGB8) so scaling and sixel encoding are shared.
const std = @import("std");
const Image = @import("png.zig").Image;

const max_pixels = 40_000_000;

const zigzag = [64]u8{
    0,  1,  8,  16, 9,  2,  3,  10, 17, 24, 32, 25, 18, 11, 4,  5,
    12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6,  7,  14, 21, 28,
    35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51,
    58, 59, 52, 45, 38, 31, 39, 46, 53, 60, 61, 54, 47, 55, 62, 63,
};

pub const Info = struct { width: u32, height: u32, progressive: bool, components: u8 };

/// Frame header fields without decoding (for layout before display).
pub fn info(bytes: []const u8) !Info {
    if (bytes.len < 4 or bytes[0] != 0xFF or bytes[1] != 0xD8) return error.NotJpeg;
    var at: usize = 2;
    while (at + 4 <= bytes.len) {
        if (bytes[at] != 0xFF) return error.CorruptJpeg;
        const marker = bytes[at + 1];
        if (marker == 0xFF) {
            at += 1;
            continue;
        }
        const len = std.mem.readInt(u16, bytes[at + 2 ..][0..2], .big);
        if (at + 2 + len > bytes.len) return error.CorruptJpeg;
        switch (marker) {
            0xC0, 0xC1, 0xC2 => {
                if (len < 8) return error.CorruptJpeg;
                return .{
                    .height = std.mem.readInt(u16, bytes[at + 5 ..][0..2], .big),
                    .width = std.mem.readInt(u16, bytes[at + 7 ..][0..2], .big),
                    .progressive = marker == 0xC2,
                    .components = bytes[at + 9],
                };
            },
            0xC3, 0xC5...0xC7, 0xC9...0xCB, 0xCD...0xCF => return error.UnsupportedJpeg,
            0xDA, 0xD9 => return error.CorruptJpeg,
            else => at += 2 + len,
        }
    }
    return error.CorruptJpeg;
}

const Huffman = struct {
    /// Canonical decoding tables (JPEG Annex F.2.2.3).
    maxcode: [18]i32 = @splat(-1),
    valptr: [17]i32 = @splat(0),
    mincode: [17]i32 = @splat(0),
    values: [256]u8 = undefined,
    defined: bool = false,

    fn init(counts: [16]u8, values: []const u8) !Huffman {
        var h: Huffman = .{ .defined = true };
        var total: usize = 0;
        for (counts) |c| total += c;
        if (total > 256 or total > values.len) return error.CorruptJpeg;
        @memcpy(h.values[0..total], values[0..total]);
        var code: i32 = 0;
        var k: i32 = 0;
        for (1..17) |l| {
            const n = counts[l - 1];
            if (n == 0) {
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
        h.maxcode[17] = std.math.maxInt(i32);
        return h;
    }
};

const Bits = struct {
    data: []const u8,
    at: usize,
    acc: u32 = 0,
    count: u5 = 0,
    /// A marker was reached; further reads yield zero bits.
    hit_marker: bool = false,

    fn byte(self: *Bits) u8 {
        if (self.hit_marker or self.at >= self.data.len) return 0;
        const b = self.data[self.at];
        if (b == 0xFF) {
            const next = if (self.at + 1 < self.data.len) self.data[self.at + 1] else 0xD9;
            if (next == 0x00) {
                self.at += 2;
                return 0xFF;
            }
            self.hit_marker = true;
            return 0;
        }
        self.at += 1;
        return b;
    }

    fn bit(self: *Bits) u1 {
        if (self.count == 0) {
            self.acc = self.byte();
            self.count = 8;
        }
        self.count -= 1;
        return @intCast((self.acc >> self.count) & 1);
    }

    fn receive(self: *Bits, n: u5) i32 {
        var v: i32 = 0;
        for (0..n) |_| v = (v << 1) | self.bit();
        return v;
    }

    fn decode(self: *Bits, h: *const Huffman) !u8 {
        var code: i32 = self.bit();
        var l: usize = 1;
        while (l <= 16) : (l += 1) {
            if (code <= h.maxcode[l]) return h.values[@intCast(h.valptr[l] + code - h.mincode[l])];
            code = (code << 1) | self.bit();
        }
        return error.CorruptJpeg;
    }

    /// Skips to just past an RSTn marker and resets the bit buffer.
    fn restart(self: *Bits) void {
        self.count = 0;
        self.hit_marker = false;
        while (self.at + 1 < self.data.len) {
            if (self.data[self.at] == 0xFF and self.data[self.at + 1] >= 0xD0 and self.data[self.at + 1] <= 0xD7) {
                self.at += 2;
                return;
            }
            self.at += 1;
        }
    }
};

fn extend(v: i32, n: u5) i32 {
    if (n == 0) return 0;
    return if (v < (@as(i32, 1) << (n - 1))) v - (@as(i32, 1) << n) + 1 else v;
}

const Component = struct {
    id: u8,
    h: u8,
    v: u8,
    tq: u8,
    td: u8 = 0,
    ta: u8 = 0,
    dc: i32 = 0,
    /// Blocks across/down the padded plane.
    bw: usize = 0,
    bh: usize = 0,
    plane: []u8 = &.{},
    /// Dequantization-free coefficients per block (natural order), kept
    /// across scans so progressive refinement can accumulate.
    coefs: []i16 = &.{},
};

fn put(coef: *i16, v: i32) void {
    coef.* = @intCast(std.math.clamp(v, -32768, 32767));
}

/// One block of one scan (baseline or any progressive pass).
fn decodeBlock(bits: *Bits, c: *Component, block: *[64]i16, dc_tables: *const [4]Huffman, ac_tables: *const [4]Huffman, ss: u8, se: u8, ah: u8, al: u5, progressive: bool, eobrun: *u32) !void {
    if (!progressive or ss == 0) {
        if (ah == 0 or !progressive) {
            const t = try bits.decode(&dc_tables[c.td]);
            if (t > 16) return error.CorruptJpeg;
            c.dc += extend(bits.receive(@intCast(t)), @intCast(t));
            put(&block[0], c.dc * (@as(i32, 1) << al));
        } else if (bits.bit() == 1) {
            block[0] |= @as(i16, 1) << @intCast(al);
        }
        if (progressive) return;
        // Baseline: AC follows in the same block.
        var k: usize = 1;
        while (k < 64) {
            const rs = try bits.decode(&ac_tables[c.ta]);
            const r = rs >> 4;
            const sz: u5 = @intCast(rs & 15);
            if (sz == 0) {
                if (r != 15) break;
                k += 16;
                continue;
            }
            k += r;
            if (k > 63) return error.CorruptJpeg;
            put(&block[zigzag[k]], extend(bits.receive(sz), sz));
            k += 1;
        }
        return;
    }
    if (ah == 0) { // AC first pass
        if (eobrun.* > 0) {
            eobrun.* -= 1;
            return;
        }
        var k: usize = ss;
        while (k <= se) : (k += 1) {
            const rs = try bits.decode(&ac_tables[c.ta]);
            const r = rs >> 4;
            const sz: u5 = @intCast(rs & 15);
            if (sz != 0) {
                k += r;
                if (k > 63) return error.CorruptJpeg;
                put(&block[zigzag[k]], extend(bits.receive(sz), sz) * (@as(i32, 1) << al));
            } else if (r == 15) {
                k += 15;
            } else {
                eobrun.* = (@as(u32, 1) << @intCast(r)) - 1;
                if (r > 0) eobrun.* += @intCast(bits.receive(@intCast(r)));
                break;
            }
        }
        return;
    }
    // AC refinement (libjpeg decode_mcu_AC_refine).
    const p1: i32 = @as(i32, 1) << al;
    const m1: i32 = -p1;
    var k: usize = ss;
    if (eobrun.* == 0) {
        while (k <= se) : (k += 1) {
            const rs = try bits.decode(&ac_tables[c.ta]);
            var r: i32 = rs >> 4;
            var value: i32 = 0;
            if (rs & 15 != 0) {
                if (rs & 15 != 1) return error.CorruptJpeg;
                value = if (bits.bit() == 1) p1 else m1;
            } else if (r != 15) {
                eobrun.* = @as(u32, 1) << @intCast(r);
                if (r > 0) eobrun.* += @intCast(bits.receive(@intCast(r)));
                break;
            }
            while (k <= se) {
                const z = &block[zigzag[k]];
                if (z.* != 0) {
                    if (bits.bit() == 1 and (z.* & @as(i16, @intCast(p1))) == 0) put(z, @as(i32, z.*) + (if (z.* >= 0) p1 else m1));
                } else {
                    r -= 1;
                    if (r < 0) break;
                }
                k += 1;
            }
            if (value != 0) {
                if (k > 63) return error.CorruptJpeg;
                put(&block[zigzag[k]], value);
            }
        }
    }
    if (eobrun.* > 0) {
        while (k <= se) : (k += 1) {
            const z = &block[zigzag[k]];
            if (z.* != 0 and bits.bit() == 1 and (z.* & @as(i16, @intCast(p1))) == 0) put(z, @as(i32, z.*) + (if (z.* >= 0) p1 else m1));
        }
        eobrun.* -= 1;
    }
}

const cosines = blk: {
    @setEvalBranchQuota(10000);
    var table: [8][8]f32 = undefined;
    for (0..8) |x| for (0..8) |u| {
        const cu: f32 = if (u == 0) 1.0 / @sqrt(2.0) else 1.0;
        table[x][u] = cu * @cos(@as(f32, @floatFromInt((2 * x + 1) * u)) * std.math.pi / 16.0) / 2.0;
    };
    break :blk table;
};

fn idct(coef: *const [64]i32, q: *const [64]u16, out: []u8, stride: usize) void {
    var f: [64]f32 = undefined;
    for (0..64) |i| f[i] = @floatFromInt(coef[i] * @as(i32, q[i]));
    var tmp: [64]f32 = undefined;
    for (0..8) |y| for (0..8) |x| {
        var s: f32 = 0;
        for (0..8) |u| s += cosines[x][u] * f[y * 8 + u];
        tmp[y * 8 + x] = s;
    };
    for (0..8) |x| for (0..8) |y| {
        var s: f32 = 0;
        for (0..8) |v| s += cosines[y][v] * tmp[v * 8 + x];
        const value = @round(s + 128);
        out[y * stride + x] = @intFromFloat(std.math.clamp(value, 0, 255));
    };
}

fn clamp8(v: f32) u8 {
    return @intFromFloat(std.math.clamp(@round(v), 0, 255));
}

pub fn decode(allocator: std.mem.Allocator, bytes: []const u8) !Image {
    if (bytes.len < 4 or bytes[0] != 0xFF or bytes[1] != 0xD8) return error.NotJpeg;
    var quant: [4][64]u16 = @splat(@splat(1));
    var dc_tables: [4]Huffman = @splat(.{});
    var ac_tables: [4]Huffman = @splat(.{});
    var comps: [4]Component = undefined;
    var ncomp: usize = 0;
    var width: usize = 0;
    var height: usize = 0;
    var hmax: u8 = 1;
    var vmax: u8 = 1;
    var restart_interval: usize = 0;
    var adobe: ?u8 = null;
    var frame = false;
    var progressive = false;
    defer for (comps[0..ncomp]) |c| {
        allocator.free(c.plane);
        allocator.free(c.coefs);
    };
    var at: usize = 2;
    scan_loop: while (at + 4 <= bytes.len) {
        if (bytes[at] != 0xFF) return error.CorruptJpeg;
        const marker = bytes[at + 1];
        if (marker == 0xFF) {
            at += 1;
            continue;
        }
        if (marker == 0xD9) break;
        const len = std.mem.readInt(u16, bytes[at + 2 ..][0..2], .big);
        if (len < 2 or at + 2 + len > bytes.len) return error.CorruptJpeg;
        const seg = bytes[at + 4 .. at + 2 + len];
        at += 2 + len;
        switch (marker) {
            0xDB => {
                var i: usize = 0;
                while (i < seg.len) {
                    const precision = seg[i] >> 4;
                    const id = seg[i] & 15;
                    if (id > 3) return error.CorruptJpeg;
                    i += 1;
                    for (0..64) |k| {
                        if (precision == 0) {
                            if (i >= seg.len) return error.CorruptJpeg;
                            quant[id][zigzag[k]] = seg[i];
                            i += 1;
                        } else {
                            if (i + 1 >= seg.len) return error.CorruptJpeg;
                            quant[id][zigzag[k]] = std.mem.readInt(u16, seg[i..][0..2], .big);
                            i += 2;
                        }
                    }
                }
            },
            0xC4 => {
                var i: usize = 0;
                while (i + 17 <= seg.len) {
                    const class = seg[i] >> 4;
                    const id = seg[i] & 15;
                    if (id > 3 or class > 1) return error.CorruptJpeg;
                    const counts = seg[i + 1 ..][0..16].*;
                    var total: usize = 0;
                    for (counts) |c| total += c;
                    if (i + 17 + total > seg.len) return error.CorruptJpeg;
                    const table = try Huffman.init(counts, seg[i + 17 ..][0..total]);
                    if (class == 0) dc_tables[id] = table else ac_tables[id] = table;
                    i += 17 + total;
                }
            },
            0xDD => {
                if (seg.len < 2) return error.CorruptJpeg;
                restart_interval = std.mem.readInt(u16, seg[0..2], .big);
            },
            0xEE => if (seg.len >= 12 and std.mem.startsWith(u8, seg, "Adobe")) {
                adobe = seg[11];
            },
            0xC0, 0xC1, 0xC2 => {
                if (seg.len < 6 or seg[0] != 8) return error.UnsupportedJpeg;
                progressive = marker == 0xC2;
                height = std.mem.readInt(u16, seg[1..3], .big);
                width = std.mem.readInt(u16, seg[3..5], .big);
                const n = seg[5];
                if (width == 0 or height == 0 or width * height > max_pixels) return error.UnsupportedJpeg;
                if (n != 1 and n != 3 and n != 4) return error.UnsupportedJpeg;
                if (seg.len < 6 + @as(usize, n) * 3) return error.CorruptJpeg;
                for (0..n) |k| {
                    const s = seg[6 + k * 3 ..][0..3];
                    const h = s[1] >> 4;
                    const v = s[1] & 15;
                    if (h < 1 or h > 4 or v < 1 or v > 4 or s[2] > 3) return error.UnsupportedJpeg;
                    comps[k] = .{ .id = s[0], .h = h, .v = v, .tq = s[2] };
                    hmax = @max(hmax, h);
                    vmax = @max(vmax, v);
                }
                const mcux = (width + 8 * @as(usize, hmax) - 1) / (8 * @as(usize, hmax));
                const mcuy = (height + 8 * @as(usize, vmax) - 1) / (8 * @as(usize, vmax));
                for (0..n) |k| {
                    comps[k].bw = mcux * comps[k].h;
                    comps[k].bh = mcuy * comps[k].v;
                    comps[k].plane = try allocator.alloc(u8, comps[k].bw * comps[k].bh * 64);
                    comps[k].coefs = try allocator.alloc(i16, comps[k].bw * comps[k].bh * 64);
                    @memset(comps[k].coefs, 0);
                    ncomp = k + 1;
                }
                frame = true;
            },
            0xC3, 0xC5...0xC7, 0xC9...0xCB, 0xCD...0xCF => return error.UnsupportedJpeg,
            0xDA => {
                if (!frame or seg.len < 1) return error.CorruptJpeg;
                const ns = seg[0];
                if (ns == 0 or ns > ncomp or seg.len < 1 + @as(usize, ns) * 2 + 3) return error.CorruptJpeg;
                const tail = seg[1 + @as(usize, ns) * 2 ..];
                const ss = tail[0];
                const se = tail[1];
                const ah = tail[2] >> 4;
                const al: u5 = @intCast(tail[2] & 15);
                if (progressive and (se > 63 or ss > se or (ss > 0 and ns != 1) or (ss == 0 and se != 0) or al > 13)) return error.CorruptJpeg;
                var scan: [4]*Component = undefined;
                for (0..ns) |k| {
                    const id = seg[1 + k * 2];
                    const tables = seg[2 + k * 2];
                    const c = for (comps[0..ncomp]) |*c| {
                        if (c.id == id) break c;
                    } else return error.CorruptJpeg;
                    c.td = tables >> 4;
                    c.ta = tables & 15;
                    if (c.td > 3 or c.ta > 3) return error.CorruptJpeg;
                    const needs_dc = !progressive or (ss == 0 and ah == 0);
                    const needs_ac = !progressive or ss > 0;
                    if ((needs_dc and !dc_tables[c.td].defined) or (needs_ac and !ac_tables[c.ta].defined)) return error.CorruptJpeg;
                    c.dc = 0;
                    scan[k] = c;
                }
                var bits: Bits = .{ .data = bytes, .at = at };
                var eobrun: u32 = 0;
                const mcux = (width + 8 * @as(usize, hmax) - 1) / (8 * @as(usize, hmax));
                const mcuy = (height + 8 * @as(usize, vmax) - 1) / (8 * @as(usize, vmax));
                // Non-interleaved scans cover only the component's real blocks.
                const units_x = if (ns == 1) (width * scan[0].h + 8 * @as(usize, hmax) - 1) / (8 * @as(usize, hmax)) else mcux;
                const units_y = if (ns == 1) (height * scan[0].v + 8 * @as(usize, vmax) - 1) / (8 * @as(usize, vmax)) else mcuy;
                var count: usize = 0;
                for (0..units_y) |my| for (0..units_x) |mx| {
                    if (restart_interval > 0 and count > 0 and count % restart_interval == 0) {
                        bits.restart();
                        eobrun = 0;
                        for (scan[0..ns]) |c| c.dc = 0;
                    }
                    count += 1;
                    for (scan[0..ns]) |c| {
                        const bh: usize = if (ns == 1) 1 else c.v;
                        const bw: usize = if (ns == 1) 1 else c.h;
                        for (0..bh) |by| for (0..bw) |bx| {
                            const col = mx * bw + bx;
                            const row = my * bh + by;
                            var scratch: [64]i16 = @splat(0);
                            const block: *[64]i16 = if (col < c.bw and row < c.bh) c.coefs[(row * c.bw + col) * 64 ..][0..64] else &scratch;
                            try decodeBlock(&bits, c, block, &dc_tables, &ac_tables, ss, se, ah, al, progressive, &eobrun);
                        };
                    }
                };
                // Continue after the entropy data (next marker).
                at = bits.at;
                while (at + 1 < bytes.len and !(bytes[at] == 0xFF and bytes[at + 1] != 0 and (bytes[at + 1] < 0xD0 or bytes[at + 1] > 0xD7))) at += 1;
                continue :scan_loop;
            },
            else => {},
        }
    }
    if (!frame) return error.CorruptJpeg;
    for (comps[0..ncomp]) |c| {
        const stride = c.bw * 8;
        var coef: [64]i32 = undefined;
        for (0..c.bh) |row| for (0..c.bw) |col| {
            for (c.coefs[(row * c.bw + col) * 64 ..][0..64], 0..) |v, i| coef[i] = v;
            idct(&coef, &quant[c.tq], c.plane[row * 8 * stride + col * 8 ..], stride);
        };
    }
    const pixels = try allocator.alloc(u8, width * height * 3);
    errdefer allocator.free(pixels);
    for (0..height) |y| for (0..width) |x| {
        var s: [4]f32 = undefined;
        for (comps[0..ncomp], 0..) |c, k| {
            const sx = x * c.h / hmax;
            const sy = y * c.v / vmax;
            s[k] = @floatFromInt(c.plane[sy * c.bw * 8 + sx]);
        }
        const out = pixels[(y * width + x) * 3 ..][0..3];
        switch (ncomp) {
            1 => out.* = .{ clamp8(s[0]), clamp8(s[0]), clamp8(s[0]) },
            3 => if (adobe != null and adobe.? == 0) {
                out.* = .{ clamp8(s[0]), clamp8(s[1]), clamp8(s[2]) };
            } else {
                out.* = ycc(s[0], s[1], s[2]);
            },
            4 => {
                // Adobe CMYK/YCCK is stored inverted: value = 255 - ink.
                // YCCK (transform 2): YCbCr→RGB gives ink, so invert it back.
                const cmy: [3]u8 = if (adobe != null and adobe.? == 2) blk: {
                    const rgb = ycc(s[0], s[1], s[2]);
                    break :blk .{ 255 - rgb[0], 255 - rgb[1], 255 - rgb[2] };
                } else .{ clamp8(s[0]), clamp8(s[1]), clamp8(s[2]) };
                const k = s[3];
                for (0..3) |i| out[i] = clamp8(@as(f32, @floatFromInt(cmy[i])) * k / 255.0);
            },
            else => unreachable,
        }
    };
    return .{ .width = @intCast(width), .height = @intCast(height), .pixels = pixels };
}

fn ycc(y: f32, cb: f32, cr: f32) [3]u8 {
    return .{ clamp8(y + 1.402 * (cr - 128)), clamp8(y - 0.344136 * (cb - 128) - 0.714136 * (cr - 128)), clamp8(y + 1.772 * (cb - 128)) };
}

test "huffman canonical codes decode" {
    // Two 1-bit codes and one 2-bit code: 0→'a', 10→'b', 11→'c'? (1 code len1, 2 codes len2)
    var counts: [16]u8 = @splat(0);
    counts[0] = 1;
    counts[1] = 2;
    const h = try Huffman.init(counts, "abc");
    var bits: Bits = .{ .data = &.{0b0_10_11_000}, .at = 0 };
    try std.testing.expectEqual(@as(u8, 'a'), try bits.decode(&h));
    try std.testing.expectEqual(@as(u8, 'b'), try bits.decode(&h));
    try std.testing.expectEqual(@as(u8, 'c'), try bits.decode(&h));
}

test "constant DC block decodes to a flat value" {
    var coef: [64]i32 = @splat(0);
    coef[0] = 80; // DC 80 × q1 → +10 over 128
    const q: [64]u16 = @splat(1);
    var out: [64]u8 = undefined;
    idct(&coef, &q, &out, 8);
    for (out) |v| try std.testing.expectEqual(@as(u8, 138), v);
}

test "sample-book baseline JPEGs decode with plausible content" {
    const io = std.testing.io;
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, "examplePubs/thinklikeaprogrammer.epub", a, .limited(64 << 20)) catch |err| switch (err) {
        error.FileNotFound => return error.SkipZigTest,
        else => return err,
    };
    const archive = try @import("epub.zig").Archive.init(a, bytes);
    var decoded: usize = 0;
    var unsupported: usize = 0;
    for (archive.entries) |entry| {
        if (!std.ascii.endsWithIgnoreCase(entry.name, ".jpg") and !std.ascii.endsWithIgnoreCase(entry.name, ".jpeg")) continue;
        var scratch = std.heap.ArenaAllocator.init(std.testing.allocator);
        defer scratch.deinit();
        const data = try archive.read(scratch.allocator(), entry.name);
        const header = try info(data);
        if (header.progressive) unsupported += 1; // counted: now decoded too
        const image = try decode(scratch.allocator(), data);
        try std.testing.expectEqual(header.width, image.width);
        // Not all one colour: real content survived decoding.
        var min: u8 = 255;
        var max: u8 = 0;
        for (image.pixels) |p| {
            min = @min(min, p);
            max = @max(max, p);
        }
        try std.testing.expect(max - min > 40);
        decoded += 1;
    }
    try std.testing.expect(decoded >= 50);
    try std.testing.expectEqual(@as(usize, 2), unsupported);
}
