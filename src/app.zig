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
};

fn shelve(allocator: std.mem.Allocator, shelf: *Shelf, entries: []const ingest.IndexEntry) !void {
    var books = try allocator.alloc(start.Book, entries.len);
    for (entries, 0..) |entry, i| books[i] = .{ .title = entry.title, .author = entry.author, .chapters = entry.chapters };
    shelf.entries = entries;
    shelf.books = books;
    _ = shelf.cache.reset(.free_all);
    shelf.docs = try shelf.cache.allocator().alloc(?document.Document, entries.len);
    @memset(shelf.docs, null);
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

/// Reads library book `index`; `psalm` opens a Psalter at that psalm,
/// `start` at a block (otherwise the saved position).
fn openBook(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, index: usize, base: []const u8, caps: @import("graphics.zig").Capabilities, psalm: ?u16) !BookResult {
    return openBookAt(allocator, io, shelf, index, base, caps, psalm, null);
}

fn openBookAt(allocator: std.mem.Allocator, io: std.Io, shelf: *Shelf, index: usize, base: []const u8, caps: @import("graphics.zig").Capabilities, psalm: ?u16, start_at: ?book_reader.Position) !BookResult {
    const entry = shelf.entries[index];
    const doc = loadBook(io, shelf, index) catch |err| {
        return .{ .exit = .home, .notice = try std.fmt.allocPrint(allocator, "Cannot open {s}: {s}. Re-run ingest.", .{ entry.title, @errorName(err) }) };
    };
    var position = start_at;
    if (psalm) |number| {
        position = book_reader.psalmPosition(doc, number) orelse
            return .{ .exit = .home, .notice = try std.fmt.allocPrint(allocator, "Psalm {d} not found in {s}.", .{ number, entry.title }) };
    }
    return .{ .exit = try book_reader.run(std.heap.page_allocator, io, doc, entry.file, base, shelf.folders.library, caps, position) };
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
        const result = try openBookAt(allocator, io, shelf, hit.book, base, caps, null, hit.position);
        if (result.exit == .quit) return result;
    }
    return .{ .exit = .home };
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
            const raw = try tui.prompt(allocator, if (comptime std.mem.eql(u8, field, "ingest")) "Ingest folder (kataIngest)" else "Library folder (kataLibrary)", full, @field(suggestion, field)) orelse return false;
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

const Session = struct { title: []const u8, rows: []const layout.Row };
fn currentDate() !i32 {
    var now = c.time(null);
    const local = c.localtime(&now) orelse return error.ClockUnavailable;
    return (local.*.tm_year + 1900) * 10000 + (local.*.tm_mon + 1) * 100 + local.*.tm_mday;
}

fn prepare(allocator: std.mem.Allocator, io: std.Io, entry: catalog.Entry, state: *storage.State, date: i32, passage: ?[]const u8) !Session {
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
    for (references.items) |reference| {
        const streams = try source.streams(allocator, io, reference);
        try rows.appendSlice(allocator, try layout.alignVerses(allocator, streams));
    }
    if (rows.items.len == 0) return error.NoVerses;
    return .{ .title = title, .rows = rows.items };
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
                const result = try openBook(allocator, init.io, &shelf, index, base, graphics_caps, null);
                if (result.exit == .quit) return;
                notice = result.notice orelse "Library";
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
        var session = prepare(work, init.io, entry, &state, date, query) catch |err| {
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
        });
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
            .open_psalter => |psalm| {
                const index = psalterIndex(&shelf) orelse {
                    notice = "No ingested Psalter found. Put a Psalter EPUB in the ingest folder; P then opens it at the current psalm.";
                    continue;
                };
                const result = try openBook(allocator, init.io, &shelf, index, base, graphics_caps, psalm);
                if (result.exit == .quit) return;
                notice = result.notice orelse "Back from the Psalter.";
                continue;
            },
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
            .book, .folders, .search_books => unreachable, // only chosen from the Library menu
        }
        const target_entry = plans.entries.items[target_plan];
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
        const validated = prepare(temporary, init.io, target_entry, &changed, date, target_query) catch |err| {
            notice = try std.fmt.allocPrint(allocator, "Cannot load requested reading: {s}. Previous view retained.", .{@errorName(err)});
            continue;
        };
        if (target_free) |at| reading.anchor(validated.rows, &changed, at) catch {
            notice = "Starting verse unavailable. Previous view retained.";
            finder.reveal = false;
            continue;
        };
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
