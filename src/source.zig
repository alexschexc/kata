const std = @import("std");

pub const tools = [_][]const u8{ "kjv", "grb", "vul" };
/// A source's own chapter:verse label when it differs from the aligned row.
pub const Label = struct { chapter: u16, number: u16 };
/// `chapter`/`number` are the alignment position; `label` (when set) is the
/// numbering printed by the source itself and is what the reader displays.
pub const Verse = struct { book: []const u8, chapter: u16, number: u16, text: []const u8, label: ?Label = null };
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

pub const catalog = @import("book_catalog.zig").catalog;

pub fn canonical(name: []const u8) ?[]const u8 {
    // Preserve the original NT aliases (notably Phi) ahead of the union.
    for (books) |book| {
        if (std.ascii.eqlIgnoreCase(name, book.name) or std.ascii.eqlIgnoreCase(name, book.alias)) return book.name;
    }
    for (catalog) |entry| {
        if (std.ascii.eqlIgnoreCase(name, entry.name)) return entry.name;
        for (entry.query_names, entry.aliases) |query_name, alias| {
            if (query_name) |n| if (std.ascii.eqlIgnoreCase(name, n)) return entry.name;
            if (alias) |n| if (std.ascii.eqlIgnoreCase(name, n)) return entry.name;
        }
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
        _ = try @import("bundled.zig").Selection.init(suffix);
        return .{ .book = book, .query = try std.fmt.allocPrint(allocator, "{s}{s}", .{ book, suffix }) };
    }
};

pub fn parse(allocator: std.mem.Allocator, output: []const u8, expected_book: []const u8) ![]Verse {
    return parseImpl(allocator, output, expected_book, false, false);
}

/// Installed tools prefix-match books. Select their exact canonical header.
/// Greek repeated labels are real data: join their text with one space, in
/// source order, without inventing labels for additions or misnumbered rows.
pub fn parseSource(allocator: std.mem.Allocator, output: []const u8, expected_book: []const u8, greek: bool) ![]Verse {
    return parseImpl(allocator, output, expected_book, true, greek);
}

fn parseImpl(allocator: std.mem.Allocator, output: []const u8, expected_book: []const u8, select_book: bool, greek: bool) ![]Verse {
    if (std.mem.indexOf(u8, output, "Unknown reference:") != null) return error.InvalidReference;
    if (!std.unicode.utf8ValidateSlice(output)) return error.MalformedOutput;
    var result: std.ArrayList(Verse) = .empty;
    errdefer result.deinit(allocator);
    var lines = std.mem.splitScalar(u8, output, '\n');
    var have_header = false;
    var selected = false;
    var saw_expected = false;
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
            // The KJV Sirach exporter appends a lone empty 0:0 sentinel.
            // It is not a verse or a populated chapter; no other empty row
            // is accepted by either the strict parser or the source adapter.
            if (select_book and selected and std.mem.eql(u8, expected_book, "Sirach") and chapter == 0 and number == 0 and tab + 1 == line.len) continue;
            if ((!select_book and (chapter == 0 or number == 0)) or tab + 1 == line.len) return error.MalformedOutput;
            if (!selected) continue;
            var repeated = false;
            for (result.items) |*existing| {
                if (existing.chapter == chapter and existing.number == number) {
                    if (!greek) return error.DuplicateVerse;
                    existing.text = try std.fmt.allocPrint(allocator, "{s} {s}", .{ existing.text, line[tab + 1 ..] });
                    repeated = true;
                    break;
                }
            }
            if (!repeated) try result.append(allocator, .{ .book = expected_book, .chapter = chapter, .number = number, .text = line[tab + 1 ..] });
        } else {
            const name = canonical(line) orelse return error.MalformedOutput;
            selected = std.mem.eql(u8, name, expected_book);
            if (!selected and !select_book) return error.UnexpectedBook;
            if (selected) saw_expected = true;
            have_header = true;
        }
    }
    if (!saw_expected and have_header) return error.UnexpectedBook;
    if (result.items.len == 0) return error.NoVerses;
    return result.toOwnedSlice(allocator);
}

pub fn catalogEntry(name: []const u8) ?@import("book_catalog.zig").Entry {
    const normalized = canonical(name) orelse return null;
    for (catalog) |entry| if (std.mem.eql(u8, entry.name, normalized)) return entry;
    return null;
}

