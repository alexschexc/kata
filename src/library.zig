const std = @import("std");
const source = @import("source.zig");

pub const Section = struct { name: []const u8, chapters: u16 };
pub const Title = struct {
    id: []const u8,
    name: []const u8,
    description: []const u8,
    sections: []const Section,
    tools: []const []const u8,
    supports_plans: bool,
};
// Only implemented source adapters belong in this registry.
const nt_sections = blk: {
    const counts = [_]u16{ 28, 16, 24, 21, 28, 16, 16, 13, 6, 6, 4, 4, 5, 3, 6, 4, 3, 1, 13, 5, 5, 3, 5, 1, 1, 1, 22 };
    var sections: [source.books.len]Section = undefined;
    for (source.books, counts, 0..) |book, chapters, i| {
        sections[i] = .{ .name = book.name, .chapters = chapters };
    }
    break :blk sections;
};
const bible_sections = blk: {
    var sections: [source.catalog.len]Section = undefined;
    for (source.catalog, 0..) |entry, i| {
        sections[i] = .{ .name = entry.name, .chapters = @max(entry.chapters[0], entry.chapters[1], entry.chapters[2]) };
    }
    break :blk sections;
};
pub const titles = [_]Title{ .{
    .id = "new-testament",
    .name = "New Testament",
    .description = "Read the New Testament with KJV, Greek, and Latin sources.",
    .sections = &nt_sections,
    .tools = &source.tools,
    .supports_plans = true,
}, .{
    .id = "bible",
    .name = "Bible",
    .description = "All books available in the installed KJV, Greek, and Latin sources.",
    .sections = &bible_sections,
    .tools = &source.tools,
    .supports_plans = true,
} };

pub const Location = struct {
    title: usize = 0,
    section: usize = 0,
    chapter: u16 = 1,
    verse: ?u16 = null,
};

pub fn valid(at: Location) bool {
    if (at.title >= titles.len) return false;
    const sections = titles[at.title].sections;
    if (at.section >= sections.len) return false;
    if (at.chapter > sections[at.section].chapters) return false;
    if (at.verse) |verse| if (verse == 0) return false;
    if (at.title == 0) {
        if (at.chapter == 0) return false;
    } else {
        const entry = source.catalog[at.section];
        var present = false;
        for (entry.present_chapters) |chapters| {
            if (std.mem.indexOfScalar(u16, chapters, at.chapter) != null) present = true;
        }
        if (!present) return false;
    }
    return true;
}

/// Allocates a full-chapter query; verse is only a reader anchor.
/// The caller owns the returned buffer.
pub fn reference(allocator: std.mem.Allocator, at: Location) ![]const u8 {
    if (!valid(at)) return error.InvalidLocation;
    return std.fmt.allocPrint(allocator, "{s}:{d}", .{ titles[at.title].sections[at.section].name, at.chapter });
}

/// Moves one chapter without wrapping or preserving a verse anchor.
pub fn adjacent(at: Location, forward: bool) ?Location {
    if (!valid(at)) return null;
    const sections = titles[at.title].sections;
    var next = at;
    next.verse = null;
    const minimum: u16 = if (at.title == 0) 1 else 0;
    while (true) {
        if (forward) {
            if (next.chapter < sections[next.section].chapters) {
                next.chapter += 1;
            } else {
                if (next.section + 1 == sections.len) return null;
                next.section += 1;
                next.chapter = minimum;
            }
        } else {
            if (next.chapter > minimum) {
                next.chapter -= 1;
            } else {
                if (next.section == 0) return null;
                next.section -= 1;
                next.chapter = sections[next.section].chapters;
            }
        }
        if (valid(next)) return next;
    }
}

test "adjacent rejects invalid locations in both directions" {
    const invalid = [_]Location{
        .{ .chapter = 0 },
        .{ .chapter = 29 },
        .{ .title = titles.len },
        .{ .title = std.math.maxInt(usize) },
        .{ .section = titles[0].sections.len },
        .{ .section = std.math.maxInt(usize) },
        .{ .chapter = std.math.maxInt(u16) },
        .{ .verse = 0 },
    };
    for (invalid) |at| {
        try std.testing.expect(adjacent(at, true) == null);
        try std.testing.expect(adjacent(at, false) == null);
    }
}

