const std = @import("std");
const library = @import("library.zig");
const source = @import("source.zig");
const catalog = @import("catalog.zig");
const tui = @import("tui.zig");

pub const Choice = union(enum) { plan: usize, free: library.Location, book: usize, folders };

/// Ingested documents offered in the Library after the built-in titles.
pub const Book = struct { title: []const u8, author: []const u8 = "", chapters: u32 = 0 };

test "visible library excludes the duplicated legacy New Testament" {
    try std.testing.expectEqual(@as(usize, 1), visible_titles.len);
    try std.testing.expectEqualStrings("bible", library.titles[visible_titles[0]].id);
}

const visible_titles = blk: {
    var count: usize = 0;
    for (library.titles) |title| {
        if (!std.mem.eql(u8, title.id, "new-testament")) count += 1;
    }
    var indexes: [count]usize = undefined;
    var next: usize = 0;
    for (library.titles, 0..) |title, i| {
        if (std.mem.eql(u8, title.id, "new-testament")) continue;
        indexes[next] = i;
        next += 1;
    }
    break :blk indexes;
};

fn menuLocation(previous: library.Location, title_index: usize) library.Location {
    if (previous.title == title_index) return previous;
    if (library.valid(previous)) {
        const name = library.titles[previous.title].sections[previous.section].name;
        for (library.titles[title_index].sections, 0..) |section, index| {
            if (!std.mem.eql(u8, name, section.name)) continue;
            const mapped: library.Location = .{ .title = title_index, .section = index, .chapter = previous.chapter, .verse = previous.verse };
            if (library.valid(mapped)) return mapped;
        }
    }
    return .{ .title = title_index };
}

test "legacy New Testament location opens the same book in Bible" {
    const at = menuLocation(.{ .section = 3, .chapter = 20 }, visible_titles[0]);
    const query = try library.reference(std.testing.allocator, at);
    defer std.testing.allocator.free(query);
    try std.testing.expectEqualStrings("John:20", query);
}

pub fn choosePlace(io: std.Io, initial: library.Location) !?library.Location {
    var arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    var at = initial;
    if (at.title >= library.titles.len) return error.InvalidLocation;
    const title = library.titles[at.title];
    var books: std.ArrayList([]const u8) = .empty;
    for (title.sections) |section| try books.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {d} chapters", .{ section.name, section.chapters }));
    var stage: enum { book, chapter, verse } = .book;
    while (!tui.isInterrupted()) {
        switch (stage) {
            .book => {
                const section = try tui.choose(allocator, "Choose book", title.name, books.items, at.section, 1) orelse return null;
                if (at.section != section) at.chapter = 1;
                at.section = section;
                stage = .chapter;
            },
            .chapter => {
                const section = title.sections[at.section];
                var chapters: std.ArrayList([]const u8) = .empty;
                var chapter_numbers: std.ArrayList(u16) = .empty;
                var selected: usize = 0;
                for (0..@as(usize, section.chapters) + 1) |chapter| {
                    const candidate: library.Location = .{ .title = at.title, .section = at.section, .chapter = @intCast(chapter) };
                    const checked = library.reference(allocator, candidate) catch |err| switch (err) {
                        error.InvalidLocation => continue,
                        else => return err,
                    };
                    allocator.free(checked);
                    if (at.chapter == chapter) selected = chapters.items.len;
                    try chapter_numbers.append(allocator, @intCast(chapter));
                    try chapters.append(allocator, try std.fmt.allocPrint(allocator, "Chapter {d}", .{chapter}));
                }
                const chapter = try tui.choose(allocator, "Choose chapter", section.name, chapters.items, selected, 1) orelse {
                    stage = .book;
                    continue;
                };
                at.chapter = chapter_numbers.items[chapter];
                at.verse = null;
                stage = .verse;
            },
            .verse => {
                const query = try library.reference(allocator, at);
                const reference = try source.Reference.init(allocator, query);
                const streams = try source.streams(allocator, io, reference);
                var verses: []const source.Verse = &.{};
                for (streams) |stream| {
                    if (stream.len > 0) {
                        verses = stream;
                        break;
                    }
                }
                if (verses.len == 0) return error.NoVerses;
                var labels: std.ArrayList([]const u8) = .empty;
                try labels.append(allocator, "Resume saved position, or chapter beginning if new");
                for (verses) |verse| {
                    const label = if (verse.label) |own|
                        try std.fmt.allocPrint(allocator, "Verse {d}:{d} · {s}", .{ own.chapter, own.number, verse.text })
                    else
                        try std.fmt.allocPrint(allocator, "Verse {d} · {s}", .{ verse.number, verse.text });
                    try labels.append(allocator, label);
                }
                const verse = try tui.choose(allocator, "Choose starting verse", query, labels.items, 0, 0) orelse {
                    stage = .chapter;
                    continue;
                };
                at.verse = if (verse == 0 or verses[verse - 1].number == 0) null else verses[verse - 1].number;
                return at;
            },
        }
    }
    return null;
}

pub fn run(plans: []const catalog.Entry, selected_plan: usize, io: std.Io, last_free: library.Location, notice: []const u8, books: []const Book) !?Choice {
    var arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    var labels: std.ArrayList([]const u8) = .empty;
    var selected_title: usize = 0;
    for (visible_titles, 0..) |index, row| {
        const title = library.titles[index];
        if (index == last_free.title) selected_title = row;
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {s}", .{ title.name, title.description }));
    }
    for (books) |book| {
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s}{s}{s} · {d} chapters", .{ book.title, if (book.author.len > 0) " · " else "", book.author, book.chapters }));
    }
    try labels.append(allocator, "Ingest folders · choose where EPUBs are read from and converted books are kept");
    while (!tui.isInterrupted()) {
        const title_row = try tui.choose(allocator, "Library", notice, labels.items, selected_title, 1) orelse return null;
        if (title_row >= visible_titles.len + books.len) return .folders;
        if (title_row >= visible_titles.len) return .{ .book = title_row - visible_titles.len };
        const title_index = visible_titles[title_row];
        const title = library.titles[title_index];
        while (!tui.isInterrupted()) {
            const modes: []const []const u8 = if (title.supports_plans) &.{ "Free reading · choose any book/chapter/verse", "Reading plans · daily assignments and saved progress" } else &.{"Free reading · choose a place"};
            const mode = try tui.choose(allocator, "Reading mode", title.name, modes, 0, 1) orelse break;
            if (mode == 0) {
                const initial = menuLocation(last_free, title_index);
                if (try choosePlace(io, initial)) |at| return .{ .free = at };
            } else {
                var choices: std.ArrayList([]const u8) = .empty;
                for (plans, 0..) |entry, i| try choices.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {d} days{s}", .{ entry.plan.name(), entry.plan.totalDays(), if (i == selected_plan) " · selected" else "" }));
                if (try tui.choose(allocator, "Choose plan", title.name, choices.items, selected_plan, 1)) |index| return .{ .plan = index };
            }
        }
    }
    return null;
}
