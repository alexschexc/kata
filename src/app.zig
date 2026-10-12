const std = @import("std");
const catalog = @import("catalog.zig");
const storage = @import("state.zig");
const scheduling = @import("plan.zig");
const source = @import("source.zig");
const layout = @import("layout.zig");
const tui = @import("tui.zig");
const library = @import("library.zig");
const reading = @import("reading.zig");
const start = @import("start_menu.zig");
const paths = @import("platform_paths.zig");
const search = @import("search.zig");
const ingest = @import("ingest.zig");
const document = @import("document.zig");
const book_reader = @import("book_reader.zig");
const plan_builder = @import("plan_builder.zig");
const reading_units = @import("reading_units.zig");

/// Ingested library state for this run.
const Shelf = struct {
    folders: ingest.Folders = .{},
    entries: []const ingest.IndexEntry = &.{},
    books: []const start.Book = &.{},
    notice: ?[]const u8 = null,
    /// Parsed books kept for the session (search and reopening never
    /// re-read or re-parse). Reset whenever the index is reloaded.
    cache: std.heap.ArenaAllocator = std.heap.ArenaAllocator.init(std.heap.page_allocator),
    docs: []?document.Document = &.{},
    psalter: ?[]const source.Verse = null,
    psalter_loaded: bool = false,
    psalter_name: []const u8 = "htm",
};

fn shelve(allocator: std.mem.Allocator, shelf: *Shelf, entries: []const ingest.IndexEntry) !void {
    var books = try allocator.alloc(start.Book, entries.len);
    for (entries, 0..) |entry, i| books[i] = .{ .title = entry.title, .author = entry.author, .chapters = entry.chapters };
    shelf.entries = entries;
    shelf.books = books;
    _ = shelf.cache.reset(.free_all);
    shelf.docs = try shelf.cache.allocator().alloc(?document.Document, entries.len);
    @memset(shelf.docs, null);
    shelf.psalter = null;
    shelf.psalter_loaded = false;
}

const BookResult = struct { exit: book_reader.Exit, notice: ?[]const u8 = null };

/// Library entry whose title names a Psalter (first match).
fn psalterIndex(shelf: *Shelf) ?usize {
    for (shelf.entries, 0..) |entry, index| {
        if (std.ascii.indexOfIgnoreCase(entry.title, "psalter") != null) return index;
    }
    return null;
}

/// Parsed book `index`, loaded from disk on first use only.
/// The ingested Psalter as Psalms verses (Greek/Latin numbering: psalm from
/// the heading's arabic number, verse numbers as printed). Superscriptions
/// are not verses and are left out. Built once per shelf.
fn psalterVerses(io: std.Io, shelf: *Shelf) ?[]const source.Verse {
    if (shelf.psalter_loaded) return shelf.psalter;
    shelf.psalter_loaded = true;
    const index = psalterIndex(shelf) orelse return null;
    const doc = loadBook(io, shelf, index) catch return null;
    var verses: std.ArrayList(source.Verse) = .empty;
    const allocator = shelf.cache.allocator();
    for (doc.chapters) |chapter| {
        var psalm: u16 = 0;
        for (chapter.blocks) |block| {
            if (block.kind == .heading) {
                psalm = if (book_reader.psalmNumber(block.text)) |n| n else if (block.level <= 1) 0 else psalm;
                continue;
            }
            if (psalm == 0 or block.kind != .verse or block.verse == 0) continue;
            verses.append(allocator, .{ .book = "Psalms", .chapter = psalm, .number = @intCast(@min(block.verse, std.math.maxInt(u16))), .text = block.text }) catch return null;
        }
    }
    if (verses.items.len == 0) return null;
    shelf.psalter = verses.items;
    shelf.psalter_name = "htm";
    return shelf.psalter;
}

/// Verses of `edition` within `reference` (Psalms chapters selected by the
/// reference's own chapter/verse selection).
fn editionVerses(allocator: std.mem.Allocator, edition: []const source.Verse, reference: source.Reference) ![]const source.Verse {
    if (!std.mem.eql(u8, reference.book, "Psalms")) return &.{};
    const colon = std.mem.indexOfScalar(u8, reference.query, ':') orelse reference.query.len;
    const selection = try @import("bundled.zig").Selection.init(reference.query[colon..]);
    var out: std.ArrayList(source.Verse) = .empty;
    for (edition) |verse| if (selection.matches(verse.chapter, verse.number)) try out.append(allocator, verse);
    return out.items;
}

fn loadBook(io: std.Io, shelf: *Shelf, index: usize) !document.Document {
    if (shelf.docs[index]) |doc| return doc;
    const allocator = shelf.cache.allocator();
    const entry = shelf.entries[index];
    const path = try std.fs.path.join(allocator, &.{ shelf.folders.library, entry.file });
    const bytes = try std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(256 << 20));
    const doc = try document.parse(allocator, bytes);
    shelf.docs[index] = doc;
    return doc;
}

/// Reads library book `index` (at `start_at`, else the saved position).
fn openBook(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, index: usize, base: []const u8, caps: @import("graphics.zig").Capabilities) !BookResult {
    return openBookAt(allocator, io, shelf, index, base, caps, null, null, null);
}

fn openBookAt(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, index: usize, base: []const u8, caps: @import("graphics.zig").Capabilities, start_at: ?book_reader.Position, completion: ?book_reader.Completion, portion: ?book_reader.Portion) !BookResult {
    const entry = shelf.entries[index];
    const doc = loadBook(io, shelf, index) catch |err| {
        return .{ .exit = .home, .notice = try std.fmt.allocPrint(allocator, "Cannot open {s}: {s}. Re-run ingest.", .{ entry.title, @errorName(err) }) };
    };
    const position = start_at;
    return .{ .exit = try book_reader.run(std.heap.page_allocator, io, doc, entry.file, base, shelf.folders.library, caps, position, portion, completion) };
}

