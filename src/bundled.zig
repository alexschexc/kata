const std = @import("std");

pub const Row = struct { chapter: u16, number: u16, text: []const u8 };
const Span = @import("data/kjv_index.zig").Span;
const Dataset = struct { text: []const u8, spans: []const Span };
const datasets = [_]Dataset{
    .{ .text = @embedFile("data/kjv.tsv"), .spans = &@import("data/kjv_index.zig").spans },
    .{ .text = @embedFile("data/grb.tsv"), .spans = &@import("data/grb_index.zig").spans },
    .{ .text = @embedFile("data/vul.tsv"), .spans = &@import("data/vul_index.zig").spans },
};

pub const Selection = struct {
    chapters: []const u8 = "",
    verses: []const u8 = "",
    cross: ?[2]u32 = null,

    pub fn init(suffix: []const u8) !Selection {
        if (suffix.len == 0) return .{};
        if (suffix[0] != ':' or suffix.len == 1) return error.InvalidReference;
        const body = suffix[1..];
        const colon = std.mem.indexOfScalar(u8, body, ':') orelse body.len;
        const chapters = body[0..colon];
        try validateList(chapters);
        const verses = if (colon < body.len) body[colon + 1 ..] else "";
        if (colon < body.len) {
            const first_chapter = try number(chapters); // Verse queries require one chapter.
            if (std.mem.indexOfScalar(u8, verses, ':')) |end_colon| {
                const dash = std.mem.indexOfScalar(u8, verses[0..end_colon], '-') orelse return error.InvalidReference;
                const first_verse = try number(verses[0..dash]);
                const last_chapter = try number(verses[dash + 1 .. end_colon]);
                const last_verse = try number(verses[end_colon + 1 ..]);
                const first = position(first_chapter, first_verse);
                const last = position(last_chapter, last_verse);
                if (last < first) return error.InvalidReference;
                return .{ .cross = .{ first, last } };
            }
            try validateList(verses);
        }
        return .{ .chapters = chapters, .verses = verses };
    }

    pub fn hasChapter(self: Selection, chapter: u16) bool {
        if (self.cross) |bounds| return chapter >= bounds[0] >> 16 and chapter <= bounds[1] >> 16;
        return self.chapters.len == 0 or contains(self.chapters, chapter);
    }

    pub fn matches(self: Selection, chapter: u16, verse: u16) bool {
        if (self.cross) |bounds| {
            const label = position(chapter, verse);
            return label >= bounds[0] and label <= bounds[1];
        }
        return self.hasChapter(chapter) and (self.verses.len == 0 or contains(self.verses, verse));
    }
};

fn position(chapter: u16, verse: u16) u32 {
    return (@as(u32, chapter) << 16) | verse;
}

fn number(text: []const u8) !u16 {
    if (text.len == 0) return error.InvalidReference;
    for (text) |ch| if (!std.ascii.isDigit(ch)) return error.InvalidReference;
    return std.fmt.parseInt(u16, text, 10) catch error.InvalidReference;
}

fn interval(text: []const u8) ![2]u16 {
    const dash = std.mem.indexOfScalar(u8, text, '-') orelse text.len;
    const first = try number(text[0..dash]);
    const last = if (dash < text.len) try number(text[dash + 1 ..]) else first;
    if (last < first) return error.InvalidReference;
    return .{ first, last };
}

fn validateList(text: []const u8) !void {
    var terms = std.mem.splitScalar(u8, text, ',');
    while (terms.next()) |term| _ = try interval(term);
}

fn contains(text: []const u8, value: u16) bool {
    var terms = std.mem.splitScalar(u8, text, ',');
    while (terms.next()) |term| {
        const bounds = interval(term) catch return false;
        if (value >= bounds[0] and value <= bounds[1]) return true;
    }
    return false;
}

/// One verse record of a source, in embedded (source) order.
pub const Record = struct { book: []const u8, chapter: u16, number: u16, text: []const u8 };

