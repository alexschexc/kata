//! In-app reading plan builder. A plan is one or more streams finished
//! together in a chosen number of days; `phases` computes the even per-day
//! rates the plan format needs. A stream is either Bible books read back to
//! back, or one ingested library book read in chapters or sections (see
//! reading_units.zig). Plans are saved as JSON in `<config>/kata/plans/`.
const std = @import("std");
const scheduling = @import("plan.zig");
const book_catalog = @import("book_catalog.zig");
const tui = @import("tui.zig");

const Book = scheduling.Config.Book;
const Phase = scheduling.Config.Phase;
const Stream = scheduling.Config.Stream;

/// An ingested book offered to the builder, with its unit labels.
pub const LibraryOption = struct {
    file: []const u8,
    title: []const u8,
    chapter_name: []const u8 = "chapters",
    section_name: []const u8 = "sections",
    chapters: []const []const u8,
    sections: []const []const u8,

    pub fn labels(self: LibraryOption, unit: []const u8) []const []const u8 {
        return if (std.mem.eql(u8, unit, "section")) self.sections else self.chapters;
    }
};

/// Chapter count used for a catalog book: the largest of its sources.
fn chaptersOf(entry: book_catalog.Entry) u16 {
    return @max(entry.chapters[0], @max(entry.chapters[1], entry.chapters[2]));
}

/// Plan book names must be plain letters, digits, and spaces.
fn plannable(entry: book_catalog.Entry) bool {
    for (entry.name) |c| if (!std.ascii.isAlphanumeric(c) and c != ' ') return false;
    return chaptersOf(entry) > 0;
}

fn total(books: []const Book) usize {
    var n: usize = 0;
    for (books) |b| n += b.chapters;
    return n;
}

/// Phases finishing every stream in exactly `days` days, each stream's
/// daily rate differing by at most one unit (extra units come first).
/// Every day must read something, so `days` ≤ the longest stream.
pub fn phases(allocator: std.mem.Allocator, streams: []const Stream, days: u16) ![]Phase {
    if (streams.len == 0 or days == 0) return error.InvalidPlan;
    var longest: usize = 0;
    for (streams) |s| longest = @max(longest, total(s.books));
    if (days > longest) return error.TooManyDays;
    var cuts: std.ArrayList(u16) = .empty;
    try cuts.append(allocator, 0);
    try cuts.append(allocator, days);
    for (streams) |s| {
        const extra: u16 = @intCast(total(s.books) % days);
        if (extra > 0) try cuts.append(allocator, extra);
    }
    std.mem.sort(u16, cuts.items, {}, std.sort.asc(u16));
    var out: std.ArrayList(Phase) = .empty;
    var i: usize = 0;
    while (i + 1 < cuts.items.len) : (i += 1) {
        const from = cuts.items[i];
        const to = cuts.items[i + 1];
        if (to == from) continue;
        const rates = try allocator.alloc(u16, streams.len);
        for (streams, rates) |s, *rate| {
            const units = total(s.books);
            rate.* = @intCast(units / days + @intFromBool(from < units % days));
        }
        try out.append(allocator, .{ .days = to - from, .rates = rates });
    }
    return out.items;
}

/// Plan JSON (validated by parsing it back with the real plan loader).
pub fn render(allocator: std.mem.Allocator, name: []const u8, repeat: bool, streams: []const Stream, days: u16) ![]u8 {
    const config: scheduling.Config = .{ .name = name, .repeat = repeat, .streams = streams, .phases = try phases(allocator, streams, days) };
    const json = try std.json.Stringify.valueAlloc(allocator, config, .{ .whitespace = .indent_2, .emit_null_optional_fields = false });
    var check = try scheduling.Plan.init(allocator, json);
    check.deinit();
    return json;
}

