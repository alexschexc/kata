const std = @import("std");

pub const tools = [_][]const u8{ "kjv", "grb", "vul" };
pub const Verse = struct { book: []const u8, chapter: u16, number: u16, text: []const u8 };
pub const Book = struct { name: []const u8, alias: []const u8 };
pub const books = [_]Book{
    .{ .name = "Matthew", .alias = "Mat" },         .{ .name = "Mark", .alias = "Mark" },
    .{ .name = "Luke", .alias = "Luke" },           .{ .name = "John", .alias = "John" },
    .{ .name = "Acts", .alias = "Acts" },           .{ .name = "Romans", .alias = "Rom" },
    .{ .name = "1 Corinthians", .alias = "1Cor" },  .{ .name = "2 Corinthians", .alias = "2Cor" },
    .{ .name = "Galatians", .alias = "Gal" },       .{ .name = "Ephesians", .alias = "Eph" },
    .{ .name = "Philippians", .alias = "Phi" },     .{ .name = "Colossians", .alias = "Col" },
    .{ .name = "1 Thessalonians", .alias = "1Th" }, .{ .name = "2 Thessalonians", .alias = "2Th" },
    .{ .name = "1 Timothy", .alias = "1Tim" },      .{ .name = "2 Timothy", .alias = "2Tim" },
    .{ .name = "Titus", .alias = "Titus" },         .{ .name = "Philemon", .alias = "Phmn" },
    .{ .name = "Hebrews", .alias = "Heb" },         .{ .name = "James", .alias = "Jas" },
    .{ .name = "1 Peter", .alias = "1Pet" },        .{ .name = "2 Peter", .alias = "2Pet" },
    .{ .name = "1 John", .alias = "1Jn" },          .{ .name = "2 John", .alias = "2Jn" },
    .{ .name = "3 John", .alias = "3Jn" },          .{ .name = "Jude", .alias = "Jude" },
    .{ .name = "Revelation", .alias = "Rev" },
};

pub fn canonical(name: []const u8) ?[]const u8 {
    if (std.ascii.eqlIgnoreCase(name, "The Acts")) return "Acts";
    for (books) |book| {
        if (std.ascii.eqlIgnoreCase(name, book.name) or std.ascii.eqlIgnoreCase(name, book.alias)) return book.name;
    }
    return null;
}

pub const Reference = struct {
    book: []const u8,
    query: []const u8,

    pub fn init(allocator: std.mem.Allocator, raw: []const u8) !Reference {
        const colon = std.mem.indexOfScalar(u8, raw, ':') orelse raw.len;
        const book = canonical(raw[0..colon]) orelse return error.InvalidReference;
        const suffix = raw[colon..];
        if (suffix.len == 1) return error.InvalidReference;
        for (suffix) |ch| {
            if (!std.ascii.isDigit(ch) and ch != ':' and ch != '-' and ch != ',') return error.InvalidReference;
        }
        return .{ .book = book, .query = try std.fmt.allocPrint(allocator, "{s}{s}", .{ book, suffix }) };
    }
};

