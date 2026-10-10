//! Inline book images (Sixel, or Kitty graphics in Ghostty/kitty/WezTerm). The reader reserves `rows` text lines per
//! image; after drawing a frame it asks `draw` to paint whichever rows of the
//! image are visible. Decoded+scaled bitmaps are cached per (asset, width);
//! encoding the visible band is cheap (~10 ms) and done per frame.
//! PNG and JPEG (baseline and progressive); unusual files fall back to
//! the `▣ image · i opens it` line.
const std = @import("std");
const graphics = @import("graphics.zig");
const png = @import("png.zig");
const jpeg = @import("jpeg.zig");
const sixel = @import("sixel.zig");

/// Paper colour behind images (matches the reader background).
const paper = [3]u8{ 21, 25, 34 };
const cache_limit = 48 << 20;

pub const Fit = struct { width: u32, height: u32, rows: u16 };

/// Scales (w, h) to at most `cols` cells wide and `max_rows` cells tall,
/// never enlarging. `rows` is the number of text lines to reserve.
pub fn fit(w: u32, h: u32, cols: usize, max_rows: usize, cell_w: u16, cell_h: u16) ?Fit {
    if (w == 0 or h == 0 or cols == 0 or max_rows == 0 or cell_w == 0 or cell_h == 0) return null;
    var width: u64 = @min(w, cols * cell_w);
    var height: u64 = @max(1, @as(u64, h) * width / w);
    const max_h = max_rows * @as(u64, cell_h);
    if (height > max_h) {
        height = max_h;
        width = @max(1, @as(u64, w) * height / h);
    }
    const rows = (height + cell_h - 1) / cell_h;
    return .{ .width = @intCast(width), .height = @intCast(height), .rows = @intCast(rows) };
}

const Bitmap = struct {
    fit: Fit,
    /// width × (rows * cell_h) RGB, padded below the image with paper.
    pixels: []u8,
    /// Kitty image id once transmitted to the terminal (0: not yet).
    kitty_id: u32 = 0,
};

/// Kitty graphics: transmits RGB once (chunked base64, quiet), then each
/// frame places the visible source rows. Placements are deleted at the start
/// of every frame, so scrolling never leaves stale images behind.
fn kittyTransmit(w: *std.Io.Writer, id: u32, pixels: []const u8, width: u32, height: usize) !void {
    const encoder = std.base64.standard.Encoder;
    const chunk = 3072; // multiple of 3 → 4096 base64 bytes per escape
    var at: usize = 0;
    var first = true;
    while (at < pixels.len or first) {
        const end = @min(pixels.len, at + chunk);
        const more: u8 = if (end < pixels.len) 1 else 0;
        if (first) {
            try w.print("\x1b_Ga=t,q=2,f=24,i={d},s={d},v={d},m={d};", .{ id, width, height, more });
        } else try w.print("\x1b_Gq=2,m={d};", .{more});
        var buf: [4096]u8 = undefined;
        try w.writeAll(encoder.encode(&buf, pixels[at..end]));
        try w.writeAll("\x1b\\");
        at = end;
        first = false;
    }
}

/// Removes every Kitty placement from the screen (image data is kept).
pub const kitty_clear = "\x1b_Ga=d,d=a,q=2\x1b\\";