/// Library-wide search: one query over every converted book, results in a
/// menu; a chosen result opens its book there, and returning from the book
/// comes back to the results.
fn searchBooks(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, base: []const u8, caps: @import("graphics.zig").Capabilities) !BookResult {
    const raw = try tui.prompt(allocator, "Search all books", "Word or phrase (word-prefix, case- and accent-insensitive). Enter searches every converted book; Esc cancels.", "") orelse return .{ .exit = .home };
    const query = search.Query.init(raw);
    if (query.empty()) return .{ .exit = .home };
    tui.status("Searching every converted book…") catch {};
    const Found = struct { book: usize, position: book_reader.Position };
    var found: std.ArrayList(Found) = .empty;
    var labels: std.ArrayList([]const u8) = .empty;
    var occurrences: usize = 0;
    var books_with_hits: usize = 0;
    const limit = 2000;
    for (shelf.entries, 0..) |entry, index| {
        var arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer arena.deinit();
        const doc = loadBook(io, shelf, index) catch continue;
        const results = try book_reader.find(arena.allocator(), doc, &query);
        if (results.hits.len > 0) books_with_hits += 1;
        occurrences += results.occurrences;
        for (results.hits) |hit| {
            if (found.items.len >= limit) break;
            const from = hit.match.start -| 30;
            var begin = from;
            while (begin > 0 and begin < hit.text.len and (hit.text[begin] & 0xC0) == 0x80) begin -= 1;
            const snippet = hit.text[begin..@min(hit.text.len, hit.match.end + 60)];
            var end = snippet.len;
            while (end > 0 and end < snippet.len and (snippet[end] & 0xC0) == 0x80) end -= 1;
            const chapter_title = doc.chapters[hit.section].title;
            try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {s} · {s}{s}", .{ entry.title, chapter_title, if (begin > 0) "…" else "", snippet[0..end] }));
            try found.append(allocator, .{ .book = index, .position = .{ .chapter = hit.section, .block = hit.number - 1 } });
        }
    }
    if (found.items.len == 0) return .{ .exit = .home, .notice = try std.fmt.allocPrint(allocator, "No matches for “{s}” in {d} books.", .{ raw, shelf.entries.len }) };
    const description = try std.fmt.allocPrint(allocator, "“{s}”: {d} passages in {d} book{s} · {d} matches{s} · Enter opens; returning from the book comes back here", .{ raw, found.items.len, books_with_hits, if (books_with_hits == 1) "" else "s", occurrences, if (found.items.len >= limit) " (first 2000 shown)" else "" });
    var selected: usize = 0;
    while (!tui.isInterrupted()) {
        selected = try tui.choose(allocator, "Search results", description, labels.items, selected, 1) orelse return .{ .exit = .home };
        const hit = found.items[selected];
        const result = try openBookAt(allocator, io, shelf, hit.book, base, caps, hit.position, null, null);
        if (result.exit == .quit) return result;
    }
    return .{ .exit = .home };
}

fn hasLibrary(entry: catalog.Entry) bool {
    for (entry.plan.parsed.value.streams) |stream| if (stream.library != null) return true;
    return false;
}

fn shelfIndex(shelf: *const Shelf, file: []const u8) ?usize {
    for (shelf.entries, 0..) |entry, i| if (std.mem.eql(u8, entry.file, file)) return i;
    return null;
}

/// Reading units of library book `index` (cached per shelf book + kind).
fn bookUnits(io: std.Io, shelf: *Shelf, index: usize, unit: []const u8) ![]reading_units.Unit {
    const doc = try loadBook(io, shelf, index);
    return reading_units.units(shelf.cache.allocator(), doc, if (std.mem.eql(u8, unit, "section")) .section else .chapter);
}

/// Every converted book with its plan units, for the plan builder.
fn libraryOptions(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf) ![]plan_builder.LibraryOption {
    tui.status("Measuring chapters and sections of converted books…") catch {};
    var out: std.ArrayList(plan_builder.LibraryOption) = .empty;
    for (shelf.entries, 0..) |entry, i| {
        const doc = loadBook(io, shelf, i) catch continue;
        const chapters = bookUnits(io, shelf, i, "chapter") catch continue;
        const sections = bookUnits(io, shelf, i, "section") catch continue;
        if (chapters.len == 0) continue;
        var chapter_labels = try allocator.alloc([]const u8, chapters.len);
        for (chapters, 0..) |u, k| chapter_labels[k] = u.label;
        var section_labels = try allocator.alloc([]const u8, sections.len);
        for (sections, 0..) |u, k| section_labels[k] = u.label;
        try out.append(allocator, .{ .file = entry.file, .title = entry.title, .chapter_name = reading_units.chapterName(doc), .section_name = reading_units.sectionName(doc), .chapters = chapter_labels, .sections = section_labels });
    }
    return out.items;
}

/// One day's reading, grouped: consecutive Bible chapters form one item;
/// consecutive units of one library stream form one portion.
const DayItem = struct {
    label: []const u8,
    /// Bible references ("John:3") for a Bible item.
    references: []const []const u8 = &.{},
    book: ?usize = null,
    start: book_reader.Position = .{},
    portion: ?book_reader.Portion = null,
};