/// Availability is based on observed chapter sets, not merely their maxima.
pub fn available(reference: Reference, tool_index: usize) bool {
    if (tool_index >= tools.len) return false;
    const entry = catalogEntry(reference.book) orelse return false;
    if (entry.chapters[tool_index] == 0) return false;
    const colon = std.mem.indexOfScalar(u8, reference.query, ':') orelse reference.query.len;
    const selection = @import("bundled.zig").Selection.init(reference.query[colon..]) catch return false;
    for (entry.present_chapters[tool_index]) |chapter| if (selection.hasChapter(chapter)) return true;
    return false;
}

pub fn streams(allocator: std.mem.Allocator, io: std.Io, reference: Reference) ![3][]const Verse {
    var result: [3][]const Verse = .{ &.{}, &.{}, &.{} };
    for (tools, 0..) |tool, i| {
        if (available(reference, i)) result[i] = try fetch(allocator, io, tool, reference);
    }
    return result;
}

pub fn fetch(allocator: std.mem.Allocator, io: std.Io, tool: []const u8, reference: Reference) ![]Verse {
    _ = io;
    const index = for (tools, 0..) |name, i| {
        if (std.mem.eql(u8, tool, name)) break i;
    } else return error.UnsupportedSource;
    if (!available(reference, index)) return error.UnsupportedLocation;
    const entry = catalogEntry(reference.book) orelse return error.InvalidReference;
    const raw_book = entry.query_names[index] orelse return error.UnsupportedLocation;
    const colon = std.mem.indexOfScalar(u8, reference.query, ':') orelse reference.query.len;
    if (index == 0 and std.mem.eql(u8, reference.book, "Psalms")) return fetchKjvPsalms(allocator, raw_book, reference);
    const rows = try @import("bundled.zig").query(allocator, index, raw_book, reference.query[colon..]);
    defer allocator.free(rows);
    const verses = try allocator.alloc(Verse, rows.len);
    for (rows, verses) |row, *verse| verse.* = .{ .book = reference.book, .chapter = row.chapter, .number = row.number, .text = row.text };
    return verses;
}

/// The KJV numbers Psalms by the Hebrew (Masoretic) text; grb and vul use the
/// Greek/Latin (LXX/Vulgate) numbering, which Kata treats as authoritative.
/// Maps a KJV psalm verse to its LXX psalm and an ordering position within it.
/// Merged psalms place the second KJV psalm after the first; split psalms
/// restart at 1. Verse numbers inside a psalm are not cross-mapped (the Greek
/// and Latin count superscriptions as verses), so the KJV keeps its own labels.
pub fn kjvPsalmToLxx(chapter: u16, verse: u16) ?Label {
    const kjv_psalm_9_verses = 20;
    const kjv_psalm_114_verses = 8;
    return switch (chapter) {
        1...9, 148...150 => .{ .chapter = chapter, .number = verse },
        10 => .{ .chapter = 9, .number = verse + kjv_psalm_9_verses },
        11...113, 117...146 => .{ .chapter = chapter - 1, .number = verse },
        114 => .{ .chapter = 113, .number = verse },
        115 => .{ .chapter = 113, .number = verse + kjv_psalm_114_verses },
        116 => if (verse <= 9) .{ .chapter = 114, .number = verse } else .{ .chapter = 115, .number = verse - 9 },
        147 => if (verse <= 11) .{ .chapter = 146, .number = verse } else .{ .chapter = 147, .number = verse - 11 },
        else => null,
    };
}

/// The reference's chapter/verse selection is in LXX numbering. Read the whole
/// KJV Psalter, relocate each verse, and keep those the selection matches.
fn fetchKjvPsalms(allocator: std.mem.Allocator, raw_book: []const u8, reference: Reference) ![]Verse {
    const colon = std.mem.indexOfScalar(u8, reference.query, ':') orelse reference.query.len;
    const selection = try @import("bundled.zig").Selection.init(reference.query[colon..]);
    const rows = try @import("bundled.zig").query(allocator, 0, raw_book, "");
    defer allocator.free(rows);
    var verses: std.ArrayList(Verse) = .empty;
    errdefer verses.deinit(allocator);
    for (rows) |row| {
        const at = kjvPsalmToLxx(row.chapter, row.number) orelse return error.MalformedOutput;
        if (!selection.matches(at.chapter, at.number)) continue;
        try verses.append(allocator, .{ .book = reference.book, .chapter = at.chapter, .number = at.number, .text = row.text, .label = .{ .chapter = row.chapter, .number = row.number } });
    }
    return verses.toOwnedSlice(allocator);
}