pub const Renderer = struct {
    allocator: std.mem.Allocator,
    io: std.Io,
    /// `<library>/<book>.assets`
    dir: []const u8,
    caps: graphics.Capabilities,
    /// asset name → PNG dimensions (null: not displayable inline)
    dims: std.StringHashMapUnmanaged(?[2]u32) = .empty,
    bitmaps: std.StringHashMapUnmanaged(Bitmap) = .empty,
    cached_bytes: usize = 0,
    /// Kitty images to delete from terminal memory at the next frame.
    pending_clear: bool = false,

    next_id: u32 = 1,

    pub fn enabled(self: *const Renderer) bool {
        return self.caps.protocol != .none and self.caps.cell_width > 0 and self.caps.cell_height > 0;
    }

    pub fn deinit(self: *Renderer) void {
        self.clear();
        var it = self.dims.keyIterator();
        while (it.next()) |key| self.allocator.free(key.*);
        self.dims.deinit(self.allocator);
        self.bitmaps.deinit(self.allocator);
    }

    /// Updates the cell size (window resized or font changed); drops bitmaps
    /// sized for the old cells. Returns true when it changed.
    pub fn setCell(self: *Renderer, width: u16, height: u16) bool {
        if (width == 0 or height == 0 or (width == self.caps.cell_width and height == self.caps.cell_height)) return false;
        self.caps.cell_width = width;
        self.caps.cell_height = height;
        self.clear();
        return true;
    }

    fn clear(self: *Renderer) void {
        var it = self.bitmaps.iterator();
        // Forget transmitted kitty images too (they are re-sent if needed).
        if (self.caps.protocol == .kitty) self.pending_clear = true;
        while (it.next()) |entry| {
            self.allocator.free(entry.key_ptr.*);
            self.allocator.free(entry.value_ptr.pixels);
        }
        self.bitmaps.clearRetainingCapacity();
        self.cached_bytes = 0;
    }

    fn path(self: *Renderer, name: []const u8) ![]u8 {
        return std.fs.path.join(self.allocator, &.{ self.dir, name });
    }

    fn dimensions(self: *Renderer, name: []const u8) ?[2]u32 {
        if (self.dims.get(name)) |known| return known;
        const value: ?[2]u32 = blk: {
            const full = self.path(name) catch break :blk null;
            defer self.allocator.free(full);
            var header: [64 << 10]u8 = undefined; // JPEG frame headers follow EXIF/ICC
            const file = std.Io.Dir.cwd().openFile(self.io, full, .{}) catch break :blk null;
            defer file.close(self.io);
            const n = file.readPositionalAll(self.io, &header, 0) catch break :blk null;
            if (png.info(header[0..n])) |info| {
                if (info.interlaced) break :blk null;
                break :blk .{ info.width, info.height };
            } else |_| {}
            const info = jpeg.info(header[0..n]) catch break :blk null;
            break :blk .{ info.width, info.height };
        };
        const key = self.allocator.dupe(u8, name) catch return value;
        self.dims.put(self.allocator, key, value) catch self.allocator.free(key);
        return value;
    }

    /// Text lines to reserve for `name` at this width, or null to show the
    /// placeholder line instead.
    pub fn rows(self: *Renderer, name: []const u8, cols: usize, max_rows: usize) ?u16 {
        if (!self.enabled()) return null;
        const d = self.dimensions(name) orelse return null;
        const f = fit(d[0], d[1], cols, max_rows, self.caps.cell_width, self.caps.cell_height) orelse return null;
        return f.rows;
    }

    var key_buf: [512]u8 = undefined;

    fn cacheKey(self: *Renderer, name: []const u8, cols: usize, max_rows: usize) ![]const u8 {
        _ = self;
        return std.fmt.bufPrint(&key_buf, "{s}\x00{d}\x00{d}", .{ name, cols, max_rows });
    }

    /// Start-of-frame commands: clear kitty placements (and freed images).
    pub fn beginFrame(self: *Renderer, w: *std.Io.Writer) !void {
        if (self.caps.protocol != .kitty) return;
        if (self.pending_clear) {
            try w.writeAll("\x1b_Ga=d,d=A,q=2\x1b\\"); // delete placements and image data
            self.pending_clear = false;
        } else try w.writeAll(kitty_clear);
    }

    fn bitmap(self: *Renderer, name: []const u8, cols: usize, max_rows: usize) !?Bitmap {
        const key = try std.fmt.allocPrint(self.allocator, "{s}\x00{d}\x00{d}", .{ name, cols, max_rows });
        if (self.bitmaps.get(key)) |hit| {
            self.allocator.free(key);
            return hit;
        }
        errdefer self.allocator.free(key);
        const d = self.dimensions(name) orelse return null;
        const f = fit(d[0], d[1], cols, max_rows, self.caps.cell_width, self.caps.cell_height) orelse return null;
        const full = try self.path(name);
        defer self.allocator.free(full);
        var scratch = std.heap.ArenaAllocator.init(self.allocator);
        defer scratch.deinit();
        const bytes = try std.Io.Dir.cwd().readFileAlloc(self.io, full, scratch.allocator(), .limited(64 << 20));
        const decoded = if (std.mem.startsWith(u8, bytes, "\xff\xd8")) try jpeg.decode(scratch.allocator(), bytes) else try png.decode(scratch.allocator(), bytes);
        const scaled = try png.scale(scratch.allocator(), decoded, f.width, f.height);
        const total_h = @as(usize, f.rows) * self.caps.cell_height;
        const pixels = try self.allocator.alloc(u8, @as(usize, f.width) * total_h * 3);
        @memcpy(pixels[0..scaled.pixels.len], scaled.pixels);
        var at = scaled.pixels.len;
        while (at < pixels.len) : (at += 3) pixels[at..][0..3].* = paper;
        if (self.cached_bytes + pixels.len > cache_limit) self.clear();
        const entry: Bitmap = .{ .fit = f, .pixels = pixels };
        try self.bitmaps.put(self.allocator, key, entry);
        self.cached_bytes += pixels.len;
        return entry;
    }

    /// Paints image rows [first_row, first_row + count) at the cursor's
    /// current cell. Never paints below the reserved rows. Returns false if
    /// the image could not be decoded (caller keeps the blank rows).
    pub fn draw(self: *Renderer, w: *std.Io.Writer, name: []const u8, cols: usize, max_rows: usize, first_row: usize, count: usize) !bool {
        const entry = (self.bitmap(name, cols, max_rows) catch null) orelse return false;
        const cell_h: usize = self.caps.cell_height;
        const top = first_row * cell_h;
        if (top >= @as(usize, entry.fit.rows) * cell_h) return true;
        // Whole sixel bands only, so nothing spills into the next text line.
        if (self.caps.protocol == .kitty) {
            const rows_shown = @min(count, entry.fit.rows - first_row);
            const height = rows_shown * cell_h;
            if (height == 0) return true;
            const total_h = @as(usize, entry.fit.rows) * cell_h;
            const key_entry = self.bitmaps.getPtr(try self.cacheKey(name, cols, max_rows)) orelse return false;
            if (key_entry.kitty_id == 0) {
                key_entry.kitty_id = self.next_id;
                self.next_id += 1;
                try kittyTransmit(w, key_entry.kitty_id, key_entry.pixels, key_entry.fit.width, total_h);
            }
            // Place the visible source rows over exactly `rows_shown` cells;
            // C=1 keeps the cursor still so text drawing is unaffected.
            try w.print("\x1b_Ga=p,q=2,i={d},y={d},w={d},h={d},r={d},C=1\x1b\\", .{ key_entry.kitty_id, top, entry.fit.width, height, rows_shown });
            return true;
        }
        const height = (@min(count, entry.fit.rows - first_row) * cell_h) / 6 * 6;
        if (height == 0) return true;
        try sixel.encode(self.allocator, w, entry.pixels, entry.fit.width, top, height);
        return true;
    }
};

