const std = @import("std");
const source = @import("source.zig");
const portable_width = @import("unicode_width.zig");
extern "c" fn wcwidth(c: c_int) c_int;

pub const Row = struct {
    book: []const u8,
    chapter: u16,
    number: u16,
    texts: [3]?[]const u8 = .{ null, null, null },
};
pub const Line = struct { cells: [3][]const u8, row: usize };

pub fn alignVerses(allocator: std.mem.Allocator, streams: [3][]const source.Verse) ![]Row {
    var rows: std.ArrayList(Row) = .empty;
    errdefer rows.deinit(allocator);
    for (streams, 0..) |verses, pane| {
        for (verses) |verse| {
            var found: ?usize = null;
            for (rows.items, 0..) |row, index| {
                if (std.mem.eql(u8, row.book, verse.book) and row.chapter == verse.chapter and row.number == verse.number) {
                    found = index;
                    break;
                }
            }
            if (found == null) {
                try rows.append(allocator, .{ .book = verse.book, .chapter = verse.chapter, .number = verse.number });
                found = rows.items.len - 1;
            }
            if (rows.items[found.?].texts[pane] != null) return error.DuplicateAlignedVerse;
            rows.items[found.?].texts[pane] = verse.text;
        }
    }
    // Each call assembles a single book/passage. Session assembly preserves passage order.
    std.mem.sort(Row, rows.items, {}, struct {
        fn less(_: void, a: Row, b: Row) bool {
            return if (a.chapter == b.chapter) a.number < b.number else a.chapter < b.chapter;
        }
    }.less);
    return rows.toOwnedSlice(allocator);
}

fn codepointWidth(cp: u21) usize {
    if (@import("builtin").os.tag == .windows) return portable_width.width(cp);
    if ((cp >= 0x300 and cp <= 0x36f) or (cp >= 0x1ab0 and cp <= 0x1aff) or
        (cp >= 0x1dc0 and cp <= 0x1dff) or (cp >= 0xfe00 and cp <= 0xfe0f)) return 0;
    const width = wcwidth(@intCast(cp));
    return if (width >= 0) @intCast(width) else 1;
}

pub fn displayWidth(text: []const u8) usize {
    const view = std.unicode.Utf8View.init(text) catch return text.len;
    var it = view.iterator();
    var width: usize = 0;
    while (it.nextCodepoint()) |cp| width += codepointWidth(cp);
    return width;
}

pub fn wrap(allocator: std.mem.Allocator, text: []const u8, width: usize) ![][]const u8 {
    if (width < 2) return error.TooNarrow;
    var lines: std.ArrayList([]const u8) = .empty;
    errdefer lines.deinit(allocator);
    var start: usize = 0;
    while (start < text.len) {
        var end = start;
        var used: usize = 0;
        var last_space: ?usize = null;
        while (end < text.len) {
            const bytes = try std.unicode.utf8ByteSequenceLength(text[end]);
            const cp = try std.unicode.utf8Decode(text[end..][0..bytes]);
            const cells = codepointWidth(cp);
            if (used + cells > width and end > start) break;
            if (cp == ' ') last_space = end;
            used += cells;
            end += bytes;
        }
        var next = end;
        if (end < text.len) {
            if (last_space) |space| {
                if (space > start) {
                    end = space;
                    next = space + 1;
                }
            }
        }
        try lines.append(allocator, text[start..end]);
        start = next;
        while (start < text.len and text[start] == ' ') start += 1;
    }
    if (lines.items.len == 0) try lines.append(allocator, "");
    return lines.toOwnedSlice(allocator);
}

pub fn renderLines(allocator: std.mem.Allocator, rows: []const Row, width: usize, enabled: [3]bool) ![]Line {
    var output: std.ArrayList(Line) = .empty;
    errdefer output.deinit(allocator);
    for (rows, 0..) |row, index| {
        var wrapped: [3][][]const u8 = undefined;
        var height: usize = 1;
        for (0..3) |pane| {
            const text = try std.fmt.allocPrint(allocator, "{d}:{d} {s}", .{ row.chapter, row.number, row.texts[pane] orelse "[not present under this verse label]" });
            wrapped[pane] = try wrap(allocator, text, width);
            if (enabled[pane]) height = @max(height, wrapped[pane].len);
        }
        for (0..height) |line_index| {
            var line: Line = .{ .cells = .{ "", "", "" }, .row = index };
            for (0..3) |pane| {
                if (line_index < wrapped[pane].len) line.cells[pane] = wrapped[pane][line_index];
            }
            try output.append(allocator, line);
        }
    }
    return output.toOwnedSlice(allocator);
}

test "alignment uses verse labels and leaves a missing cell" {
    const a = [_]source.Verse{
        .{ .book = "John", .chapter = 1, .number = 1, .text = "first" },
        .{ .book = "John", .chapter = 1, .number = 3, .text = "third" },
    };
    const b = [_]source.Verse{.{ .book = "John", .chapter = 1, .number = 3, .text = "τρίτος" }};
    const rows = try alignVerses(std.testing.allocator, .{ &a, &b, &a });
    defer std.testing.allocator.free(rows);
    try std.testing.expectEqual(@as(usize, 2), rows.len);
    try std.testing.expect(rows[0].texts[1] == null);
    try std.testing.expectEqualStrings("τρίτος", rows[1].texts[1].?);
}

test "wrapping respects UTF-8 boundaries and combining marks" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const lines = try wrap(arena.allocator(), "Ἐν ἀρχῇ λόγος", 5);
    try std.testing.expect(lines.len > 1);
    for (lines) |line| {
        try std.testing.expect(std.unicode.utf8ValidateSlice(line));
        try std.testing.expect(displayWidth(line) <= 5);
    }
    try std.testing.expectEqual(@as(usize, 1), displayWidth("a\u{0301}"));
}

test "row heights are shared across panes" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const rows = [_]Row{.{ .book = "John", .chapter = 1, .number = 1, .texts = .{ "short", "a much longer translation", "short" } }};
    const lines = try renderLines(arena.allocator(), &rows, 10, .{ true, true, true });
    try std.testing.expect(lines.len > 1);
    try std.testing.expectEqualStrings("", lines[lines.len - 1].cells[0]);
    try std.testing.expectEqualStrings("", lines[lines.len - 1].cells[2]);
}

test {
    std.testing.refAllDecls(portable_width);
}