const Preset = struct { label: []const u8, first: []const u8, last: []const u8 };
const presets = [_]Preset{
    .{ .label = "Gospels (Matthew–John)", .first = "Matthew", .last = "John" },
    .{ .label = "New Testament (Matthew–Revelation)", .first = "Matthew", .last = "Revelation" },
    .{ .label = "Epistles (Romans–Jude)", .first = "Romans", .last = "Jude" },
    .{ .label = "Old Testament (Genesis–Malachi)", .first = "Genesis", .last = "Malachi" },
    .{ .label = "Torah (Genesis–Deuteronomy)", .first = "Genesis", .last = "Deuteronomy" },
    .{ .label = "Psalms", .first = "Psalms", .last = "Psalms" },
    .{ .label = "Wisdom (Job–Song of Solomon)", .first = "Job", .last = "Song of Solomon" },
};

fn catalogIndex(name: []const u8) usize {
    for (book_catalog.catalog, 0..) |entry, i| if (std.mem.eql(u8, entry.name, name)) return i;
    unreachable;
}

fn bookOf(entry: book_catalog.Entry) Book {
    return .{ .name = entry.name, .chapters = chaptersOf(entry) };
}

/// Builder-side stream: Bible books, or one library book (`library` indexes
/// the options) read in `unit`s.
const Draft = struct {
    books: std.ArrayList(Book) = .empty,
    library: ?usize = null,
    unit: []const u8 = "chapter",

    fn stream(self: Draft, allocator: std.mem.Allocator, options: []const LibraryOption) !Stream {
        if (self.library) |i| {
            const option = options[i];
            const books = try allocator.alloc(Book, 1);
            books[0] = .{ .name = option.title, .chapters = @intCast(@min(option.labels(self.unit).len, std.math.maxInt(u16))) };
            return .{ .books = books, .library = option.file, .unit = self.unit };
        }
        return .{ .books = self.books.items };
    }

    fn describe(self: Draft, allocator: std.mem.Allocator, options: []const LibraryOption) ![]const u8 {
        if (self.library) |i| {
            const option = options[i];
            const name = if (std.mem.eql(u8, self.unit, "section")) option.section_name else option.chapter_name;
            return std.fmt.allocPrint(allocator, "{s} · {d} {s}", .{ option.title, option.labels(self.unit).len, name });
        }
        const books = self.books.items;
        if (books.len == 0) return "no books yet";
        const span = if (books.len == 1) books[0].name else try std.fmt.allocPrint(allocator, "{s} … {s} ({d} books)", .{ books[0].name, books[books.len - 1].name, books.len });
        return std.fmt.allocPrint(allocator, "Bible: {s} · {d} chapters", .{ span, total(books) });
    }

    fn empty(self: Draft) bool {
        return self.library == null and self.books.items.len == 0;
    }
};

/// Adds Bible books to `books` from presets or the catalog until Esc.
fn addBooks(allocator: std.mem.Allocator, books: *std.ArrayList(Book)) !void {
    var offered: std.ArrayList(usize) = .empty;
    for (book_catalog.catalog, 0..) |entry, i| if (plannable(entry)) try offered.append(allocator, i);
    var selected: usize = 0;
    while (!tui.isInterrupted()) {
        var labels: std.ArrayList([]const u8) = .empty;
        for (presets) |p| try labels.append(allocator, try std.fmt.allocPrint(allocator, "＋ {s}", .{p.label}));
        for (offered.items) |i| {
            const entry = book_catalog.catalog[i];
            var added = false;
            for (books.items) |b| added = added or std.mem.eql(u8, b.name, entry.name);
            try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s}{s} · {d} chapters", .{ if (added) "✓ " else "  ", entry.name, chaptersOf(entry) }));
        }
        const draft: Draft = .{ .books = books.* };
        const description = try std.fmt.allocPrint(allocator, "Books are read in the order added. This stream: {s}. Enter adds; Esc returns.", .{try draft.describe(allocator, &.{})});
        selected = try tui.choose(allocator, "Add Bible books", description, labels.items, selected, 1) orelse return;
        if (selected < presets.len) {
            const p = presets[selected];
            for (catalogIndex(p.first)..catalogIndex(p.last) + 1) |i| {
                const entry = book_catalog.catalog[i];
                if (plannable(entry)) try books.append(allocator, bookOf(entry));
            }
        } else try books.append(allocator, bookOf(book_catalog.catalog[offered.items[selected - presets.len]]));
    }
}