test "KJV Psalms follow Greek and Latin psalm numbering with their own verse labels" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const Case = struct { lxx: []const u8, first: Label, last: Label, count: usize };
    for ([_]Case{
        .{ .lxx = "Psalms:8", .first = .{ .chapter = 8, .number = 1 }, .last = .{ .chapter = 8, .number = 9 }, .count = 9 },
        .{ .lxx = "Psalms:9", .first = .{ .chapter = 9, .number = 1 }, .last = .{ .chapter = 10, .number = 18 }, .count = 38 },
        .{ .lxx = "Psalms:22", .first = .{ .chapter = 23, .number = 1 }, .last = .{ .chapter = 23, .number = 6 }, .count = 6 },
        .{ .lxx = "Psalms:112", .first = .{ .chapter = 113, .number = 1 }, .last = .{ .chapter = 113, .number = 9 }, .count = 9 },
        .{ .lxx = "Psalms:113", .first = .{ .chapter = 114, .number = 1 }, .last = .{ .chapter = 115, .number = 18 }, .count = 26 },
        .{ .lxx = "Psalms:114", .first = .{ .chapter = 116, .number = 1 }, .last = .{ .chapter = 116, .number = 9 }, .count = 9 },
        .{ .lxx = "Psalms:115", .first = .{ .chapter = 116, .number = 10 }, .last = .{ .chapter = 116, .number = 19 }, .count = 10 },
        .{ .lxx = "Psalms:116", .first = .{ .chapter = 117, .number = 1 }, .last = .{ .chapter = 117, .number = 2 }, .count = 2 },
        .{ .lxx = "Psalms:145", .first = .{ .chapter = 146, .number = 1 }, .last = .{ .chapter = 146, .number = 10 }, .count = 10 },
        .{ .lxx = "Psalms:146", .first = .{ .chapter = 147, .number = 1 }, .last = .{ .chapter = 147, .number = 11 }, .count = 11 },
        .{ .lxx = "Psalms:147", .first = .{ .chapter = 147, .number = 12 }, .last = .{ .chapter = 147, .number = 20 }, .count = 9 },
        .{ .lxx = "Psalms:148", .first = .{ .chapter = 148, .number = 1 }, .last = .{ .chapter = 148, .number = 14 }, .count = 14 },
    }) |case| {
        const rows = try streams(a, std.testing.io, try Reference.init(a, case.lxx));
        const kjv = rows[0];
        try std.testing.expectEqual(case.count, kjv.len);
        try std.testing.expectEqual(case.first, kjv[0].label.?);
        try std.testing.expectEqual(case.last, kjv[kjv.len - 1].label.?);
        for (kjv) |verse| try std.testing.expectEqual(kjv[0].chapter, verse.chapter);
        // Greek and Latin are unchanged and keep their own numbering.
        for (rows[1..]) |other| for (other) |verse| try std.testing.expect(verse.label == null);
    }
    const shepherd = try streams(a, std.testing.io, try Reference.init(a, "Psalms:22:1"));
    try std.testing.expectEqualStrings("The LORD is my shepherd; I shall not want.", shepherd[0][0].text);
    try std.testing.expect(std.mem.startsWith(u8, shepherd[2][0].text, "Psalmus David."));
    const extra = try streams(a, std.testing.io, try Reference.init(a, "Psalms:151"));
    try std.testing.expectEqual(@as(usize, 0), extra[0].len);
    try std.testing.expect(extra[1].len > 0);
    // Every KJV psalm verse appears exactly once under a distinct LXX position.
    const whole = try streams(a, std.testing.io, try Reference.init(a, "Psalms"));
    try std.testing.expectEqual(@as(usize, 2461), whole[0].len);
    const aligned = try @import("layout.zig").alignVerses(a, whole);
    var kjv_rows: usize = 0;
    for (aligned) |row| if (row.texts[0] != null) {
        kjv_rows += 1;
    };
    try std.testing.expectEqual(@as(usize, 2461), kjv_rows);
}

