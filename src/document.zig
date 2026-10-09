//! Converted-document model and its on-disk format (one file per book in the
//! library folder). Line-oriented and tab-separated like the embedded Bible
//! data, so documents stay greppable and diffable:
//!
//!   KATA-DOC<TAB>1
//!   title<TAB>…           author<TAB>…       source<TAB>file.epub
//!   converter<TAB>N
//!   chapter<TAB><title>
//!   <kind><TAB><level><TAB><verse><TAB><page><TAB><text>
//!
//! Text escapes: `\\`, `\n`, `\t`. `verse` is 0 when absent; `page` is written
//! only when it changes and carries forward when empty.
const std = @import("std");

pub const magic = "KATA-DOC";
pub const format_version = 1;

pub const Kind = enum {
    heading,
    paragraph,
    verse,
    superscription,
    rubric,
    quote,
    code,
    list_item,
    term,
    table_row,
    figure,
    footnote,
    /// Text is an asset file name inside `<document>.assets/`.
    image,

    /// Separated from the previous block by a blank line when rendered.
    pub fn spaced(self: Kind) bool {
        return switch (self) {
            .verse, .list_item, .table_row, .footnote => false,
            else => true,
        };
    }
};

pub const Block = struct {
    kind: Kind,
    /// Heading level 1–6; 0 otherwise.
    level: u8 = 0,
    /// Verse number (explicit or inferred); 0 when absent.
    verse: u32 = 0,
    /// Print page label in effect where the block starts ("" if unknown).
    page: []const u8 = "",
    text: []const u8,
};

pub const Chapter = struct { title: []const u8, blocks: []const Block };

pub const Document = struct {
    title: []const u8,
    author: []const u8 = "",
    source: []const u8 = "",
    converter: u32 = 0,
    chapters: []const Chapter,

    pub fn blockCount(self: Document) usize {
        var total: usize = 0;
        for (self.chapters) |chapter| total += chapter.blocks.len;
        return total;
    }
};

fn writeEscaped(w: *std.Io.Writer, text: []const u8) !void {
    for (text) |c| switch (c) {
        '\\' => try w.writeAll("\\\\"),
        '\n' => try w.writeAll("\\n"),
        '\t' => try w.writeAll("\\t"),
        '\r' => {},
        else => try w.writeByte(c),
    };
}

fn unescape(allocator: std.mem.Allocator, text: []const u8) ![]const u8 {
    if (std.mem.indexOfScalar(u8, text, '\\') == null) return text;
    var out: std.ArrayList(u8) = .empty;
    var i: usize = 0;
    while (i < text.len) : (i += 1) {
        if (text[i] == '\\' and i + 1 < text.len) {
            i += 1;
            try out.append(allocator, switch (text[i]) {
                'n' => '\n',
                't' => '\t',
                else => text[i],
            });
        } else try out.append(allocator, text[i]);
    }
    return out.toOwnedSlice(allocator);
}

pub fn write(w: *std.Io.Writer, doc: Document) !void {
    try w.print("{s}\t{d}\n", .{ magic, format_version });
    inline for (.{ "title", "author", "source" }) |field| {
        try w.writeAll(field ++ "\t");
        try writeEscaped(w, @field(doc, field));
        try w.writeByte('\n');
    }
    try w.print("converter\t{d}\n", .{doc.converter});
    var page: []const u8 = "";
    for (doc.chapters) |chapter| {
        try w.writeAll("chapter\t");
        try writeEscaped(w, chapter.title);
        try w.writeByte('\n');
        for (chapter.blocks) |block| {
            try w.print("{s}\t{d}\t{d}\t", .{ @tagName(block.kind), block.level, block.verse });
            if (!std.mem.eql(u8, block.page, page)) {
                try writeEscaped(w, block.page);
                page = block.page;
            }
            try w.writeByte('\t');
            try writeEscaped(w, block.text);
            try w.writeByte('\n');
        }
    }
}