fn dayItems(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, entry: catalog.Entry, ordinal: usize) ![]DayItem {
    const streams = entry.plan.parsed.value.streams;
    var items: std.ArrayList(DayItem) = .empty;
    const chapters = try entry.plan.assignments(allocator, ordinal);
    var i: usize = 0;
    while (i < chapters.len) {
        const stream = streams[chapters[i].stream];
        var j = i;
        while (j < chapters.len and chapters[j].stream == chapters[i].stream) j += 1;
        if (stream.library) |file| {
            const index = shelfIndex(shelf, file) orelse {
                try items.append(allocator, .{ .label = try std.fmt.allocPrint(allocator, "{s} · not in the library (convert it again)", .{stream.books[0].name}) });
                i = j;
                continue;
            };
            const units = try bookUnits(io, shelf, index, stream.unit.?);
            const first = units[@min(chapters[i].chapter - 1, units.len - 1)];
            const last = units[@min(chapters[j - 1].chapter - 1, units.len - 1)];
            const label = if (i + 1 == j) first.label else try std.fmt.allocPrint(allocator, "{s}  →  {s}", .{ first.label, last.label });
            try items.append(allocator, .{
                .label = try std.fmt.allocPrint(allocator, "{s} · {s}", .{ shelf.entries[index].title, label }),
                .book = index,
                .start = .{ .chapter = first.chapter, .block = first.start },
                .portion = .{ .chapter = last.chapter, .end = last.end, .label = label },
            });
        } else {
            var refs: std.ArrayList([]const u8) = .empty;
            var text: std.ArrayList(u8) = .empty;
            try text.appendSlice(allocator, "Bible · ");
            for (chapters[i..j], 0..) |ch, k| {
                try refs.append(allocator, try std.fmt.allocPrint(allocator, "{s}:{d}", .{ ch.book, ch.chapter }));
                if (k > 0) try text.appendSlice(allocator, ", ");
                try text.print(allocator, "{s} {d}", .{ ch.book, ch.chapter });
            }
            try items.append(allocator, .{ .label = text.items, .references = refs.items });
        }
        i = j;
    }
    return items.items;
}

const HubExit = enum { home, quit };

/// Daily view for plans with library streams: lists today's portions
/// (Bible chapters open in the Bible reader, library portions in the book
/// reader with an end-of-reading marker), marks the day complete, or picks
/// another day. Progress uses the same state files as Bible plans.
fn planHub(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, entry: catalog.Entry, base: []const u8, base_hash: *u64, caps: @import("graphics.zig").Capabilities, finder: *search.State) !HubExit {
    const path = try catalog.progressPath(allocator, base, base_hash.*, entry.hash);
    var state = try storage.load(allocator, io, path);
    if (state.plan_hash != 0 and state.plan_hash != entry.hash) return error.PlanChangedUseSeparateState;
    state.plan_hash = entry.hash;
    try storage.save(allocator, io, path, state);
    if (std.mem.eql(u8, base, path)) base_hash.* = entry.hash;
    try catalog.saveSelected(allocator, io, base, entry.id);
    const plan = entry.plan;
    const days = plan.totalDays();
    var note: []const u8 = "";
    var selected: usize = 0;
    while (!tui.isInterrupted()) {
        const date = try currentDate();
        const ordinal = state.assignment(date);
        const finished = !plan.parsed.value.repeat and ordinal >= days;
        var arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer arena.deinit();
        const work = arena.allocator();
        const items: []DayItem = if (finished) &.{} else try dayItems(work, io, shelf, entry, ordinal);
        var labels: std.ArrayList([]const u8) = .empty;
        for (items) |item| try labels.append(work, try std.fmt.allocPrint(work, "▸ {s}", .{item.label}));
        const done_today = state.completed_on >= date;
        try labels.append(work, if (done_today) "✓ Completed today · next day opens tomorrow" else "✓ Mark today's reading complete");
        try labels.append(work, "Choose day…");
        try labels.append(work, "Back to Library");
        const heading = if (finished) "Plan finished. Choose day… to read again." else try std.fmt.allocPrint(work, "Day {d}/{d}{s} · Enter opens a reading; finish at the end-of-reading marker, then m.", .{ ordinal % days + 1, days, if (done_today) " · completed today" else "" });
        const description = try std.fmt.allocPrint(work, "{s}{s}", .{ note, heading });
        note = "";
        const choice = try tui.choose(work, plan.name(), description, labels.items, @min(selected, labels.items.len - 1), 1) orelse return .home;
        selected = choice;
        if (choice < items.len) {
            const item = items[choice];
            if (item.book) |index| {
                const result = try openBookAt(allocator, io, shelf, index, base, caps, item.start, .{ .state = &state, .path = path, .today = date }, item.portion);
                if (result.exit == .quit) return .quit;
                if (result.notice) |n| note = try std.fmt.allocPrint(allocator, "{s} ", .{n});
            } else if (item.references.len > 0) {
                var rows: std.ArrayList(layout.Row) = .empty;
                for (item.references) |raw| {
                    const reference = try source.Reference.init(work, raw);
                    try rows.appendSlice(work, try layout.alignVerses(work, try source.streams(work, io, reference)));
                }
                const scope = std.hash.Wyhash.hash(entry.hash, std.mem.asBytes(&ordinal));
                if (state.scope_hash != scope) {
                    state.positions = .{ .{}, .{}, .{} };
                    state.scope_hash = scope;
                }
                const action = try tui.runInTerminal(work, rows.items, &state, .{
                    .title = try std.fmt.allocPrint(work, "{s} · day {d}/{d}", .{ plan.name(), ordinal % days + 1, days }),
                    .plan_mode = false,
                    .completable = true,
                    .today = date,
                    .state_path = path,
                    .io = io,
                    .notice = "Plan reading · m returns to the plan",
                    .search = finder,
                });
                try storage.save(allocator, io, path, state);
                if (action == null) return .quit;
            }
            continue;
        }
        switch (choice - items.len) {
            0 => if (!finished) {
                state.complete(date) catch |err| switch (err) {
                    error.AlreadyCompletedToday => {
                        note = "Already completed today. ";
                        continue;
                    },
                    else => return err,
                };
                try storage.save(allocator, io, path, state);
                note = "Completed and saved. ";
            },
            1 => {
                var day_labels: std.ArrayList([]const u8) = .empty;
                for (0..days) |day| {
                    const day_items = try dayItems(work, io, shelf, entry, day);
                    var text: std.ArrayList(u8) = .empty;
                    try text.print(work, "Day {d} · ", .{day + 1});
                    for (day_items, 0..) |item, k| {
                        if (k > 0) try text.appendSlice(work, "  |  ");
                        try text.appendSlice(work, item.label);
                    }
                    try day_labels.append(work, text.items);
                }
                const day = try tui.choose(work, "Choose plan day", "Earlier days in this cycle count as complete; the chosen day is pending.", day_labels.items, ordinal % days, 1) orelse continue;
                if (!plan.parsed.value.repeat) {
                    state.next_day = 0;
                    state.completed_on = 0;
                }
                try state.startAtDay(day + 1, days, date);
                try storage.save(allocator, io, path, state);
                note = try std.fmt.allocPrint(allocator, "Moved to day {d}. ", .{day + 1});
                selected = 0;
            },
            else => return .home,
        }
    }
    return .quit;
}