test "native absent verse does not block translations with that label" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const ref = try Reference.init(a, "Susanna:1:1");
    const rows = try streams(a, std.testing.io, ref);
    try std.testing.expectEqual(@as(usize, 1), rows[0].len);
    try std.testing.expectEqual(@as(usize, 0), rows[1].len);
    try std.testing.expectEqual(@as(usize, 0), rows[2].len);
    const absent = try streams(a, std.testing.io, try Reference.init(a, "John:65535"));
    for (absent) |verses| try std.testing.expectEqual(@as(usize, 0), verses.len);
}

test "native chapter lists include later available chapters" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const rows = try streams(a, std.testing.io, try Reference.init(a, "Jeremiah:2,3,5-6"));
    try std.testing.expect(rows[1].len > 0);
    for (rows[1]) |verse| try std.testing.expect(verse.chapter == 3 or verse.chapter == 5 or verse.chapter == 6);
}

test "native cross chapter ranges do not suppress a source missing first chapter" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const ref = try Reference.init(a, "Jeremiah:2:1-3:3");
    const rows = try streams(a, std.testing.io, ref);
    try std.testing.expect(rows[1].len > 0);
    for (rows[1]) |verse| {
        try std.testing.expectEqual(@as(u16, 3), verse.chapter);
        try std.testing.expect(verse.number <= 3);
    }
}

test "native cross chapter verse ranges retain endpoint labels" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const ref = try Reference.init(a, "John:1:50-2:3");
    const rows = try streams(a, std.testing.io, ref);
    for (rows) |verses| {
        try std.testing.expectEqual(@as(usize, 5), verses.len);
        try std.testing.expectEqual(@as(u16, 50), verses[0].number);
        try std.testing.expectEqual(@as(u16, 2), verses[4].chapter);
        try std.testing.expectEqual(@as(u16, 3), verses[4].number);
    }
}

test "native verse lists and ranges select labels once in source order" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const ref = try Reference.init(a, "John:1:5,1-3,3");
    const rows = try streams(a, std.testing.io, ref);
    for (rows) |verses| {
        try std.testing.expectEqual(@as(usize, 4), verses.len);
        for (verses, [_]u16{ 1, 2, 3, 5 }) |verse, n| try std.testing.expectEqual(n, verse.number);
    }
}

test "native chapter ranges include available chapters across source gaps" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const ref = try Reference.init(a, "Jeremiah:2-3");
    const rows = try streams(a, std.testing.io, ref);
    try std.testing.expect(rows[1].len > 0);
    for (rows[1]) |verse| try std.testing.expectEqual(@as(u16, 3), verse.chapter);
    try std.testing.expectEqual(@as(u16, 2), rows[0][0].chapter);
}

test "bundled fetch works without external executables" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const reference = try Reference.init(a, "John:1:1");
    const verses = try fetch(a, std.testing.io, "kjv", reference);
    try std.testing.expectEqual(@as(usize, 1), verses.len);
    try std.testing.expectEqualStrings("In the beginning was the Word, and the Word was with God, and the Word was God.", verses[0].text);
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

test "source Sirach sentinel and explicit zero chapter preserve real prologue" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const parsed = try parseSource(a, "Sirach\n1:1\tfirst\n0:0\t\n", "Sirach", false);
    try std.testing.expectEqual(@as(usize, 1), parsed.len);
    try std.testing.expectError(error.MalformedOutput, parseSource(a, "Sirach\n1:1\t\n", "Sirach", false));
    const ref = try Reference.init(a, "Sirach:0");
    const prologue = try streams(a, std.testing.io, ref);
    try std.testing.expect(prologue[1].len > 0);
    for (prologue) |verses| for (verses) |verse| try std.testing.expectEqual(@as(u16, 0), verse.chapter);
}

test "source parser preserves repeated Greek labels and zero labels" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const verses = try parseSource(arena.allocator(), "Sirach\n0:0\tprologue\n1:1\tfirst\n1:1\ta addition\n", "Sirach", true);
    try std.testing.expectEqual(@as(usize, 2), verses.len);
    try std.testing.expectEqual(@as(u16, 0), verses[0].chapter);
    try std.testing.expectEqualStrings("first a addition", verses[1].text);
    const selected = try parseSource(arena.allocator(), "Sussana\n1:6\told Greek\nSussana (Theodotion)\n1:1\tvariant\n", "Susanna", true);
    try std.testing.expectEqual(@as(usize, 1), selected.len);
    try std.testing.expectEqualStrings("old Greek", selected[0].text);
    try std.testing.expectError(error.MalformedOutput, parseSource(arena.allocator(), "Sirach\nnot a source heading\n", "Sirach", true));
}

