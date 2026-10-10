//! Minimal PNG decoder for inline book images: all standard colour types and
//! bit depths, the five scanline filters, palette transparency. Interlaced
//! (Adam7) images are rejected (callers fall back to the `i` viewer).
//! Output is RGB8; alpha is composited over white paper, which suits book
//! diagrams drawn with dark lines on transparent backgrounds.
const std = @import("std");

pub const Image = struct {
    width: u32,
    height: u32,
    /// RGB8, row-major, width * height * 3 bytes.
    pixels: []u8,

    pub fn deinit(self: Image, allocator: std.mem.Allocator) void {
        allocator.free(self.pixels);
    }
};

pub const Info = struct { width: u32, height: u32, depth: u8, color: u8, interlaced: bool };

const signature = "\x89PNG\r\n\x1a\n";
const max_pixels = 40_000_000;

/// Header fields from IHDR, without decoding any image data.
pub fn info(bytes: []const u8) !Info {
    if (bytes.len < 33 or !std.mem.startsWith(u8, bytes, signature) or !std.mem.eql(u8, bytes[12..16], "IHDR")) return error.NotPng;
    return .{
        .width = std.mem.readInt(u32, bytes[16..20], .big),
        .height = std.mem.readInt(u32, bytes[20..24], .big),
        .depth = bytes[24],
        .color = bytes[25],
        .interlaced = bytes[28] != 0,
    };
}

fn channels(color: u8) !u8 {
    return switch (color) {
        0, 3 => 1,
        2 => 3,
        4 => 2,
        6 => 4,
        else => error.UnsupportedPng,
    };
}

fn validDepth(color: u8, depth: u8) bool {
    return switch (color) {
        0 => depth == 1 or depth == 2 or depth == 4 or depth == 8 or depth == 16,
        3 => depth == 1 or depth == 2 or depth == 4 or depth == 8,
        2, 4, 6 => depth == 8 or depth == 16,
        else => false,
    };
}

fn paeth(a: u8, b: u8, c: u8) u8 {
    const p = @as(i16, a) + b - c;
    const pa = @abs(p - a);
    const pb = @abs(p - b);
    const pc = @abs(p - c);
    if (pa <= pb and pa <= pc) return a;
    if (pb <= pc) return b;
    return c;
}

/// Sample `index` (0-based, in samples) of a scanline at bit depth `depth`,
/// scaled to 8 bits (16-bit samples keep their high byte).
fn sample(line: []const u8, index: usize, depth: u8) u8 {
    return switch (depth) {
        8 => line[index],
        16 => line[index * 2],
        else => blk: {
            const bit = index * depth;
            const shift: u3 = @intCast(8 - depth - (bit % 8));
            const mask: u8 = (@as(u8, 1) << @intCast(depth)) - 1;
            break :blk (line[bit / 8] >> shift) & mask;
        },
    };
}

fn blend(c: u8, a: u8) u8 {
    return @intCast((@as(u32, c) * a + 255 * (255 - @as(u32, a)) + 127) / 255);
}