test "adjacent walks chapters in both directions clearing anchors" {
    for (titles[0..1], 0..) |title, title_index| {
        for (title.sections, 0..) |section, section_index| {
            var chapter: u16 = 1;
            while (chapter <= section.chapters) : (chapter += 1) {
                const at: Location = .{ .title = title_index, .section = section_index, .chapter = chapter, .verse = 1 };
                if (chapter < section.chapters) {
                    try std.testing.expectEqual(Location{ .title = title_index, .section = section_index, .chapter = chapter + 1 }, adjacent(at, true).?);
                } else if (section_index + 1 < title.sections.len) {
                    try std.testing.expectEqual(Location{ .title = title_index, .section = section_index + 1 }, adjacent(at, true).?);
                } else {
                    try std.testing.expect(adjacent(at, true) == null);
                }
                if (chapter > 1) {
                    try std.testing.expectEqual(Location{ .title = title_index, .section = section_index, .chapter = chapter - 1 }, adjacent(at, false).?);
                } else if (section_index > 0) {
                    try std.testing.expectEqual(Location{ .title = title_index, .section = section_index - 1, .chapter = title.sections[section_index - 1].chapters }, adjacent(at, false).?);
                } else {
                    try std.testing.expect(adjacent(at, false) == null);
                }
            }
        }
    }
}

test "reference rejects invalid locations before allocation" {
    const invalid = [_]Location{
        .{ .chapter = 0 },
        .{ .chapter = 29 },
        .{ .title = titles.len },
        .{ .title = std.math.maxInt(usize) },
        .{ .section = titles[0].sections.len },
        .{ .section = std.math.maxInt(usize) },
        .{ .chapter = std.math.maxInt(u16) },
        .{ .verse = 0 },
    };
    for (invalid) |at| {
        try std.testing.expectError(error.InvalidLocation, reference(std.testing.failing_allocator, at));
    }
}

test "reference returns a full canonical chapter even with a verse anchor" {
    const default_query = try reference(std.testing.allocator, .{});
    defer std.testing.allocator.free(default_query);
    try std.testing.expectEqualStrings("Matthew:1", default_query);
    const query = try reference(std.testing.allocator, .{ .section = 3, .chapter = 20, .verse = 7 });
    defer std.testing.allocator.free(query);
    try std.testing.expectEqualStrings("John:20", query);
    const numbered = try reference(std.testing.allocator, .{ .section = 6, .chapter = 16 });
    defer std.testing.allocator.free(numbered);
    try std.testing.expectEqualStrings("1 Corinthians:16", numbered);
}

test "Bible zero chapter is observed and all locations roundtrip" {
    const bible = titles[1];
    try std.testing.expectEqualStrings("Genesis", bible.sections[0].name);
    try std.testing.expectEqual(source.catalog.len, bible.sections.len);
    var sirach: usize = 0;
    for (bible.sections, 0..) |section, index| {
        if (std.mem.eql(u8, section.name, "Sirach")) sirach = index;
    }
    const zero = try reference(std.testing.allocator, .{ .title = 1, .section = sirach, .chapter = 0 });
    defer std.testing.allocator.free(zero);
    try std.testing.expectEqualStrings("Sirach:0", zero);
    try std.testing.expectEqual(Location{ .title = 1, .section = sirach, .chapter = 0 }, adjacent(.{ .title = 1, .section = sirach, .chapter = 1 }, false).?);
    for (bible.sections, 0..) |section, index| {
        var chapter: u16 = 0;
        while (chapter <= section.chapters) : (chapter += 1) {
            const at: Location = .{ .title = 1, .section = index, .chapter = chapter };
            if (!valid(at)) continue;
            const query = try reference(std.testing.allocator, at);
            defer std.testing.allocator.free(query);
            const ref = try source.Reference.init(std.testing.allocator, query);
            defer std.testing.allocator.free(ref.query);
            try std.testing.expect(source.available(ref, 0) or source.available(ref, 1) or source.available(ref, 2));
            if (adjacent(at, true)) |next| try std.testing.expectEqual(at, adjacent(next, false).?);
            if (adjacent(at, false)) |previous| try std.testing.expectEqual(at, adjacent(previous, true).?);
        }
    }
}

test "registry preserves NT bookmarks and adds the discovered Bible" {
    try std.testing.expectEqual(@as(usize, 2), titles.len);
    if (titles.len == 0) return;
    const title = titles[0];
    try std.testing.expectEqualStrings("new-testament", title.id);
    try std.testing.expectEqualStrings("New Testament", title.name);
    try std.testing.expect(title.description.len > 0);
    try std.testing.expect(title.supports_plans);
    try std.testing.expectEqual(source.books.len, title.sections.len);
    const counts = [_]u16{ 28, 16, 24, 21, 28, 16, 16, 13, 6, 6, 4, 4, 5, 3, 6, 4, 3, 1, 13, 5, 5, 3, 5, 1, 1, 1, 22 };
    for (title.sections, source.books, counts) |section, book, count| {
        try std.testing.expectEqualStrings(book.name, section.name);
        try std.testing.expectEqual(count, section.chapters);
    }
    try std.testing.expectEqual(source.tools.len, title.tools.len);
    for (title.tools, source.tools) |actual, expected| try std.testing.expectEqualStrings(expected, actual);
}
