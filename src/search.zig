//! Whole-source word search for one translation, with case- and
//! accent-insensitive word-prefix matching ("love" finds loved, loveth;
//! "λογος" finds Λόγος, λόγου). Results carry the Bible location the reader
//! jumps to; KJV Psalms report Greek/Latin psalm positions with KJV labels.
const std = @import("std");
const bundled = @import("bundled.zig");
const source = @import("source.zig");
const library = @import("library.zig");
const fold_table = @import("fold_table.zig");

/// Lowercase base letter for case- and accent-insensitive comparison.
pub fn fold(cp: u21) u21 {
    if (cp < 0x80) return std.ascii.toLower(@intCast(cp));
    const entries = &fold_table.entries;
    var low: usize = 0;
    var high: usize = entries.len;
    while (low < high) {
        const middle = (low + high) / 2;
        if (entries[middle][0] < cp) low = middle + 1 else high = middle;
    }
    if (low < entries.len and entries[low][0] == cp) return entries[low][1];
    return cp;
}

/// Letters and digits of the scripts Kata carries, after folding.
pub fn isWordChar(folded: u21) bool {
    return switch (folded) {
        '0'...'9', 'a'...'z' => true,
        0xDF...0x24F => folded != 0xF7,
        0x370...0x3FF => switch (folded) {
            0x375, 0x37E, 0x384, 0x385, 0x387 => false,
            else => true,
        },
        else => false,
    };
}

const Char = struct { cp: u21, len: usize };

fn decodeAt(text: []const u8, at: usize) Char {
    const len = std.unicode.utf8ByteSequenceLength(text[at]) catch return .{ .cp = 0xFFFD, .len = 1 };
    if (at + len > text.len) return .{ .cp = 0xFFFD, .len = 1 };
    const cp = std.unicode.utf8Decode(text[at..][0..len]) catch return .{ .cp = 0xFFFD, .len = 1 };
    return .{ .cp = cp, .len = len };
}

pub const max_query = 64;

/// A folded query. Runs of spaces collapse to one; leading/trailing spaces
/// are dropped. Invalid UTF-8 is ignored.
pub const Query = struct {
    cps: [max_query]u21 = undefined,
    len: usize = 0,

    pub fn init(text: []const u8) Query {
        var query: Query = .{};
        var at: usize = 0;
        var pending_space = false;
        while (at < text.len and query.len < max_query) {
            const char = decodeAt(text, at);
            at += char.len;
            if (char.cp == 0xFFFD) continue;
            if (char.cp == ' ' or char.cp == '\t') {
                pending_space = query.len > 0;
                continue;
            }
            if (pending_space and query.len + 1 < max_query) {
                query.cps[query.len] = ' ';
                query.len += 1;
            }
            pending_space = false;
            query.cps[query.len] = fold(char.cp);
            query.len += 1;
        }
        return query;
    }

    pub fn empty(self: *const Query) bool {
        return self.len == 0;
    }
};

pub const Range = struct { start: usize, end: usize };

fn matchAt(text: []const u8, start: usize, query: *const Query) ?usize {
    var at = start;
    for (query.cps[0..query.len]) |want| {
        if (at >= text.len) return null;
        const char = decodeAt(text, at);
        if (fold(char.cp) != want) return null;
        at += char.len;
    }
    return at;
}

/// Next match at or after byte `from`. A query beginning with a letter only
/// matches at the start of a word; the range extends to the end of that word.
pub fn next(text: []const u8, query: *const Query, from: usize) ?Range {
    if (query.len == 0) return null;
    const word_start = isWordChar(query.cps[0]);
    var previous_word = false;
    if (from > 0 and from <= text.len) {
        var back = from - 1;
        while (back > 0 and (text[back] & 0xC0) == 0x80) back -= 1;
        previous_word = isWordChar(fold(decodeAt(text, back).cp));
    }
    var at = from;
    while (at < text.len) {
        const char = decodeAt(text, at);
        if (!word_start or !previous_word) {
            if (matchAt(text, at, query)) |end| {
                var stop = end;
                if (word_start and isWordChar(query.cps[query.len - 1])) {
                    while (stop < text.len) {
                        const following = decodeAt(text, stop);
                        if (!isWordChar(fold(following.cp))) break;
                        stop += following.len;
                    }
                }
                return .{ .start = at, .end = stop };
            }
        }
        previous_word = isWordChar(fold(char.cp));
        at += char.len;
    }
    return null;
}