pub fn decode(allocator: std.mem.Allocator, bytes: []const u8) !Image {
    const header = try info(bytes);
    if (header.interlaced) return error.UnsupportedPng;
    if (!validDepth(header.color, header.depth)) return error.UnsupportedPng;
    const width = header.width;
    const height = header.height;
    if (width == 0 or height == 0 or @as(u64, width) * height > max_pixels) return error.UnsupportedPng;
    var palette: []const u8 = &.{};
    var trns: []const u8 = &.{};
    var idat: std.ArrayList(u8) = .empty;
    defer idat.deinit(allocator);
    var at: usize = 8;
    while (at + 12 <= bytes.len) {
        const len = std.mem.readInt(u32, bytes[at..][0..4], .big);
        const kind = bytes[at + 4 .. at + 8];
        if (at + 12 + @as(usize, len) > bytes.len) return error.CorruptPng;
        const body = bytes[at + 8 ..][0..len];
        at += 12 + @as(usize, len);
        if (std.mem.eql(u8, kind, "PLTE")) palette = body else if (std.mem.eql(u8, kind, "tRNS")) trns = body else if (std.mem.eql(u8, kind, "IDAT")) try idat.appendSlice(allocator, body) else if (std.mem.eql(u8, kind, "IEND")) break;
    }
    if (header.color == 3 and palette.len < 3) return error.CorruptPng;

    const n: usize = try channels(header.color);
    const bits_per_pixel = n * header.depth;
    const stride = (@as(usize, width) * bits_per_pixel + 7) / 8;
    const bpp = @max(1, bits_per_pixel / 8);
    const raw = try allocator.alloc(u8, (stride + 1) * height);
    defer allocator.free(raw);
    var input: std.Io.Reader = .fixed(idat.items);
    var output: std.Io.Writer = .fixed(raw);
    var inflate: std.compress.flate.Decompress = .init(&input, .zlib, &.{});
    _ = inflate.reader.streamRemaining(&output) catch |err| switch (err) {
        error.WriteFailed => {}, // trailing data beyond the image is ignored
        else => return error.CorruptPng,
    };
    if (output.end != raw.len) return error.CorruptPng;

    const pixels = try allocator.alloc(u8, @as(usize, width) * height * 3);
    errdefer allocator.free(pixels);
    const zero = try allocator.alloc(u8, stride);
    defer allocator.free(zero);
    @memset(zero, 0);
    var previous: []const u8 = zero;
    const max_value: u32 = if (header.depth >= 8) 255 else (@as(u32, 1) << @intCast(header.depth)) - 1;
    for (0..height) |y| {
        const line = raw[y * (stride + 1) ..][0 .. stride + 1];
        const current = line[1..];
        switch (line[0]) {
            0 => {},
            1 => for (bpp..stride) |i| {
                current[i] +%= current[i - bpp];
            },
            2 => for (0..stride) |i| {
                current[i] +%= previous[i];
            },
            3 => for (0..stride) |i| {
                const left: u16 = if (i >= bpp) current[i - bpp] else 0;
                current[i] +%= @intCast((left + previous[i]) / 2);
            },
            4 => for (0..stride) |i| {
                const left = if (i >= bpp) current[i - bpp] else 0;
                const corner = if (i >= bpp) previous[i - bpp] else 0;
                current[i] +%= paeth(left, previous[i], corner);
            },
            else => return error.CorruptPng,
        }
        previous = current;
        const out = pixels[y * @as(usize, width) * 3 ..];
        for (0..width) |x| {
            var rgb: [3]u8 = undefined;
            var alpha: u8 = 255;
            switch (header.color) {
                0, 4 => {
                    const v: u8 = @intCast(@as(u32, sample(current, x * n, header.depth)) * 255 / max_value);
                    rgb = .{ v, v, v };
                    if (header.color == 4) alpha = sample(current, x * n + 1, header.depth);
                },
                2, 6 => {
                    rgb = .{ sample(current, x * n, header.depth), sample(current, x * n + 1, header.depth), sample(current, x * n + 2, header.depth) };
                    if (header.color == 6) alpha = sample(current, x * n + 3, header.depth);
                },
                3 => {
                    const index = sample(current, x, header.depth);
                    if (@as(usize, index) * 3 + 2 >= palette.len) return error.CorruptPng;
                    rgb = palette[@as(usize, index) * 3 ..][0..3].*;
                    if (index < trns.len) alpha = trns[index];
                },
                else => unreachable,
            }
            for (0..3) |c| out[x * 3 + c] = if (alpha == 255) rgb[c] else blend(rgb[c], alpha);
        }
    }
    return .{ .width = width, .height = height, .pixels = pixels };
}

/// Area-averaging resize (box filter); each source pixel is read about once.
pub fn scale(allocator: std.mem.Allocator, image: Image, width: u32, height: u32) !Image {
    const w = @max(1, width);
    const h = @max(1, height);
    const pixels = try allocator.alloc(u8, @as(usize, w) * h * 3);
    for (0..h) |y| {
        const y0 = y * image.height / h;
        const y1 = @max(y0 + 1, (y + 1) * image.height / h);
        for (0..w) |x| {
            const x0 = x * image.width / w;
            const x1 = @max(x0 + 1, (x + 1) * image.width / w);
            var sum: [3]u32 = .{ 0, 0, 0 };
            for (y0..y1) |sy| for (x0..x1) |sx| {
                const p = (sy * image.width + sx) * 3;
                for (0..3) |c| sum[c] += image.pixels[p + c];
            };
            const count: u32 = @intCast((y1 - y0) * (x1 - x0));
            for (0..3) |c| pixels[(y * w + x) * 3 + c] = @intCast((sum[c] + count / 2) / count);
        }
    }
    return .{ .width = w, .height = h, .pixels = pixels };
}

// ---------------------------------------------------------------------------
// Tests build PNGs by hand: zlib with stored (uncompressed) deflate blocks.

fn chunk(list: *std.ArrayList(u8), a: std.mem.Allocator, kind: []const u8, body: []const u8) !void {
    var len: [4]u8 = undefined;
    std.mem.writeInt(u32, &len, @intCast(body.len), .big);
    try list.appendSlice(a, &len);
    try list.appendSlice(a, kind);
    try list.appendSlice(a, body);
    var crc = std.hash.Crc32.init();
    crc.update(kind);
    crc.update(body);
    std.mem.writeInt(u32, &len, crc.final(), .big);
    try list.appendSlice(a, &len);
}