pub fn parse(allocator: std.mem.Allocator, output: []const u8, expected_book: []const u8) ![]Verse {
    if (std.mem.indexOf(u8, output, "Unknown reference:") != null) return error.InvalidReference;
    if (!std.unicode.utf8ValidateSlice(output)) return error.MalformedOutput;
    var result: std.ArrayList(Verse) = .empty;
    errdefer result.deinit(allocator);
    var lines = std.mem.splitScalar(u8, output, '\n');
    var have_header = false;
    while (lines.next()) |raw_line| {
        // vul contains CRLF verse records. Strip only the line-ending CR;
        // embedded controls are still rejected before terminal rendering.
        const line = if (std.mem.endsWith(u8, raw_line, "\r")) raw_line[0 .. raw_line.len - 1] else raw_line;
        for (line) |ch| {
            if ((ch < 32 and ch != '\t') or ch == 127) return error.MalformedOutput;
        }
        if (line.len == 0) continue;
        if (std.mem.indexOfScalar(u8, line, '\t')) |tab| {
            if (!have_header) return error.MalformedOutput;
            const colon = std.mem.indexOfScalar(u8, line[0..tab], ':') orelse return error.MalformedOutput;
            const chapter = std.fmt.parseInt(u16, line[0..colon], 10) catch return error.MalformedOutput;
            const number = std.fmt.parseInt(u16, line[colon + 1 .. tab], 10) catch return error.MalformedOutput;
            if (chapter == 0 or number == 0 or tab + 1 == line.len) return error.MalformedOutput;
            for (result.items) |existing| {
                if (existing.chapter == chapter and existing.number == number) return error.DuplicateVerse;
            }
            try result.append(allocator, .{ .book = expected_book, .chapter = chapter, .number = number, .text = line[tab + 1 ..] });
        } else {
            const name = canonical(line) orelse return error.MalformedOutput;
            if (!std.mem.eql(u8, name, expected_book)) return error.UnexpectedBook;
            have_header = true;
        }
    }
    if (result.items.len == 0) return error.NoVerses;
    return result.toOwnedSlice(allocator);
}

pub fn fetch(allocator: std.mem.Allocator, io: std.Io, tool: []const u8, reference: Reference) ![]Verse {
    const result = try std.process.run(allocator, io, .{
        .argv = &.{ tool, "-W", reference.query },
        .stdout_limit = .limited(8 * 1024 * 1024),
        .stderr_limit = .limited(64 * 1024),
    });
    // stdout remains alive in the application arena: verse text borrows it.
    switch (result.term) {
        .exited => |code| if (code != 0) return error.SourceFailed,
        else => return error.SourceFailed,
    }
    if (result.stderr.len != 0) return error.SourceFailed;
    return parse(allocator, result.stdout, reference.book);
}

test "parse UTF-8 verses using their labels" {
    const text = "John\n1:1\tἘν ἀρχῇ\n1:3\tthird\n";
    const verses = try parse(std.testing.allocator, text, "John");
    defer std.testing.allocator.free(verses);
    try std.testing.expectEqual(@as(usize, 2), verses.len);
    try std.testing.expectEqual(@as(u16, 3), verses[1].number);
    try std.testing.expectEqualStrings("Ἐν ἀρχῇ", verses[0].text);
}

test "Vulgate CRLF rows retain text without terminal controls" {
    const verses = try parse(std.testing.allocator, "John\n1:1\t[In principio erat Verbum.\r\n", "John");
    defer std.testing.allocator.free(verses);
    try std.testing.expectEqualStrings("[In principio erat Verbum.", verses[0].text);
}

test "exit-zero error output is not a passage" {
    try std.testing.expectError(error.InvalidReference, parse(std.testing.allocator, "Unknown reference: invalid\n", "John"));
}

test "unexpected book and duplicate labels are rejected" {
    try std.testing.expectError(error.UnexpectedBook, parse(std.testing.allocator, "John\n1:1\ttext\n", "Matthew"));
    try std.testing.expectError(error.DuplicateVerse, parse(std.testing.allocator, "John\n1:1\tfirst\n1:1\tsecond\n", "John"));
}

test "control sequences and malformed rows are rejected" {
    try std.testing.expectError(error.MalformedOutput, parse(std.testing.allocator, "John\n1:1\t\x1b[2J\n", "John"));
    try std.testing.expectError(error.MalformedOutput, parse(std.testing.allocator, "John\n1:1 text\n", "John"));
}

test "aliases canonicalize and cannot inject shell arguments" {
    const reference = try Reference.init(std.testing.allocator, "Phi:1:1-3");
    defer std.testing.allocator.free(reference.query);
    try std.testing.expectEqualStrings("Philippians", reference.book);
    try std.testing.expectEqualStrings("Philippians:1:1-3", reference.query);
    try std.testing.expectError(error.InvalidReference, Reference.init(std.testing.allocator, "John:1;echo x"));
}