/// Picks a library book for a stream; null when cancelled.
fn pickLibrary(allocator: std.mem.Allocator, options: []const LibraryOption) !?usize {
    var labels: std.ArrayList([]const u8) = .empty;
    for (options) |o| try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {d} {s} · {d} {s}", .{ o.title, o.chapters.len, o.chapter_name, o.sections.len, o.section_name }));
    return tui.choose(allocator, "Choose library book", "Front matter (title pages, contents, prefaces, introductions, indexes) is left out of plans.", labels.items, 0, 1);
}

fn defaultUnit(option: LibraryOption) []const u8 {
    return if (option.sections.len > option.chapters.len) "section" else "chapter";
}

/// Edits one stream. Returns false when the stream should be deleted.
fn editStream(allocator: std.mem.Allocator, draft: *Draft, number: usize, options: []const LibraryOption) !bool {
    if (draft.empty()) try addBooks(allocator, &draft.books);
    const title = try std.fmt.allocPrint(allocator, "Stream {d}", .{number});
    while (!tui.isInterrupted()) {
        var labels: std.ArrayList([]const u8) = .empty;
        if (draft.library) |i| {
            const option = options[i];
            const sections = std.mem.eql(u8, draft.unit, "section");
            const other = if (sections) "chapter" else "section";
            try labels.append(allocator, try std.fmt.allocPrint(allocator, "Read by {s} ({d}) · Enter switches to {s} ({d})", .{
                if (sections) option.section_name else option.chapter_name,
                option.labels(draft.unit).len,
                if (sections) option.chapter_name else option.section_name,
                option.labels(other).len,
            }));
            try labels.append(allocator, "✕ Delete this stream");
            for (option.labels(draft.unit)) |label| try labels.append(allocator, try std.fmt.allocPrint(allocator, "  {s}", .{label}));
            const choice = try tui.choose(allocator, title, try draft.describe(allocator, options), labels.items, 0, 1) orelse return true;
            switch (choice) {
                0 => draft.unit = other,
                1 => return false,
                else => {},
            }
            continue;
        }
        try labels.append(allocator, "＋ Add books…");
        try labels.append(allocator, "− Remove last book");
        try labels.append(allocator, "✕ Delete this stream");
        for (draft.books.items) |b| try labels.append(allocator, try std.fmt.allocPrint(allocator, "  {s} · {d} chapters", .{ b.name, b.chapters }));
        const choice = try tui.choose(allocator, title, try draft.describe(allocator, options), labels.items, 0, 1) orelse return !draft.empty();
        switch (choice) {
            0 => try addBooks(allocator, &draft.books),
            1 => _ = draft.books.pop(),
            2 => return false,
            else => {},
        }
    }
    return !draft.empty();
}

fn slug(allocator: std.mem.Allocator, name: []const u8) ![]const u8 {
    var out: std.ArrayList(u8) = .empty;
    for (name) |c| {
        if (std.ascii.isAlphanumeric(c)) try out.append(allocator, std.ascii.toLower(c)) else if (out.items.len > 0 and out.items[out.items.len - 1] != '-') try out.append(allocator, '-');
    }
    while (out.items.len > 0 and out.items[out.items.len - 1] == '-') _ = out.pop();
    return if (out.items.len == 0) "plan" else out.items;
}