fn testPng(a: std.mem.Allocator, width: u32, height: u32, depth: u8, color: u8, raw: []const u8, plte: []const u8, trns: []const u8) ![]u8 {
    var out: std.ArrayList(u8) = .empty;
    try out.appendSlice(a, signature);
    var ihdr: [13]u8 = undefined;
    std.mem.writeInt(u32, ihdr[0..4], width, .big);
    std.mem.writeInt(u32, ihdr[4..8], height, .big);
    ihdr[8] = depth;
    ihdr[9] = color;
    ihdr[10] = 0;
    ihdr[11] = 0;
    ihdr[12] = 0;
    try chunk(&out, a, "IHDR", &ihdr);
    if (plte.len > 0) try chunk(&out, a, "PLTE", plte);
    if (trns.len > 0) try chunk(&out, a, "tRNS", trns);
    var z: std.ArrayList(u8) = .empty;
    try z.appendSlice(a, &.{ 0x78, 0x01, 0x01 });
    var le: [2]u8 = undefined;
    std.mem.writeInt(u16, &le, @intCast(raw.len), .little);
    try z.appendSlice(a, &le);
    std.mem.writeInt(u16, &le, ~@as(u16, @intCast(raw.len)), .little);
    try z.appendSlice(a, &le);
    try z.appendSlice(a, raw);
    var adler: [4]u8 = undefined;
    std.mem.writeInt(u32, &adler, std.hash.Adler32.hash(raw), .big);
    try z.appendSlice(a, &adler);
    try chunk(&out, a, "IDAT", z.items);
    try chunk(&out, a, "IEND", "");
    return out.items;
}

test "all five filters reconstruct RGB scanlines" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    // 2×5 RGB: each row uses a different filter over the same target pixels
    // [10,20,30][40,50,60]; rows after the first see the same row above.
    const raw = [_]u8{
        0, 10, 20, 30, 40, 50, 60, // none
        1, 10, 20, 30, 30, 30, 30, // sub
        2, 0, 0, 0, 0, 0, 0, // up
        3, 5, 10, 15, 15, 15, 15, // average: (left+up)/2
        4, 0, 0, 0, 0, 0, 0, // paeth picks up
    };
    const image = try decode(a, try testPng(a, 2, 5, 8, 2, &raw, "", ""));
    for (0..5) |y| try std.testing.expectEqualSlices(u8, &.{ 10, 20, 30, 40, 50, 60 }, image.pixels[y * 6 ..][0..6]);
}

test "palette, low bit depth, transparency, and grayscale alpha" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    // 4×1, 2-bit palette indices 0,1,2,3; index 1 fully transparent → white.
    const plte = [_]u8{ 0, 0, 0, 255, 0, 0, 0, 255, 0, 0, 0, 255 };
    const image = try decode(a, try testPng(a, 4, 1, 2, 3, &.{ 0, 0b00011011 }, &plte, &.{ 255, 0 }));
    try std.testing.expectEqualSlices(u8, &.{ 0, 0, 0, 255, 255, 255, 0, 255, 0, 0, 0, 255 }, image.pixels);
    // 1-bit grayscale: 1 → white, 0 → black.
    const bits = try decode(a, try testPng(a, 3, 1, 1, 0, &.{ 0, 0b10100000 }, "", ""));
    try std.testing.expectEqualSlices(u8, &.{ 255, 255, 255, 0, 0, 0, 255, 255, 255 }, bits.pixels);
    // Gray+alpha: black at alpha 0 composites to white paper.
    const ga = try decode(a, try testPng(a, 1, 1, 8, 4, &.{ 0, 0, 0 }, "", ""));
    try std.testing.expectEqualSlices(u8, &.{ 255, 255, 255 }, ga.pixels);
    try std.testing.expectError(error.NotPng, decode(a, "GIF89a not a png at all......................"));
}

test "area scaling averages blocks" {
    var px = [_]u8{ 0, 0, 0, 255, 255, 255, 255, 255, 255, 0, 0, 0 };
    const image: Image = .{ .width = 2, .height = 2, .pixels = &px };
    const half = try scale(std.testing.allocator, image, 1, 1);
    defer half.deinit(std.testing.allocator);
    try std.testing.expectEqualSlices(u8, &.{ 128, 128, 128 }, half.pixels);
}

test "every PNG in the sample books decodes" {
    const io = std.testing.io;
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, "examplePubs/buildingmicroservices2ndedition.epub", a, .limited(64 << 20)) catch |err| switch (err) {
        error.FileNotFound => return error.SkipZigTest,
        else => return err,
    };
    const archive = try @import("epub.zig").Archive.init(a, bytes);
    var decoded: usize = 0;
    for (archive.entries) |entry| {
        if (!std.ascii.endsWithIgnoreCase(entry.name, ".png")) continue;
        var scratch = std.heap.ArenaAllocator.init(std.testing.allocator);
        defer scratch.deinit();
        const data = try archive.read(scratch.allocator(), entry.name);
        const image = try decode(scratch.allocator(), data);
        try std.testing.expectEqual((try info(data)).width, image.width);
        decoded += 1;
    }
    try std.testing.expect(decoded >= 180);
}