/// Parses a document file. Strings borrow `bytes` or are allocated.
pub fn parse(allocator: std.mem.Allocator, bytes: []const u8) !Document {
    if (!std.unicode.utf8ValidateSlice(bytes)) return error.InvalidDocument;
    var lines = std.mem.splitScalar(u8, bytes, '\n');
    const first = lines.next() orelse return error.InvalidDocument;
    if (!std.mem.eql(u8, first, magic ++ "\t1")) return error.InvalidDocument;
    var doc: Document = .{ .title = "", .chapters = &.{} };
    var chapters: std.ArrayList(Chapter) = .empty;
    var blocks: std.ArrayList(Block) = .empty;
    var chapter_title: ?[]const u8 = null;
    var page: []const u8 = "";
    while (lines.next()) |line| {
        if (line.len == 0) continue;
        var fields = std.mem.splitScalar(u8, line, '\t');
        const key = fields.next().?;
        if (std.mem.eql(u8, key, "chapter")) {
            if (chapter_title) |title| try chapters.append(allocator, .{ .title = title, .blocks = try blocks.toOwnedSlice(allocator) });
            chapter_title = try unescape(allocator, fields.rest());
            continue;
        }
        if (std.mem.eql(u8, key, "title")) {
            doc.title = try unescape(allocator, fields.rest());
        } else if (std.mem.eql(u8, key, "author")) {
            doc.author = try unescape(allocator, fields.rest());
        } else if (std.mem.eql(u8, key, "source")) {
            doc.source = try unescape(allocator, fields.rest());
        } else if (std.mem.eql(u8, key, "converter")) {
            doc.converter = std.fmt.parseInt(u32, fields.rest(), 10) catch return error.InvalidDocument;
        } else {
            const kind = std.meta.stringToEnum(Kind, key) orelse return error.InvalidDocument;
            if (chapter_title == null) return error.InvalidDocument;
            const level = std.fmt.parseInt(u8, fields.next() orelse return error.InvalidDocument, 10) catch return error.InvalidDocument;
            const verse = std.fmt.parseInt(u32, fields.next() orelse return error.InvalidDocument, 10) catch return error.InvalidDocument;
            const page_field = fields.next() orelse return error.InvalidDocument;
            if (page_field.len > 0) page = try unescape(allocator, page_field);
            try blocks.append(allocator, .{ .kind = kind, .level = level, .verse = verse, .page = page, .text = try unescape(allocator, fields.rest()) });
        }
    }
    if (chapter_title) |title| try chapters.append(allocator, .{ .title = title, .blocks = try blocks.toOwnedSlice(allocator) });
    if (chapters.items.len == 0) return error.InvalidDocument;
    doc.chapters = try chapters.toOwnedSlice(allocator);
    return doc;
}

test "documents round-trip with escapes and carried page labels" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const doc: Document = .{ .title = "T\tx", .author = "A", .source = "t.epub", .converter = 3, .chapters = &.{
        .{ .title = "One", .blocks = &.{
            .{ .kind = .heading, .level = 1, .text = "One" },
            .{ .kind = .verse, .verse = 1, .page = "26", .text = "a \\ b" },
            .{ .kind = .code, .page = "26", .text = "fn x() {\n\treturn;\n}" },
        } },
        .{ .title = "Two", .blocks = &.{.{ .kind = .paragraph, .page = "27", .text = "p" }} },
    } };
    var out: std.Io.Writer.Allocating = .init(a);
    try write(&out.writer, doc);
    try std.testing.expectEqual(@as(usize, 1), std.mem.count(u8, out.written(), "\t26\t"));
    const back = try parse(a, out.written());
    try std.testing.expectEqualStrings("T\tx", back.title);
    try std.testing.expectEqual(@as(u32, 3), back.converter);
    try std.testing.expectEqual(@as(usize, 2), back.chapters.len);
    try std.testing.expectEqualStrings("a \\ b", back.chapters[0].blocks[1].text);
    try std.testing.expectEqualStrings("fn x() {\n\treturn;\n}", back.chapters[0].blocks[2].text);
    try std.testing.expectEqualStrings("26", back.chapters[0].blocks[2].page);
    try std.testing.expectEqualStrings("27", back.chapters[1].blocks[0].page);
    try std.testing.expectError(error.InvalidDocument, parse(a, "nope\n"));
}