/// Quick startup check: convert only new/changed EPUBs, then load the index.
fn refreshShelf(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf) !void {
    if (!shelf.folders.configured()) return;
    tui.status("Checking ingest folder for new or changed EPUBs…") catch {};
    const summary = ingest.run(allocator, io, shelf.folders, false) catch |err| {
        shelf.notice = try std.fmt.allocPrint(allocator, "Ingest check failed: {s}. Existing library kept.", .{@errorName(err)});
        try shelve(allocator, shelf, ingest.loadIndex(allocator, io, shelf.folders.library) catch &.{});
        return;
    };
    try shelve(allocator, shelf, ingest.loadIndex(allocator, io, shelf.folders.library) catch &.{});
    if (summary.converted > 0 or summary.failed > 0) {
        shelf.notice = try std.fmt.allocPrint(allocator, "Ingest: {d} converted, {d} failed, {d} unchanged. Run `kata ingest` in a shell for details.", .{ summary.converted, summary.failed, summary.unchanged });
    }
}

/// First-run (or on request) folder choice. Never guesses: the user confirms
/// or edits each absolute path. Returns false when cancelled.
/// Nearest existing directory at or above `path` ("/" at worst).
fn existingAncestor(allocator: std.mem.Allocator, io: std.Io, path: []const u8) ![]const u8 {
    var at = path;
    while (true) {
        if (std.Io.Dir.cwd().openDir(io, at, .{})) |dir| {
            var d = dir;
            d.close(io);
            return allocator.dupe(u8, at);
        } else |_| {}
        at = std.fs.path.dirname(at) orelse return allocator.dupe(u8, "/");
    }
}

/// Subfolder names of `path` (hidden ones only when `hidden`), sorted.
fn listFolders(allocator: std.mem.Allocator, io: std.Io, path: []const u8, hidden: bool) ![]const []const u8 {
    var dir = try std.Io.Dir.cwd().openDir(io, path, .{ .iterate = true });
    defer dir.close(io);
    var names: std.ArrayList([]const u8) = .empty;
    var it = dir.iterate();
    while (try it.next(io)) |entry| {
        if (entry.kind != .directory) continue;
        if (!hidden and entry.name[0] == '.') continue;
        try names.append(allocator, try allocator.dupe(u8, entry.name));
    }
    std.mem.sort([]const u8, names.items, {}, struct {
        fn less(_: void, a: []const u8, b: []const u8) bool {
            return std.ascii.lessThanIgnoreCase(a, b);
        }
    }.less);
    return names.items;
}

/// Menu-driven folder picker starting at `start` (or its nearest existing
/// parent). Returns the chosen absolute path, or null when cancelled.
fn browseFolders(allocator: std.mem.Allocator, io: std.Io, title: []const u8, from: []const u8) !?[]const u8 {
    var current = try existingAncestor(allocator, io, from);
    var hidden = false;
    var note: []const u8 = "";
    while (!tui.isInterrupted()) {
        const folders = listFolders(allocator, io, current, hidden) catch |err| blk: {
            note = try std.fmt.allocPrint(allocator, "Cannot list this folder ({s}). ", .{@errorName(err)});
            break :blk &[_][]const u8{};
        };
        var labels: std.ArrayList([]const u8) = .empty;
        try labels.append(allocator, try std.fmt.allocPrint(allocator, "✓ Use this folder: {s}", .{current}));
        try labels.append(allocator, "↑ Parent folder");
        try labels.append(allocator, "+ New folder here…");
        try labels.append(allocator, if (hidden) "· Hide hidden folders" else "· Show hidden folders");
        for (folders) |name| try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s}/", .{name}));
        const description = try std.fmt.allocPrint(allocator, "{s}Browsing {s} · {d} folders · Enter opens a folder; choose ✓ to use the current one; Esc returns to typing", .{ note, current, folders.len });
        note = "";
        const choice = try tui.choose(allocator, title, description, labels.items, if (folders.len > 0) 4 else 0, 1) orelse return null;
        switch (choice) {
            0 => return current,
            1 => current = std.fs.path.dirname(current) orelse current,
            2 => {
                const name = try tui.prompt(allocator, "New folder", try std.fmt.allocPrint(allocator, "Name of the new folder inside {s}", .{current}), "") orelse continue;
                if (std.mem.indexOfAny(u8, name, "/\\") != null or std.mem.eql(u8, name, ".") or std.mem.eql(u8, name, "..")) {
                    note = "Folder names cannot contain slashes. ";
                    continue;
                }
                const path = try std.fs.path.join(allocator, &.{ current, name });
                std.Io.Dir.cwd().createDirPath(io, path) catch |err| {
                    note = try std.fmt.allocPrint(allocator, "Cannot create {s}: {s}. ", .{ name, @errorName(err) });
                    continue;
                };
                current = path;
            },
            3 => hidden = !hidden,
            else => current = try std.fs.path.join(allocator, &.{ current, folders[choice - 4] }),
        }
    }
    return null;
}