pub fn count(text: []const u8, query: *const Query) usize {
    var total: usize = 0;
    var at: usize = 0;
    while (next(text, query, at)) |range| {
        total += 1;
        at = range.end;
    }
    return total;
}

pub const Hit = struct {
    /// Index into `source.catalog` (and the Bible title's sections).
    section: usize,
    /// Aligned reader position (Greek/Latin psalm numbering for KJV Psalms).
    chapter: u16,
    number: u16,
    /// The source's own chapter:verse label.
    label: source.Label,
    text: []const u8,
    match: Range,
    occurrences: u32,

    pub fn book(self: Hit) []const u8 {
        return source.catalog[self.section].name;
    }

    pub fn location(self: Hit) library.Location {
        return .{ .title = bible_title, .section = self.section, .chapter = self.chapter, .verse = self.number };
    }
};

const bible_title = blk: {
    for (library.titles, 0..) |title, index| {
        if (std.mem.eql(u8, title.id, "bible")) break :blk index;
    }
    unreachable;
};

fn sectionFor(pane: usize, raw_book: []const u8) ?usize {
    for (source.catalog, 0..) |entry, index| {
        if (entry.query_names[pane]) |name| if (std.mem.eql(u8, name, raw_book)) return index;
    }
    return null;
}

pub const Results = struct { hits: []Hit, occurrences: usize };

/// Every verse of source `pane` containing the query, in source order.
/// Repeated Greek labels are reported once, as the reader shows them.
pub fn find(allocator: std.mem.Allocator, pane: usize, query: *const Query) !Results {
    var hits: std.ArrayList(Hit) = .empty;
    errdefer hits.deinit(allocator);
    var seen = std.AutoHashMap(u64, usize).init(allocator);
    defer seen.deinit();
    var occurrences: usize = 0;
    if (query.empty() or pane >= source.tools.len) return .{ .hits = try hits.toOwnedSlice(allocator), .occurrences = 0 };
    var records = bundled.records(pane);
    var cached_book: []const u8 = "";
    var cached_section: ?usize = null;
    while (records.next()) |record| {
        const first = next(record.text, query, 0) orelse continue;
        const matches: u32 = @intCast(count(record.text, query));
        if (!std.mem.eql(u8, record.book, cached_book)) {
            cached_book = record.book;
            cached_section = sectionFor(pane, record.book);
        }
        const section = cached_section orelse continue;
        var chapter = record.chapter;
        var number = record.number;
        if (pane == 0 and std.mem.eql(u8, source.catalog[section].name, "Psalms")) {
            const aligned = source.kjvPsalmToLxx(chapter, number) orelse continue;
            chapter = aligned.chapter;
            number = aligned.number;
        }
        occurrences += matches;
        const key = (@as(u64, section) << 32) | (@as(u64, chapter) << 16) | number;
        const entry = try seen.getOrPut(key);
        if (entry.found_existing) {
            hits.items[entry.value_ptr.*].occurrences += matches;
            continue;
        }
        entry.value_ptr.* = hits.items.len;
        try hits.append(allocator, .{
            .section = section,
            .chapter = chapter,
            .number = number,
            .label = .{ .chapter = record.chapter, .number = record.number },
            .text = record.text,
            .match = first,
            .occurrences = matches,
        });
    }
    return .{ .hits = try hits.toOwnedSlice(allocator), .occurrences = occurrences };
}