test "streams uses source availability and source-specific names" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const ref = try Reference.init(a, "Judges (Vaticanus):1");
    const rows = try streams(a, std.testing.io, ref);
    try std.testing.expectEqual(@as(usize, 0), rows[0].len);
    try std.testing.expect(rows[1].len > 0);
    try std.testing.expectEqual(@as(usize, 0), rows[2].len);
    const gap = try Reference.init(a, "Jeremiah:2");
    const gap_rows = try streams(a, std.testing.io, gap);
    try std.testing.expect(gap_rows[0].len > 0);
    try std.testing.expectEqual(@as(usize, 0), gap_rows[1].len);
    try std.testing.expect(gap_rows[2].len > 0);
    try std.testing.expectError(error.UnsupportedLocation, fetch(a, std.testing.io, "grb", gap));
    const kings = try Reference.init(a, "2 Kings:25");
    const kings_rows = try streams(a, std.testing.io, kings);
    for (kings_rows) |verses| try std.testing.expect(verses.len > 0);
}

test "Old Testament and variant aliases canonicalize" {
    const reference = try Reference.init(std.testing.allocator, "Gen:50");
    defer std.testing.allocator.free(reference.query);
    try std.testing.expectEqualStrings("Genesis", reference.book);
    try std.testing.expectEqualStrings("Genesis:50", reference.query);
    try std.testing.expectEqualStrings("2 Kings", canonical("2 King").?);
    try std.testing.expectEqualStrings("Susanna (Theodotion)", canonical("SusT").?);
    try std.testing.expectEqualStrings("Susanna", canonical("Sussana").?);
}

test "every discovered book fetches with exact canonical labels and observed chapters" {
    var counts: [3]usize = .{ 0, 0, 0 };
    for (catalog) |entry| {
        try std.testing.expectEqualStrings(entry.name, canonical(entry.name).?);
        for (tools, 0..) |tool, index| {
            if (entry.query_names[index] == null) continue;
            try std.testing.expectEqualStrings(entry.name, canonical(entry.query_names[index].?).?);
            try std.testing.expectEqualStrings(entry.name, canonical(entry.aliases[index].?).?);
            var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
            defer arena.deinit();
            const a = arena.allocator();
            const ref = try Reference.init(a, entry.name);
            const verses = try fetch(a, std.testing.io, tool, ref);
            try std.testing.expect(verses.len > 0);
            for (verses) |verse| {
                try std.testing.expectEqualStrings(entry.name, verse.book);
                try std.testing.expect(std.mem.indexOfScalar(u16, entry.present_chapters[index], verse.chapter) != null);
            }
            for (entry.present_chapters[index]) |chapter| {
                var observed = false;
                for (verses) |verse| if (verse.chapter == chapter) {
                    observed = true;
                    break;
                };
                try std.testing.expect(observed);
            }
            counts[index] += 1;
        }
    }
    try std.testing.expectEqual([3]usize{ 79, 87, 68 }, counts);
    try std.testing.expectEqual(@as(usize, 89), catalog.len);
    try std.testing.expectEqual(@as(usize, 27), books.len);
}

test "reference rejects malformed numeric suffixes before availability" {
    for ([_][]const u8{ "John::1", "John:1-", "John:1,,2", "John:-1", "John:65536", "John:2-1", "John:1:3-1", "John:1:1-2:3:4", "John:1:2:3", "John:1-2:3", "John:2:1-1:50", "John:1:1-2:65536" }) |raw| {
        try std.testing.expectError(error.InvalidReference, Reference.init(std.testing.allocator, raw));
    }
}

test "aliases canonicalize and cannot inject shell arguments" {
    const reference = try Reference.init(std.testing.allocator, "Phi:1:1-3");
    defer std.testing.allocator.free(reference.query);
    try std.testing.expectEqualStrings("Philippians", reference.book);
    try std.testing.expectEqualStrings("Philippians:1:1-3", reference.query);
    try std.testing.expectError(error.InvalidReference, Reference.init(std.testing.allocator, "John:1;echo x"));
}