fn chooseFolders(allocator: std.mem.Allocator, io: std.Io, config_home: []const u8, home: ?[]const u8, shelf: *Shelf) !bool {
    const suggestion = if (shelf.folders.configured()) shelf.folders else if (home) |h| try ingest.defaultFolders(allocator, h) else ingest.Folders{};
    var folders: ingest.Folders = .{};
    var error_note: []const u8 = "";
    inline for (.{ "ingest", "library" }) |field| {
        while (true) {
            const description = if (comptime std.mem.eql(u8, field, "ingest"))
                "Where should Kata look for EPUB files to convert? Use an absolute path (~/ is expanded). The folder is created if missing; Kata only reads from it."
            else
                "Where should Kata keep converted books? Each EPUB becomes a readable document with the same name here, plus a small index. Use an absolute path.";
            const full = try std.fmt.allocPrint(allocator, "{s}{s}{s}", .{ description, if (error_note.len > 0) "  ⚠ " else "", error_note });
            const label = if (comptime std.mem.eql(u8, field, "ingest")) "Ingest folder (kataIngest)" else "Library folder (kataLibrary)";
            var raw = try tui.promptWith(allocator, label, full, if (@field(folders, field).len > 0) @field(folders, field) else @field(suggestion, field), true) orelse return false;
            if (raw.len > 0 and raw[0] == tui.browse_marker) {
                const typed = ingest.normalizeFolder(allocator, if (raw.len > 1) raw[1..] else "~/", home) catch home orelse "/";
                const picked = try browseFolders(allocator, io, label, typed) orelse {
                    @field(folders, field) = typed;
                    continue;
                };
                raw = picked;
            }
            @field(folders, field) = ingest.normalizeFolder(allocator, raw, home) catch |err| {
                error_note = switch (err) {
                    error.FolderMustBeAbsolute => "That path is relative; enter an absolute path.",
                    else => @errorName(err),
                };
                continue;
            };
            error_note = "";
            break;
        }
    }
    if (std.mem.eql(u8, folders.ingest, folders.library)) {
        shelf.notice = "Ingest and library folders must differ. Folders unchanged.";
        return false;
    }
    ingest.saveFolders(allocator, io, config_home, folders) catch |err| {
        shelf.notice = try std.fmt.allocPrint(allocator, "Cannot create folders: {s}. Folders unchanged.", .{@errorName(err)});
        return false;
    };
    shelf.folders = folders;
    return true;
}
const c = @cImport({
    @cInclude("time.h");
});

const Session = struct { title: []const u8, rows: []const layout.Row, english_edition: bool = false };
fn currentDate() !i32 {
    var now = c.time(null);
    const local = c.localtime(&now) orelse return error.ClockUnavailable;
    return (local.*.tm_year + 1900) * 10000 + (local.*.tm_mon + 1) * 100 + local.*.tm_mday;
}

fn prepare(allocator: std.mem.Allocator, io: std.Io, entry: catalog.Entry, state: *storage.State, date: i32, passage: ?[]const u8, english: ?[]const source.Verse) !Session {
    var references: std.ArrayList(source.Reference) = .empty;
    var title: []const u8 = undefined;
    var scope: u64 = undefined;
    if (passage) |raw| {
        const reference = try source.Reference.init(allocator, raw);
        try references.append(allocator, reference);
        title = reference.query;
        scope = std.hash.Wyhash.hash(0, title);
    } else {
        state.plan_hash = entry.hash;
        const ordinal = state.assignment(date);
        if (!entry.plan.parsed.value.repeat and ordinal >= entry.plan.totalDays()) {
            return .{ .title = entry.plan.name(), .rows = &.{} };
        }
        const chapters = try entry.plan.assignments(allocator, ordinal);
        for (chapters) |chapter| {
            try references.append(allocator, try source.Reference.init(allocator, try std.fmt.allocPrint(allocator, "{s}:{d}", .{ chapter.book, chapter.chapter })));
        }
        title = try std.fmt.allocPrint(allocator, "{s} · day {d}/{d}", .{ entry.plan.name(), ordinal % entry.plan.totalDays() + 1, entry.plan.totalDays() });
        scope = std.hash.Wyhash.hash(entry.hash, std.mem.asBytes(&ordinal));
    }
    if (state.scope_hash != scope) {
        state.positions = .{ .{}, .{}, .{} };
        state.scope_hash = scope;
    }
    var rows: std.ArrayList(layout.Row) = .empty;
    var used_edition = false;
    for (references.items) |reference| {
        var streams = try source.streams(allocator, io, reference);
        // The Psalter covers only the Psalms; elsewhere column 1 stays KJV.
        if (english) |edition| if (state.edition[0] == 1 and std.mem.eql(u8, reference.book, "Psalms")) {
            streams[0] = try editionVerses(allocator, edition, reference);
            used_edition = true;
        };
        try rows.appendSlice(allocator, try layout.alignVerses(allocator, streams));
    }
    if (rows.items.len == 0) return error.NoVerses;
    return .{ .title = title, .rows = rows.items, .english_edition = used_edition };
}