/// Runs the builder; returns the saved plan file path, or null if cancelled.
/// `options` are the library books offered; `preset` (an option's file)
/// starts the plan with that book as stream 1.
pub fn run(allocator: std.mem.Allocator, io: std.Io, plans_dir: []const u8, options: []const LibraryOption, preset: ?[]const u8) !?[]const u8 {
    var drafts: std.ArrayList(Draft) = .empty;
    var default_name: []const u8 = "";
    if (preset) |file| for (options, 0..) |o, i| if (std.mem.eql(u8, o.file, file)) {
        try drafts.append(allocator, .{ .library = i, .unit = defaultUnit(o) });
        default_name = o.title;
    };
    var name = try tui.prompt(allocator, "New reading plan", "Name the plan (shown in the plan list).", default_name) orelse return null;
    var repeat = preset == null;
    var days: ?u16 = null;
    if (drafts.items.len == 0) {
        try drafts.append(allocator, .{});
        if (!try editStream(allocator, &drafts.items[0], 1, options)) _ = drafts.pop();
    }
    var note: []const u8 = "";
    const fixed_items = 6;
    while (!tui.isInterrupted()) {
        var streams: std.ArrayList(Stream) = .empty;
        var longest: usize = 0;
        for (drafts.items) |d| {
            const s = try d.stream(allocator, options);
            try streams.append(allocator, s);
            longest = @max(longest, total(s.books));
        }
        const length: u16 = days orelse @intCast(@min(longest, std.math.maxInt(u16)));
        var labels: std.ArrayList([]const u8) = .empty;
        try labels.append(allocator, "✓ Save plan");
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "Name: {s}", .{name}));
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "Length: {d} days{s}", .{ length, if (days == null) " (one unit a day for the longest stream)" else "" }));
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "Repeat when finished: {s}", .{if (repeat) "yes" else "no"}));
        try labels.append(allocator, "＋ Add a Bible stream (read alongside the others)");
        try labels.append(allocator, if (options.len > 0) "＋ Add a library book stream" else "＋ Add a library book stream (no converted books yet)");
        for (drafts.items, streams.items, 0..) |d, s, i| {
            const per_day = if (length > 0) @as(f64, @floatFromInt(total(s.books))) / @as(f64, @floatFromInt(length)) else 0;
            try labels.append(allocator, try std.fmt.allocPrint(allocator, "Stream {d}: {s} · ~{d:.1}/day", .{ i + 1, try d.describe(allocator, options), per_day }));
        }
        var preview: []const u8 = "Add a stream to begin.";
        if (streams.items.len > 0 and length > 0) {
            if (render(allocator, name, repeat, streams.items, length)) |json| {
                var plan = try scheduling.Plan.init(allocator, json);
                defer plan.deinit();
                const first = try plan.assignments(allocator, 0);
                var text: std.ArrayList(u8) = .empty;
                try text.appendSlice(allocator, "Day 1: ");
                for (first, 0..) |c, i| {
                    if (i > 0) try text.appendSlice(allocator, ", ");
                    const draft = drafts.items[c.stream];
                    if (draft.library) |o| {
                        try text.appendSlice(allocator, options[o].labels(draft.unit)[c.chapter - 1]);
                    } else try text.print(allocator, "{s} {d}", .{ c.book, c.chapter });
                }
                preview = text.items;
            } else |err| preview = switch (err) {
                error.TooManyDays => try std.fmt.allocPrint(allocator, "Too many days: the longest stream has only {d} units.", .{longest}),
                else => @errorName(err),
            };
        }
        const description = try std.fmt.allocPrint(allocator, "{s}{s}", .{ note, preview });
        note = "";
        const choice = try tui.choose(allocator, "Build reading plan", description, labels.items, 0, 1) orelse return null;
        switch (choice) {
            0 => {
                if (streams.items.len == 0) {
                    note = "Add at least one stream. ";
                    continue;
                }
                const json = render(allocator, name, repeat, streams.items, length) catch {
                    note = "Fix the length first. ";
                    continue;
                };
                const cwd = std.Io.Dir.cwd();
                try cwd.createDirPath(io, plans_dir);
                const stem = try slug(allocator, name);
                var path = try std.fmt.allocPrint(allocator, "{s}/{s}.json", .{ plans_dir, stem });
                var n: usize = 2;
                while (true) : (n += 1) {
                    _ = cwd.statFile(io, path, .{}) catch break;
                    path = try std.fmt.allocPrint(allocator, "{s}/{s}-{d}.json", .{ plans_dir, stem, n });
                }
                try cwd.writeFile(io, .{ .sub_path = path, .data = json });
                return path;
            },
            1 => name = try tui.prompt(allocator, "Plan name", "Name the plan.", name) orelse name,
            2 => {
                const raw = try tui.prompt(allocator, "Plan length", try std.fmt.allocPrint(allocator, "Number of days (1–{d}). Every stream finishes on the last day. 0 restores the default.", .{longest}), "") orelse continue;
                days = std.fmt.parseInt(u16, raw, 10) catch {
                    note = "Enter a whole number of days. ";
                    continue;
                };
                if (days.? == 0) days = null;
            },
            3 => repeat = !repeat,
            4 => {
                try drafts.append(allocator, .{});
                if (!try editStream(allocator, &drafts.items[drafts.items.len - 1], drafts.items.len, options)) _ = drafts.pop();
            },
            5 => {
                if (options.len == 0) {
                    note = "Convert a book first (put an EPUB in the ingest folder). ";
                    continue;
                }
                const picked = try pickLibrary(allocator, options) orelse continue;
                try drafts.append(allocator, .{ .library = picked, .unit = defaultUnit(options[picked]) });
            },
            else => {
                const index = choice - fixed_items;
                if (!try editStream(allocator, &drafts.items[index], index + 1, options)) _ = drafts.orderedRemove(index);
            },
        }
    }
    return null;
}