/// Interactive search state, kept by the application across reader sessions
/// so the results panel stays open when a result opens another chapter.
pub const State = struct {
    arena: std.heap.ArenaAllocator = .init(std.heap.page_allocator),
    /// Source searched (and highlighted) by the submitted query.
    pane: usize = 0,
    /// Prompt being typed (`/`), searched in `prompt_pane`.
    prompt: bool = false,
    prompt_pane: usize = 0,
    text: [256]u8 = undefined,
    text_len: usize = 0,
    pending: [4]u8 = undefined,
    pending_len: usize = 0,
    typing: Query = .{},
    /// Submitted search.
    submitted: [256]u8 = undefined,
    submitted_len: usize = 0,
    active: Query = .{},
    hits: []Hit = &.{},
    occurrences: usize = 0,
    selected: usize = 0,
    /// Results panel visible / receiving keys.
    open: bool = false,
    focus: bool = false,
    /// Set when a result opened another reading; the next reader view shows
    /// and focuses the searched source.
    reveal: bool = false,

    pub fn deinit(self: *State) void {
        self.arena.deinit();
    }

    pub fn begin(self: *State, pane: usize) void {
        self.prompt = true;
        self.prompt_pane = pane;
        self.text_len = 0;
        self.pending_len = 0;
        self.typing = .{};
    }

    pub fn promptText(self: *const State) []const u8 {
        return self.text[0..self.text_len];
    }

    pub fn submittedText(self: *const State) []const u8 {
        return self.submitted[0..self.submitted_len];
    }

    /// Accepts printable ASCII and complete UTF-8 sequences, byte by byte.
    pub fn input(self: *State, byte: u8) void {
        if (byte < 0x80) {
            self.pending_len = 0;
            if (byte < 0x20 or byte == 0x7f) return;
            self.append(&.{byte});
        } else if (byte >= 0xC0) {
            self.pending[0] = byte;
            self.pending_len = 1;
        } else if (self.pending_len > 0 and self.pending_len < self.pending.len) {
            self.pending[self.pending_len] = byte;
            self.pending_len += 1;
            const need = std.unicode.utf8ByteSequenceLength(self.pending[0]) catch {
                self.pending_len = 0;
                return;
            };
            if (self.pending_len == need) {
                if (std.unicode.utf8ValidateSlice(self.pending[0..need])) self.append(self.pending[0..need]);
                self.pending_len = 0;
            }
        }
    }

    fn append(self: *State, bytes: []const u8) void {
        if (self.text_len + bytes.len > self.text.len) return;
        @memcpy(self.text[self.text_len..][0..bytes.len], bytes);
        self.text_len += bytes.len;
        self.typing = Query.init(self.promptText());
    }

    pub fn backspace(self: *State) void {
        self.pending_len = 0;
        if (self.text_len == 0) return;
        self.text_len -= 1;
        while (self.text_len > 0 and (self.text[self.text_len] & 0xC0) == 0x80) self.text_len -= 1;
        self.typing = Query.init(self.promptText());
    }

    pub fn clear(self: *State) void {
        self.text_len = 0;
        self.pending_len = 0;
        self.typing = .{};
    }

    pub fn cancel(self: *State) void {
        self.prompt = false;
        self.pending_len = 0;
    }

    /// Runs the typed query over the prompt pane's whole source.
    /// An empty query cancels without disturbing the previous results.
    pub fn submit(self: *State) !void {
        self.prompt = false;
        if (self.typing.empty()) return;
        _ = self.arena.reset(.retain_capacity);
        const results = try find(self.arena.allocator(), self.prompt_pane, &self.typing);
        self.pane = self.prompt_pane;
        self.active = self.typing;
        @memcpy(self.submitted[0..self.text_len], self.promptText());
        self.submitted_len = self.text_len;
        self.hits = results.hits;
        self.occurrences = results.occurrences;
        self.selected = 0;
        self.open = true;
        self.focus = true;
    }

    pub fn close(self: *State) void {
        self.open = false;
        self.focus = false;
        self.active = .{};
        self.hits = &.{};
        self.occurrences = 0;
        self.submitted_len = 0;
        _ = self.arena.reset(.retain_capacity);
    }

    /// Query to highlight in the reader, and in which source pane.
    pub fn highlight(self: *const State) ?struct { pane: usize, query: *const Query } {
        if (self.prompt) return if (self.typing.empty()) null else .{ .pane = self.prompt_pane, .query = &self.typing };
        if (self.active.empty()) return null;
        return .{ .pane = self.pane, .query = &self.active };
    }

    pub fn current(self: *const State) ?Hit {
        if (self.selected >= self.hits.len) return null;
        return self.hits[self.selected];
    }
};

