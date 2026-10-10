//! Sixel encoder for RGB8 bitmaps. The palette is built per image from its
//! own colours: pixels are bucketed at 5 bits per channel, the most frequent
//! buckets (up to 256) become palette entries at their average colour, and
//! every other bucket maps to its nearest entry. Book diagrams use a handful
//! of colours, so they are reproduced closely; no dithering keeps text sharp.
//! Runs of four or more identical sixels use DECGRI (`!n`).
const std = @import("std");

const buckets = 1 << 15;

fn bucketOf(p: []const u8) u16 {
    return (@as(u16, p[0] >> 3) << 10) | (@as(u16, p[1] >> 3) << 5) | (p[2] >> 3);
}

pub const Palette = struct {
    colors: [256][3]u8 = undefined,
    len: usize = 0,
    /// bucket → palette index
    map: []u8,

    pub fn deinit(self: Palette, allocator: std.mem.Allocator) void {
        allocator.free(self.map);
    }
};

/// Builds the palette for rows [top, top + height) of the bitmap.
pub fn palette(allocator: std.mem.Allocator, pixels: []const u8, width: usize, top: usize, height: usize) !Palette {
    const counts = try allocator.alloc(u32, buckets);
    defer allocator.free(counts);
    const sums = try allocator.alloc([3]u32, buckets);
    defer allocator.free(sums);
    @memset(counts, 0);
    @memset(sums, .{ 0, 0, 0 });
    var at = top * width * 3;
    const end = (top + height) * width * 3;
    while (at < end) : (at += 3) {
        const b = bucketOf(pixels[at..][0..3]);
        counts[b] += 1;
        for (0..3) |c| sums[b][c] += pixels[at + c];
    }
    var order: std.ArrayList(u16) = .empty;
    defer order.deinit(allocator);
    for (counts, 0..) |n, b| if (n > 0) try order.append(allocator, @intCast(b));
    std.mem.sort(u16, order.items, counts, struct {
        fn more(c: []u32, a: u16, b: u16) bool {
            return c[a] > c[b] or (c[a] == c[b] and a < b);
        }
    }.more);
    var result: Palette = .{ .map = try allocator.alloc(u8, buckets) };
    result.len = @min(256, order.items.len);
    for (order.items[0..result.len], 0..) |b, i| {
        for (0..3) |c| result.colors[i][c] = @intCast(sums[b][c] / counts[b]);
        result.map[b] = @intCast(i);
    }
    for (order.items[result.len..]) |b| {
        var best: usize = 0;
        var best_d: u32 = std.math.maxInt(u32);
        for (result.colors[0..result.len], 0..) |color, i| {
            var d: u32 = 0;
            for (0..3) |c| {
                const delta = @as(i32, @intCast(sums[b][c] / counts[b])) - color[c];
                d += @intCast(delta * delta);
            }
            if (d < best_d) {
                best_d = d;
                best = i;
            }
        }
        result.map[b] = @intCast(best);
    }
    return result;
}

fn run(w: *std.Io.Writer, char: u8, count: usize) !void {
    if (count >= 4) {
        try w.print("!{d}{c}", .{ count, char });
    } else for (0..count) |_| try w.writeByte(char);
}

/// Encodes rows [top, top + height) of an RGB8 bitmap `width` pixels wide as
/// a complete sixel sequence (DCS … ST). `height` should be a multiple of 6
/// so the image never extends below the rows reserved for it.
pub fn encode(allocator: std.mem.Allocator, w: *std.Io.Writer, pixels: []const u8, width: usize, top: usize, height: usize) !void {
    const pal = try palette(allocator, pixels, width, top, height);
    defer pal.deinit(allocator);
    const indexes = try allocator.alloc(u8, width * height);
    defer allocator.free(indexes);
    for (0..width * height) |i| indexes[i] = pal.map[bucketOf(pixels[(top * width + i) * 3 ..][0..3])];
    try w.print("\x1bP0;1;0q\"1;1;{d};{d}", .{ width, height });
    for (pal.colors[0..pal.len], 0..) |c, i| {
        try w.print("#{d};2;{d};{d};{d}", .{ i, (@as(u32, c[0]) * 100 + 127) / 255, (@as(u32, c[1]) * 100 + 127) / 255, (@as(u32, c[2]) * 100 + 127) / 255 });
    }
    var band: usize = 0;
    while (band < height) : (band += 6) {
        const rows = @min(6, height - band);
        var present: [256]bool = @splat(false);
        for (0..rows) |r| for (indexes[(band + r) * width ..][0..width]) |index| {
            present[index] = true;
        };
        var first = true;
        for (present, 0..) |on, color| if (on) {
            if (!first) try w.writeByte('$');
            first = false;
            try w.print("#{d}", .{color});
            var previous: u8 = 0;
            var count: usize = 0;
            for (0..width) |x| {
                var bits: u8 = 0;
                for (0..rows) |r| {
                    if (indexes[(band + r) * width + x] == color) bits |= @as(u8, 1) << @intCast(r);
                }
                const char = 63 + bits;
                if (count > 0 and char == previous) {
                    count += 1;
                } else {
                    if (count > 0) try run(w, previous, count);
                    previous = char;
                    count = 1;
                }
            }
            // Trailing blank sixels draw nothing; omit them.
            if (previous != 63) try run(w, previous, count);
        };
        try w.writeByte('-');
    }
    try w.writeAll("\x1b\\");
}

test "palette reproduces the image's own colours, most frequent first" {
    var px: [5 * 3]u8 = .{ 178, 212, 235, 178, 212, 235, 178, 212, 235, 0, 0, 0, 0, 0, 0 };
    const pal = try palette(std.testing.allocator, &px, 5, 0, 1);
    defer pal.deinit(std.testing.allocator);
    try std.testing.expectEqual(@as(usize, 2), pal.len);
    try std.testing.expectEqual([3]u8{ 178, 212, 235 }, pal.colors[0]);
    try std.testing.expectEqual([3]u8{ 0, 0, 0 }, pal.colors[1]);
}

test "encoder emits header, palette, run-length bands, and terminator" {
    var out: std.Io.Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    // 8×6: left half black, right half pure red.
    var px: [8 * 6 * 3]u8 = undefined;
    for (0..6) |y| for (0..8) |x| {
        const p = (y * 8 + x) * 3;
        px[p..][0..3].* = if (x < 4) .{ 0, 0, 0 } else .{ 255, 0, 0 };
    };
    try encode(std.testing.allocator, &out.writer, &px, 8, 0, 6);
    const s = out.written();
    try std.testing.expect(std.mem.startsWith(u8, s, "\x1bP0;1;0q\"1;1;8;6"));
    try std.testing.expect(std.mem.endsWith(u8, s, "-\x1b\\"));
    try std.testing.expect(std.mem.indexOf(u8, s, ";2;100;0;0") != null);
    try std.testing.expect(std.mem.indexOf(u8, s, "!4~") != null); // 4 full black columns
    try std.testing.expect(std.mem.indexOf(u8, s, "!4?!4~") != null); // red: 4 blank then 4 full
}
