const std = @import("std");
const library = @import("library.zig");
const source = @import("source.zig");
const catalog = @import("catalog.zig");
const tui = @import("tui.zig");
const scheduling = @import("plan.zig");

pub const Choice = union(enum) { plan: usize, free: library.Location, book: usize, folders, search_books, build_plan, plans };

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

/// "Matthew–John + Acts–Revelation": what a plan reads, one entry per stream.
pub fn coverage(allocator: std.mem.Allocator, plan: scheduling.Plan) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    for (plan.parsed.value.streams, 0..) |stream, i| {
        if (i > 0) try out.appendSlice(allocator, " + ");
        const books = stream.books;
        if (books.len == 0) continue;
        if (stream.library != null or books.len == 1) try out.appendSlice(allocator, books[0].name) else try out.print(allocator, "{s}–{s}", .{ books[0].name, books[books.len - 1].name });
    }
    return out.items;
}

/// True when the plan reads any Bible book (a stream without a library file).
pub fn readsBible(plan: scheduling.Plan) bool {
    for (plan.parsed.value.streams) |stream| if (stream.library == null) return true;
    return false;
}

pub fn run(plans: []const catalog.Entry, selected_plan: usize, io: std.Io, last_free: library.Location, notice: []const u8, books: []const Book) !?Choice {
    var arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
    defer arena.deinit();
    const allocator = arena.allocator();
    var labels: std.ArrayList([]const u8) = .empty;
    // What the `/` filter searches: title and author only, never the descriptions.
    var haystack: std.ArrayList([]const u8) = .empty;
    var selected_title: usize = 0;
    for (visible_titles, 0..) |index, row| {
        const title = library.titles[index];
        if (index == last_free.title) selected_title = row;
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {s}", .{ title.name, title.description }));
        try haystack.append(allocator, title.name);
    }
    try haystack.append(allocator, "Reading plans");
    try labels.append(allocator, try std.fmt.allocPrint(allocator, "Reading plans · {d} {s} · daily assignments, saved progress, build or remove plans", .{ plans.len, if (plans.len == 1) "plan" else "plans" }));
    for (books) |book| {
        try haystack.append(allocator, try std.fmt.allocPrint(allocator, "{s} {s}", .{ book.title, book.author }));
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s}{s}{s} · {d} chapters", .{ book.title, if (book.author.len > 0) " · " else "", book.author, book.chapters }));
    }
    if (books.len > 0) try haystack.append(allocator, "Search all books");
    if (books.len > 0) try labels.append(allocator, "Search all books · find a word or phrase across every converted book");
    try haystack.append(allocator, "Ingest folders");
    try labels.append(allocator, "Ingest folders · choose where EPUBs are read from and converted books are kept");
    while (!tui.isInterrupted()) {
        const picked = try tui.chooseOpts(allocator, "Library", notice, labels.items, selected_title, 1, .{ .haystack = haystack.items }) orelse return null;
        const title_row = picked.pick;
        const plans_row = visible_titles.len;
        const books_end = plans_row + 1 + books.len;
        if (title_row == plans_row) return .plans;
        if (title_row < books_end and title_row > plans_row) return .{ .book = title_row - plans_row - 1 };
        if (books.len > 0 and title_row == books_end) return .search_books;
        if (title_row > books_end or (books.len == 0 and title_row == books_end)) return .folders;
        const title_index = visible_titles[title_row];
        // A title offers free reading and its own plans; all plans (any book) are also under Library → Reading plans.
        const title = library.titles[title_index];
        while (!tui.isInterrupted()) {
            var choices: std.ArrayList([]const u8) = .empty;
            var rows: std.ArrayList(usize) = .empty;
            try choices.append(allocator, "Free reading · choose any book/chapter/verse");
            if (title.supports_plans) {
                for (plans, 0..) |entry, i| {
                    if (!readsBible(entry.plan)) continue;
                    try rows.append(allocator, i);
                    try choices.append(allocator, try std.fmt.allocPrint(allocator, "Plan · {s} · {d} days · {s}{s}", .{ entry.plan.name(), entry.plan.totalDays(), try coverage(allocator, entry.plan), if (i == selected_plan) " · selected" else "" }));
                }
                try choices.append(allocator, "＋ Build a new plan · choose books, length, and parallel streams");
            }
            const mode = try tui.choose(allocator, title.name, "Read freely, follow a plan, or build one. Plans for your other books are under Library → Reading plans.", choices.items, 0, 1) orelse break;
            if (mode == 0) {
                if (try choosePlace(io, menuLocation(last_free, title_index))) |at| return .{ .free = at };
            } else if (mode == choices.items.len - 1) return .build_plan else return .{ .plan = rows.items[mode - 1] };
        }
    }
    return null;
}