test "folding ignores case, accents, and final sigma" {
    try std.testing.expectEqual(@as(u21, 'a'), fold('A'));
    try std.testing.expectEqual(@as(u21, 0x03B1), fold(0x1F71)); // ά
    try std.testing.expectEqual(@as(u21, 0x03C3), fold(0x03C2)); // ς
    try std.testing.expectEqual(@as(u21, 0x03BB), fold(0x039B)); // Λ
    try std.testing.expectEqual(@as(u21, 'e'), fold(0x00C9)); // É
    try std.testing.expectEqual(@as(u21, 0x2019), fold(0x2019));
}

test "matches start at word boundaries and extend to the word end" {
    const query = Query.init("love");
    const text = "I love; he loveth. Glove beloved LOVE";
    var found: [3]Range = undefined;
    var n: usize = 0;
    var at: usize = 0;
    while (next(text, &query, at)) |range| : (n += 1) {
        found[n] = range;
        at = range.end;
    }
    try std.testing.expectEqual(@as(usize, 3), n);
    try std.testing.expectEqualStrings("love", text[found[0].start..found[0].end]);
    try std.testing.expectEqualStrings("loveth", text[found[1].start..found[1].end]);
    try std.testing.expectEqualStrings("LOVE", text[found[2].start..found[2].end]);
}

test "Greek and Latin match without accents and phrases collapse spaces" {
    const greek = Query.init("λογος");
    const verse = "Ἐν ἀρχῇ ἦν ὁ Λόγος, καὶ ὁ λόγος ἦν πρὸς τὸν θεόν";
    try std.testing.expectEqual(@as(usize, 2), count(verse, &greek));
    const phrase = Query.init("  my   SHEPHERD ");
    try std.testing.expectEqual(@as(usize, 1), count("The LORD is my shepherd; I shall not want.", &phrase));
    const empty = Query.init("   ");
    try std.testing.expect(empty.empty());
    try std.testing.expect(next("anything", &empty, 0) == null);
}

test "every source book maps to a Bible section" {
    for (0..source.tools.len) |pane| {
        var records = bundled.records(pane);
        var last: []const u8 = "";
        while (records.next()) |record| {
            if (std.mem.eql(u8, record.book, last)) continue;
            last = record.book;
            try std.testing.expect(sectionFor(pane, record.book) != null);
        }
    }
}

test "whole-source search finds verses with jumpable locations" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const shepherd = Query.init("shepherd");
    const kjv = try find(a, 0, &shepherd);
    try std.testing.expect(kjv.hits.len > 50);
    var psalm = false;
    for (kjv.hits) |hit| {
        try std.testing.expect(library.valid(hit.location()));
        if (std.mem.eql(u8, hit.book(), "Psalms") and hit.label.chapter == 23 and hit.label.number == 1) {
            try std.testing.expectEqual(@as(u16, 22), hit.chapter);
            psalm = true;
        }
    }
    try std.testing.expect(psalm);
    const logos = Query.init("λογος");
    const grb = try find(a, 1, &logos);
    var john = false;
    for (grb.hits) |hit| {
        if (std.mem.eql(u8, hit.book(), "John") and hit.chapter == 1 and hit.number == 1) {
            try std.testing.expectEqual(@as(u32, 3), hit.occurrences);
            john = true;
        }
    }
    try std.testing.expect(john);
    const verbum = Query.init("verbum");
    const vul = try find(a, 2, &verbum);
    try std.testing.expect(vul.hits.len > 0);
    try std.testing.expect(vul.occurrences >= vul.hits.len);
    const none = Query.init("zzzzqqq");
    try std.testing.expectEqual(@as(usize, 0), (try find(a, 0, &none)).hits.len);
}

test "prompt accepts UTF-8 input and drops malformed bytes" {
    var state: State = .{};
    defer state.deinit();
    state.begin(1);
    for ("λόγ") |byte| state.input(byte);
    state.input(0xCE); // lead byte interrupted by ASCII
    state.input('o');
    try std.testing.expectEqualStrings("λόγo", state.promptText());
    state.backspace();
    state.backspace();
    try std.testing.expectEqualStrings("λό", state.promptText());
    state.clear();
    for ("Λόγος") |byte| state.input(byte);
    try state.submit();
    try std.testing.expect(state.open and state.focus and state.hits.len > 0);
    try std.testing.expectEqual(@as(usize, 1), state.highlight().?.pane);
    state.close();
    try std.testing.expect(state.highlight() == null);
}