test "fit never enlarges, respects column and row limits, and rounds rows up" {
    const small = fit(100, 50, 80, 20, 10, 20).?;
    try std.testing.expectEqual(Fit{ .width = 100, .height = 50, .rows = 3 }, small);
    const wide = fit(2000, 1000, 80, 30, 10, 20).?;
    try std.testing.expectEqual(Fit{ .width = 800, .height = 400, .rows = 20 }, wide);
    const tall = fit(400, 2000, 80, 10, 10, 20).?;
    try std.testing.expectEqual(@as(u16, 10), tall.rows);
    try std.testing.expectEqual(@as(u32, 200), tall.height);
    try std.testing.expectEqual(@as(u32, 40), tall.width);
    try std.testing.expect(fit(100, 100, 80, 20, 0, 20) == null);
}

test "kitty transmit chunks base64 with continuation flags" {
    var out: std.Io.Writer.Allocating = .init(std.testing.allocator);
    defer out.deinit();
    const px = try std.testing.allocator.alloc(u8, 4000 * 3);
    defer std.testing.allocator.free(px);
    @memset(px, 7);
    try kittyTransmit(&out.writer, 5, px, 4000, 1);
    const s = out.written();
    try std.testing.expect(std.mem.startsWith(u8, s, "\x1b_Ga=t,q=2,f=24,i=5,s=4000,v=1,m=1;"));
    try std.testing.expectEqual(@as(usize, 4), std.mem.count(u8, s, "\x1b\\")); // 12000 bytes / 3072
    try std.testing.expect(std.mem.indexOf(u8, s, "\x1b_Gq=2,m=0;") != null);
}