pub fn run(init: std.process.Init, optina: []const u8, gospels: []const u8, base: []const u8, explicit_plan: ?[]const u8, initial_passage: ?[]const u8) !void {
    const allocator = init.arena.allocator();
    var plans: catalog.Catalog = .{ .allocator = allocator };
    defer plans.deinit();
    _ = try plans.add("builtin:optina", optina);
    _ = try plans.add("builtin:gospels", gospels);
    try plans.discover(init.io, "config");
    const config_home = try paths.root(allocator, init.environ_map, @import("builtin").os.tag == .windows, .config);
    const user_plans = try std.fs.path.join(allocator, &.{ config_home, "kata", "plans" });
    try plans.discover(init.io, user_plans);
    catalog.dropHidden(&plans, try catalog.loadHidden(allocator, init.io, base));
    var selected: usize = 0;
    var notice: []const u8 = "Verse-label alignment; numbering variants are not yet mapped.";
    if (explicit_plan) |path| {
        selected = try plans.addFile(init.io, path);
    } else if (try catalog.loadSelected(allocator, init.io, base)) |id| {
        var found = false;
        for (plans.entries.items, 0..) |entry, index| {
            if (std.mem.eql(u8, entry.id, id)) {
                selected = index;
                found = true;
                break;
            }
        }
        if (!found) {
            selected = plans.addFile(init.io, id) catch blk: {
                notice = "Saved plan unavailable. p selects another; its previous progress is preserved.";
                break :blk 0;
            };
        }
    }
    if (plans.skipped > 0) notice = try std.fmt.allocPrint(allocator, "{d} invalid plan files ignored. p chooses plans. Verse-number variants are unmapped.", .{plans.skipped});
    var base_hash = (try storage.load(allocator, init.io, base)).plan_hash;
    if (explicit_plan != null and initial_passage == null and base_hash != 0 and base_hash != plans.entries.items[selected].hash) return error.PlanChangedUseSeparateState;
    var last_free: library.Location = reading.loadBookmark(allocator, init.io, base) catch .{};
    var active: ?start.Choice = if (initial_passage) |raw| .{ .free = try reading.fromReference(allocator, raw) } else if (explicit_plan != null) .{ .plan = selected } else null;
    var override = initial_passage;
    var finder: search.State = .{};
    defer finder.deinit();
    var build_for: ?[]const u8 = null;
    // Column editions are one app-wide choice (not per reading location).
    const editions_path = try std.fmt.allocPrint(allocator, "{s}.editions", .{base});
    var editions: [3]u8 = .{ 0, 0, 0 };
    if (std.Io.Dir.cwd().readFileAlloc(init.io, editions_path, allocator, .limited(16))) |bytes| {
        for (bytes[0..@min(3, bytes.len)], 0..) |b, i| editions[i] = if (b == '1') 1 else 0;
    } else |_| {}
    var shelf: Shelf = .{};
    const home = init.environ_map.get("HOME") orelse init.environ_map.get("USERPROFILE");
    const folders_saved = ingest.loadFolders(allocator, init.io, config_home) catch |err| blk: {
        notice = try std.fmt.allocPrint(allocator, "Ingest folder settings unreadable ({s}); choose them again from the Library.", .{@errorName(err)});
        break :blk null;
    };
    if (folders_saved) |f| shelf.folders = f;
    var terminal = try tui.Terminal.init();
    defer terminal.deinit();
    // One capability query per launch (bounded); KATA_IMAGES=off disables.
    const graphics_caps: @import("graphics.zig").Capabilities = if (init.environ_map.get("KATA_IMAGES")) |v| (if (std.mem.eql(u8, v, "off")) .{} else tui.detectGraphics(300)) else tui.detectGraphics(300);
    if (active == null) {
        if (folders_saved == null and shelf.notice == null) {
            if (!try chooseFolders(allocator, init.io, config_home, home, &shelf) and shelf.notice == null) {
                shelf.notice = "Ingest folders not chosen. Choose them any time from the last Library entry.";
            }
        }
        try refreshShelf(allocator, init.io, &shelf);
        if (shelf.notice) |message| notice = message;
    }
    while (!tui.isInterrupted()) {
        if (active == null) {
            active = start.run(plans.entries.items, selected, init.io, last_free, notice, shelf.books) catch |err| {
                notice = try std.fmt.allocPrint(allocator, "Cannot open requested reading: {s}. Choose another place or mode.", .{@errorName(err)});
                continue;
            };
            if (active == null) return;
            override = null;
        }
        var free: ?library.Location = null;
        switch (active.?) {
            .plan => |index| selected = index,
            .free => |at| free = at,
            .folders => {
                active = null;
                shelf.notice = null;
                if (try chooseFolders(allocator, init.io, config_home, home, &shelf)) {
                    try refreshShelf(allocator, init.io, &shelf);
                    notice = shelf.notice orelse try std.fmt.allocPrint(allocator, "Folders saved. {d} books in the library.", .{shelf.books.len});
                } else notice = shelf.notice orelse "Folder change cancelled.";
                continue;
            },
            .book => |index| {
                active = null;
                // Book menu: read, or a reading plan that includes this book.
                var choices: std.ArrayList([]const u8) = .empty;
                var plan_rows: std.ArrayList(usize) = .empty;
                try choices.append(allocator, "Read · resume where you left off");
                for (plans.entries.items, 0..) |plan_entry, p| for (plan_entry.plan.parsed.value.streams) |stream| {
                    if (stream.library) |file| if (std.mem.eql(u8, file, shelf.entries[index].file)) {
                        try choices.append(allocator, try std.fmt.allocPrint(allocator, "Reading plan · {s} · {d} days", .{ plan_entry.plan.name(), plan_entry.plan.totalDays() }));
                        try plan_rows.append(allocator, p);
                        break;
                    };
                };
                try choices.append(allocator, "＋ Build a reading plan for this book");
                const pick = try tui.choose(allocator, shelf.entries[index].title, "Read freely, follow a reading plan, or build one.", choices.items, 0, 1) orelse continue;
                if (pick == choices.items.len - 1) {
                    build_for = shelf.entries[index].file;
                    active = .build_plan;
                    continue;
                }
                if (pick > 0) {
                    active = .{ .plan = plan_rows.items[pick - 1] };
                    continue;
                }
                const result = try openBook(allocator, init.io, &shelf, index, base, graphics_caps);
                if (result.exit == .quit) return;
                notice = result.notice orelse "Library";
                continue;
            },
            .plans => {
                active = null;
                var plans_note: []const u8 = "Pick a plan to read, build a new one, or press x to remove one you no longer want.";
                while (!tui.isInterrupted()) {
                    var labels: std.ArrayList([]const u8) = .empty;
                    for (plans.entries.items, 0..) |entry, i| try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {d} days · {s}{s}", .{ entry.plan.name(), entry.plan.totalDays(), try start.coverage(allocator, entry.plan), if (i == selected) " · selected" else "" }));
                    try labels.append(allocator, "＋ Build a new plan · choose books, length, and parallel streams");
                    const pick = try tui.chooseOpts(allocator, "Reading plans", plans_note, labels.items, @min(selected, labels.items.len - 1), 1, .{ .removable_rows = plans.entries.items.len }) orelse break;
                    switch (pick) {
                        .pick => |index| {
                            active = if (index == plans.entries.items.len) .build_plan else .{ .plan = index };
                            break;
                        },
                        .remove => |index| {
                            const entry = plans.entries.items[index];
                            if (plans.entries.items.len <= 1) {
                                plans_note = "Keep at least one plan; build another before removing this one.";
                                continue;
                            }
                            const prefix = try std.fmt.allocPrint(allocator, "{s}{c}", .{ user_plans, std.fs.path.sep });
                            const name = try allocator.dupe(u8, entry.plan.name());
                            if (std.mem.startsWith(u8, entry.id, prefix)) {
                                // Plans you built or added live in your config folder: delete the file.
                                std.Io.Dir.cwd().deleteFile(init.io, entry.id) catch |err| {
                                    plans_note = try std.fmt.allocPrint(allocator, "Could not remove the plan file: {s}.", .{@errorName(err)});
                                    continue;
                                };
                            } else {
                                // Built-in and project-folder plans have no file of ours to delete: hide them.
                                catalog.hide(allocator, init.io, base, entry.id) catch |err| {
                                    plans_note = try std.fmt.allocPrint(allocator, "Could not hide the plan: {s}.", .{@errorName(err)});
                                    continue;
                                };
                            }
                            plans_note = try std.fmt.allocPrint(allocator, "Removed \"{s}\". Saved progress for it was kept.", .{name});
                            plans.remove(index);
                            if (selected == index) {
                                selected = 0;
                                try catalog.saveSelected(allocator, init.io, base, plans.entries.items[0].id);
                            } else if (selected > index) selected -= 1;
                        },
                    }
                }
                continue;
            },
            .build_plan => {
                active = null;
                const options = try libraryOptions(allocator, init.io, &shelf);
                const saved = plan_builder.run(allocator, init.io, user_plans, options, build_for) catch |err| {
                    notice = try std.fmt.allocPrint(allocator, "Plan builder failed: {s}.", .{@errorName(err)});
                    continue;
                } orelse {
                    notice = "Plan builder closed without saving.";
                    continue;
                };
                const index = plans.addFile(init.io, saved) catch |err| {
                    notice = try std.fmt.allocPrint(allocator, "Saved {s} but cannot load it: {s}.", .{ saved, @errorName(err) });
                    continue;
                };
                active = .{ .plan = index };
                build_for = null;
                notice = try std.fmt.allocPrint(allocator, "Plan saved to {s} and selected.", .{saved});
                continue;
            },
            .search_books => {
                active = null;
                const result = try searchBooks(allocator, init.io, &shelf, base, graphics_caps);
                if (result.exit == .quit) return;
                notice = result.notice orelse "Library";
                continue;
            },
        }
        const entry = plans.entries.items[selected];
        if (free == null and hasLibrary(entry)) {
            active = null;
            const exit = planHub(allocator, init.io, &shelf, entry, base, &base_hash, graphics_caps, &finder) catch |err| {
                notice = try std.fmt.allocPrint(allocator, "Cannot open plan: {s}.", .{@errorName(err)});
                continue;
            };
            if (exit == .quit) return;
            notice = "Library";
            continue;
        }
        var session_arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer session_arena.deinit();
        const work = session_arena.allocator();
        const query: ?[]const u8 = if (free) |at| override orelse try library.reference(work, at) else null;
        const path = if (free) |at| try reading.path(work, base, at, override) else try catalog.progressPath(work, base, base_hash, entry.hash);
        var state = storage.load(work, init.io, path) catch |err| {
            notice = try std.fmt.allocPrint(allocator, "Cannot load progress: {s}. Saved data was not changed.", .{@errorName(err)});
            active = null;
            continue;
        };
        if (free == null and state.plan_hash != 0 and state.plan_hash != entry.hash) return error.PlanChangedUseSeparateState;
        if (free != null and (state.plan_hash != 0 or state.next_day != 0 or state.completed_on != 0)) return error.InvalidFreeState;
        const date = try currentDate();
        const psalter = psalterVerses(init.io, &shelf);
        state.edition = if (psalter == null) .{ 0, 0, 0 } else editions;
        var session = prepare(work, init.io, entry, &state, date, query, psalter) catch |err| {
            notice = try std.fmt.allocPrint(allocator, "Cannot load reading: {s}. Progress was not changed.", .{@errorName(err)});
            active = null;
            continue;
        };
        if (free) |at| {
            try reading.anchor(session.rows, &state, at);
            session.title = try std.fmt.allocPrint(work, "{s} · free reading", .{session.title});
            last_free = at;
            last_free.verse = null;
            active = .{ .free = last_free };
        }
        const names: [3][]const u8 = .{ if (session.english_edition) shelf.psalter_name else "kjv", "grb", "vul" };
        finder.names = names;
        finder.editions = .{ if (session.english_edition) psalter else null, null, null };
        const action = try tui.runInTerminal(work, session.rows, &state, .{
            .title = session.title,
            .plan_mode = free == null,
            .today = date,
            .state_path = path,
            .io = init.io,
            .plans = plans.entries.items,
            .active_plan = selected,
            .plan = &plans.entries.items[selected].plan,
            .notice = notice,
            .search = &finder,
            .editions = .{ if (psalter != null and session.rows.len > 0 and std.mem.eql(u8, session.rows[0].book, "Psalms")) 2 else 1, 1, 1 },
            .names = names,
        });
        if (!std.mem.eql(u8, &state.edition, &editions)) {
            editions = state.edition;
            var text: [3]u8 = undefined;
            for (editions, 0..) |e, i| text[i] = '0' + e;
            std.Io.Dir.cwd().writeFile(init.io, .{ .sub_path = editions_path, .data = &text }) catch {};
        }
        state.edition = .{ 0, 0, 0 };
        try storage.save(allocator, init.io, path, state);
        if (free != null) try reading.saveBookmark(allocator, init.io, base, last_free) else {
            try catalog.saveSelected(allocator, init.io, base, entry.id);
            if (std.mem.eql(u8, base, path)) base_hash = state.plan_hash;
        }
        if (action == null) return;
        var target = active.?;
        var changed_day: ?usize = null;
        switch (action.?) {
            .home => {
                active = null;
                override = null;
                continue;
            },
            .open_place => {
                const at = start.choosePlace(init.io, last_free) catch |err| {
                    notice = try std.fmt.allocPrint(allocator, "Cannot choose reading place: {s}. Previous view retained.", .{@errorName(err)});
                    continue;
                } orelse continue;
                target = .{ .free = at };
            },
            .next_chapter, .previous_chapter => {
                const at = free orelse continue;
                const next = library.adjacent(at, action.? == .next_chapter) orelse {
                    notice = "This is the boundary of the selected title. o chooses any place; m opens the library.";
                    continue;
                };
                target = .{ .free = next };
            },
            .choose_plan => |index| target = .{ .plan = index },
            .open_location => |at| target = .{ .free = at },
            .reload => continue,
            .start_day => |day| {
                if (free != null) continue;
                changed_day = day;
            },
        }
        // Validate the requested content before committing any mode/location change.
        var validation = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer validation.deinit();
        const temporary = validation.allocator();
        var target_plan = selected;
        var target_free: ?library.Location = null;
        switch (target) {
            .plan => |index| target_plan = index,
            .free => |at| target_free = at,
            .book, .folders, .search_books, .build_plan, .plans => unreachable, // only chosen from the Library menu
        }
        const target_entry = plans.entries.items[target_plan];
        if (target_free == null and changed_day == null and hasLibrary(target_entry)) {
            active = target;
            continue;
        }
        const target_path = if (target_free) |at| try reading.path(temporary, base, at, null) else try catalog.progressPath(temporary, base, base_hash, target_entry.hash);
        var changed = storage.load(temporary, init.io, target_path) catch |err| {
            notice = try std.fmt.allocPrint(allocator, "Cannot open requested progress: {s}. Previous view retained.", .{@errorName(err)});
            continue;
        };
        if (target_free == null and changed.plan_hash != 0 and changed.plan_hash != target_entry.hash) {
            notice = "Plan identity mismatch. Previous view retained.";
            continue;
        }
        if (changed_day) |day| {
            if (!target_entry.plan.parsed.value.repeat) {
                changed.next_day = 0;
                changed.completed_on = 0;
            }
            changed.startAtDay(day, target_entry.plan.totalDays(), date) catch |err| {
                notice = try std.fmt.allocPrint(allocator, "Cannot change day: {s}", .{@errorName(err)});
                continue;
            };
        }
        const target_query: ?[]const u8 = if (target_free) |at| try library.reference(temporary, at) else null;
        changed.edition = editions;
        const validated = prepare(temporary, init.io, target_entry, &changed, date, target_query, psalterVerses(init.io, &shelf)) catch |err| {
            notice = try std.fmt.allocPrint(allocator, "Cannot load requested reading: {s}. Previous view retained.", .{@errorName(err)});
            continue;
        };
        if (target_free) |at| reading.anchor(validated.rows, &changed, at) catch {
            notice = "Starting verse unavailable. Previous view retained.";
            finder.reveal = false;
            continue;
        };
        changed.edition = .{ 0, 0, 0 };
        try storage.save(allocator, init.io, target_path, changed);
        if (target_free) |at| try reading.saveBookmark(allocator, init.io, base, at) else {
            try catalog.saveSelected(allocator, init.io, base, target_entry.id);
            if (std.mem.eql(u8, base, target_path)) base_hash = changed.plan_hash;
        }
        active = target;
        override = null;
        notice = if (action.? == .open_location) "Search result opened in free reading · n/N next/previous · r results · x closes search" else if (changed_day != null) "Reading position saved. Preceding days count as complete; selected day is pending." else "Reading selected. Free-reading positions and plan progress are kept separately; verse-number variants are unmapped.";
    }
}