test "phases finish every stream together with even rates" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const gospels = [_]Book{ .{ .name = "Matthew", .chapters = 28 }, .{ .name = "Mark", .chapters = 16 }, .{ .name = "Luke", .chapters = 24 }, .{ .name = "John", .chapters = 21 } };
    const epistles = [_]Book{ .{ .name = "Romans", .chapters = 16 }, .{ .name = "James", .chapters = 5 } };
    const streams = [_]Stream{ .{ .books = &gospels }, .{ .books = &epistles } };
    const json = try render(a, "Two streams", false, &streams, 30);
    var plan = try scheduling.Plan.init(a, json);
    defer plan.deinit();
    try std.testing.expectEqual(@as(usize, 30), plan.totalDays());
    // 89 chapters / 30 days = 2.97 → 29 days of 3, 1 of 2; 21 / 30 → 21 of 1, 9 of 0.
    var seen: [2]usize = .{ 0, 0 };
    for (0..30) |day| {
        const chapters = try plan.assignments(a, day);
        try std.testing.expect(chapters.len >= 1);
        for (chapters) |c| seen[c.stream] += 1;
    }
    try std.testing.expectEqual([2]usize{ 89, 21 }, seen);
    const last = try plan.assignments(a, 29);
    try std.testing.expectEqualStrings("John", last[last.len - 1].book);
    try std.testing.expectEqual(@as(u16, 21), last[last.len - 1].chapter);
    try std.testing.expectError(error.TooManyDays, phases(a, &streams, 90));
}

test "library streams render with file and unit; titles may use punctuation" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const options = [_]LibraryOption{.{ .file = "bms", .title = "Building Microservices: 2nd Ed.", .chapters = &.{ "1", "2" }, .sections = &.{ "1a", "1b", "2a", "2b", "2c" } }};
    const draft: Draft = .{ .library = 0, .unit = "section" };
    const streams = [_]Stream{ try draft.stream(a, &options), .{ .books = &.{.{ .name = "John", .chapters = 21 }} } };
    const json = try render(a, "Mixed", false, &streams, 5);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"library\": \"bms\"") != null);
    try std.testing.expect(std.mem.indexOf(u8, json, "\"library\": null") == null);
    var plan = try scheduling.Plan.init(a, json);
    defer plan.deinit();
    const day = try plan.assignments(a, 0);
    try std.testing.expectEqual(@as(usize, 0), day[0].stream);
    try std.testing.expectEqual(@as(u16, 1), day[0].chapter);
}

test "catalog presets resolve to plannable books" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var nt: usize = 0;
    for (catalogIndex("Matthew")..catalogIndex("Revelation") + 1) |i| nt += @intFromBool(plannable(book_catalog.catalog[i]));
    try std.testing.expectEqual(@as(usize, 27), nt);
    try std.testing.expectEqualStrings("one-chapter-a-day", try slug(arena.allocator(), "One chapter, a day!"));
}