/// Sequential scan of every non-empty verse record in one source, borrowing
/// the embedded text. `book` is the source's own book name (span name).
pub const Records = struct {
    data: Dataset,
    span: usize = 0,
    lines: ?std.mem.SplitIterator(u8, .scalar) = null,

    pub fn next(self: *Records) ?Record {
        while (true) {
            if (self.lines == null) {
                if (self.span >= self.data.spans.len) return null;
                const span = self.data.spans[self.span];
                self.lines = std.mem.splitScalar(u8, self.data.text[span.start..span.end], '\n');
            }
            const raw_line = self.lines.?.next() orelse {
                self.lines = null;
                self.span += 1;
                continue;
            };
            const line = std.mem.trimEnd(u8, raw_line, "\r");
            var fields = std.mem.splitScalar(u8, line, '\t');
            _ = fields.next() orelse continue;
            _ = fields.next() orelse continue;
            _ = fields.next() orelse continue;
            const c = std.fmt.parseInt(u16, fields.next() orelse continue, 10) catch continue;
            const label = fields.next() orelse continue;
            const dash = std.mem.indexOfScalar(u8, label, '-') orelse label.len;
            const v = std.fmt.parseInt(u16, label[0..dash], 10) catch continue;
            const text = fields.rest();
            if (text.len == 0) continue;
            return .{ .book = self.data.spans[self.span].book, .chapter = c, .number = v, .text = text };
        }
    }
};

pub fn records(source_index: usize) Records {
    return .{ .data = datasets[source_index] };
}

/// Pure native retrieval from immutable embedded TSV, using generated chapter
/// byte spans. No I/O, subprocesses, filesystem access, or runtime cache.
/// Text borrows embedded bytes except merged Greek labels, which use allocator;
/// callers normally supply their passage/application arena for both lifetimes.
pub fn query(allocator: std.mem.Allocator, source_index: usize, raw_book: []const u8, suffix: []const u8) ![]Row {
    var result: std.ArrayList(Row) = .empty;
    errdefer result.deinit(allocator);
    // Index each observed label once; whole books and daily chapters are linear
    // in selected rows, including non-adjacent repeated Greek labels.
    var labels = std.AutoHashMap(u32, struct { index: usize, owned: bool }).init(allocator);
    defer labels.deinit();
    errdefer {
        var values = labels.valueIterator();
        while (values.next()) |value| if (value.owned) allocator.free(result.items[value.index].text);
    }
    if (source_index >= datasets.len) return result.toOwnedSlice(allocator);
    const selection = try Selection.init(suffix);
    const data = datasets[source_index];
    for (data.spans) |span| {
        if (!std.mem.eql(u8, span.book, raw_book)) continue;
        if (!selection.hasChapter(span.chapter)) continue;
        var lines = std.mem.splitScalar(u8, data.text[span.start..span.end], '\n');
        while (lines.next()) |raw_line| {
            const line = std.mem.trimEnd(u8, raw_line, "\r");
            var fields = std.mem.splitScalar(u8, line, '\t');
            _ = fields.next() orelse continue;
            _ = fields.next() orelse continue;
            _ = fields.next() orelse continue;
            const c = std.fmt.parseInt(u16, fields.next() orelse continue, 10) catch continue;
            const label = fields.next() orelse continue;
            // The original exporter prints these combined source labels with
            // %d (e.g. 27-28 becomes 27); never fabricate the omitted label.
            const dash = std.mem.indexOfScalar(u8, label, '-') orelse label.len;
            const v = try std.fmt.parseInt(u16, label[0..dash], 10);
            const text = fields.rest();
            if (text.len == 0 and source_index == 0 and std.mem.eql(u8, raw_book, "Sirach") and c == 0 and v == 0) continue;
            if (!selection.matches(c, v)) continue;
            const entry = try labels.getOrPut(position(c, v));
            if (entry.found_existing) {
                if (source_index != 1) return error.DuplicateVerse;
                const existing = &result.items[entry.value_ptr.index];
                const joined = try std.fmt.allocPrint(allocator, "{s} {s}", .{ existing.text, text });
                if (entry.value_ptr.owned) allocator.free(existing.text);
                existing.text = joined;
                entry.value_ptr.owned = true;
            } else {
                entry.value_ptr.* = .{ .index = result.items.len, .owned = false };
                try result.append(allocator, .{ .chapter = c, .number = v, .text = text });
            }
        }
    }
    return result.toOwnedSlice(allocator);
}
