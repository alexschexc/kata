const std = @import("std");

pub const Chapter = struct { book: []const u8, chapter: u16 };
pub const Config = struct {
    name: []const u8,
    repeat: bool,
    streams: []const Stream,
    phases: []const Phase,
    pub const Book = struct { name: []const u8, chapters: u16 };
    pub const Stream = struct { books: []const Book };
    pub const Phase = struct { days: u16, rates: []const u16 };
};
pub const Plan = struct {
    parsed: std.json.Parsed(Config),
    pub fn init(allocator: std.mem.Allocator, bytes: []const u8) !Plan {
        const parsed = try std.json.parseFromSlice(Config, allocator, bytes, .{ .allocate = .alloc_always });
        errdefer parsed.deinit();
        try validate(parsed.value);
        return .{ .parsed = parsed };
    }
    /// Returns caller-owned storage; book strings are borrowed until Plan.deinit.
    /// Ordinals are zero-based. Nonrepeating plans return DayOutOfRange after the end.
    pub fn assignments(self: Plan, allocator: std.mem.Allocator, ordinal: usize) ![]Chapter {
        const config = self.parsed.value;
        const duration = self.totalDays();
        if (!config.repeat and ordinal >= duration) return error.DayOutOfRange;
        const day = if (config.repeat) ordinal % duration else ordinal;
        var result: std.ArrayList(Chapter) = .empty;
        errdefer result.deinit(allocator);
        for (config.streams, 0..) |stream, index| {
            var remaining = day;
            var offset: usize = 0;
            var rate: usize = 0;
            for (config.phases) |phase| {
                if (remaining < phase.days) {
                    offset += remaining * phase.rates[index];
                    rate = phase.rates[index];
                    break;
                }
                offset += @as(usize, phase.days) * phase.rates[index];
                remaining -= phase.days;
            }
            for (stream.books) |book| {
                if (offset >= book.chapters) {
                    offset -= book.chapters;
                    continue;
                }
                const count = @min(rate, book.chapters - offset);
                for (0..count) |i| {
                    try result.append(allocator, .{ .book = book.name, .chapter = @intCast(offset + i + 1) });
                }
                rate -= count;
                offset = 0;
                if (rate == 0) break;
            }
        }
        return result.toOwnedSlice(allocator);
    }
    pub fn deinit(self: *Plan) void {
        self.parsed.deinit();
    }
    pub fn name(self: Plan) []const u8 {
        return self.parsed.value.name;
    }
    pub fn totalDays(self: Plan) usize {
        var days: usize = 0;
        for (self.parsed.value.phases) |phase| days += phase.days;
        return days;
    }
};

fn validate(config: Config) !void {
    if (std.mem.trim(u8, config.name, " \t\r\n").len == 0 or config.streams.len == 0 or config.phases.len == 0)
        return error.InvalidPlan;
    var duration: usize = 0;
    for (config.phases) |phase| {
        if (phase.days == 0 or phase.rates.len != config.streams.len) return error.InvalidPlan;
        duration = std.math.add(usize, duration, phase.days) catch return error.InvalidPlan;
        var progress = false;
        for (phase.rates) |rate| progress = progress or rate != 0;
        if (!progress) return error.InvalidPlan;
    }
    for (config.streams, 0..) |stream, index| {
        if (stream.books.len == 0) return error.InvalidPlan;
        var chapters: usize = 0;
        for (stream.books) |book| {
            if (book.chapters == 0 or std.mem.trim(u8, book.name, " ").len == 0) return error.InvalidPlan;
            // Only plain English letters, digits, and spaces are safe identifiers.
            // Canonical Bible-name mapping belongs to the source adapter.
            for (book.name) |character| {
                if (!std.ascii.isAlphanumeric(character) and character != ' ') return error.InvalidPlan;
            }
            chapters = std.math.add(usize, chapters, book.chapters) catch return error.InvalidPlan;
        }
        var assigned: usize = 0;
        for (config.phases) |phase| {
            const progress = std.math.mul(usize, phase.days, phase.rates[index]) catch return error.InvalidPlan;
            assigned = std.math.add(usize, assigned, progress) catch return error.InvalidPlan;
            if (assigned > chapters) return error.InvalidPlan;
        }
        if (assigned != chapters) return error.InvalidPlan;
    }
}

const simple =
    \\{"name":"Simple","repeat":false,"streams":[{"books":[{"name":"1 John","chapters":2}]}],"phases":[{"days":2,"rates":[1]}]}
;
test "parse config and expose name and duration" {
    var plan = try Plan.init(std.testing.allocator, simple);
    defer plan.deinit();
    try std.testing.expectEqualStrings("Simple", plan.name());
    try std.testing.expectEqual(@as(usize, 2), plan.totalDays());
}

test "irregular phases preserve stream and book order and allow paused streams" {
    const bytes =
        \\{"name":"Irregular","repeat":true,"streams":[{"books":[{"name":"Matthew","chapters":1},{"name":"Mark","chapters":2}]},{"books":[{"name":"Acts","chapters":2}]}],"phases":[{"days":1,"rates":[2,0]},{"days":1,"rates":[0,2]},{"days":1,"rates":[1,0]}]}
    ;
    var plan = try Plan.init(std.testing.allocator, bytes);
    defer plan.deinit();
    const first = try plan.assignments(std.testing.allocator, 0);
    defer std.testing.allocator.free(first);
    try std.testing.expectEqual(@as(usize, 2), first.len);
    try std.testing.expectEqualStrings("Matthew", first[0].book);
    try std.testing.expectEqual(@as(u16, 1), first[0].chapter);
    try std.testing.expectEqualStrings("Mark", first[1].book);
    try std.testing.expectEqual(@as(u16, 1), first[1].chapter);
    const second = try plan.assignments(std.testing.allocator, 1);
    defer std.testing.allocator.free(second);
    try std.testing.expectEqual(@as(usize, 2), second.len);
    try std.testing.expectEqualStrings("Acts", second[0].book);
    try std.testing.expectEqual(@as(u16, 2), second[1].chapter);
    const last = try plan.assignments(std.testing.allocator, 2);
    defer std.testing.allocator.free(last);
    try std.testing.expectEqual(@as(usize, 1), last.len);
    try std.testing.expectEqualStrings("Mark", last[0].book);
    try std.testing.expectEqual(@as(u16, 2), last[0].chapter);
    const repeated = try plan.assignments(std.testing.allocator, 3);
    defer std.testing.allocator.free(repeated);
    try std.testing.expectEqualStrings(first[0].book, repeated[0].book);
    try std.testing.expectEqual(first[1].chapter, repeated[1].chapter);
}

test "nonrepeating plans reject out of range ordinals" {
    var plan = try Plan.init(std.testing.allocator, simple);
    defer plan.deinit();
    try std.testing.expectError(error.DayOutOfRange, plan.assignments(std.testing.allocator, 2));
    try std.testing.expectError(error.DayOutOfRange, plan.assignments(std.testing.allocator, std.math.maxInt(usize)));
}

fn expectInvalid(bytes: []const u8) !void {
    if (Plan.init(std.testing.allocator, bytes)) |value| {
        var plan = value;
        plan.deinit();
        return error.TestExpectedError;
    } else |err| {
        try std.testing.expectEqual(error.InvalidPlan, err);
    }
}

test "reject empty configuration and empty collections" {
    const cases = [_][]const u8{
        \\{"name":"","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}
        ,
        \\{"name":"  ","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[],"phases":[{"days":1,"rates":[]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[]}],"phases":[{"days":1,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[]}
    };
    for (cases) |bytes| try expectInvalid(bytes);
}

test "reject zero chapters days progress and mismatched rates" {
    const cases = [_][]const u8{
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":0}]}],"phases":[{"days":1,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":0,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[0]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1,1]}]}
    };
    for (cases) |bytes| try expectInvalid(bytes);
}

test "reject undershoot and overshoot of each stream" {
    const cases = [_][]const u8{
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":3}]}],"phases":[{"days":2,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":2,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":2}]},{"books":[{"name":"Acts","chapters":1}]}],"phases":[{"days":1,"rates":[1,2]}]}
    };
    for (cases) |bytes| try expectInvalid(bytes);
}

test "reject unsafe book names while accepting full English names" {
    const names = [_][]const u8{ "", " ", "../John", "John/Acts", "John\\\\Acts", "John:1", "John;cmd", "John<Acts", "John\\n", "John\\t", "John\\u007f", "John\\u0000" };
    for (names) |book| {
        const bytes = try std.fmt.allocPrint(std.testing.allocator, "{{\"name\":\"X\",\"repeat\":true,\"streams\":[{{\"books\":[{{\"name\":\"{s}\",\"chapters\":1}}]}}],\"phases\":[{{\"days\":1,\"rates\":[1]}}]}}", .{book});
        defer std.testing.allocator.free(bytes);
        try expectInvalid(bytes);
    }
}

// Kept identical to config/optina.json; no runtime filesystem access.
const optina =
    \\{"name":"Optina","repeat":true,"streams":[{"books":[{"name":"Matthew","chapters":28},{"name":"Mark","chapters":16},{"name":"Luke","chapters":24},{"name":"John","chapters":21}]},{"books":[{"name":"Acts","chapters":28},{"name":"Romans","chapters":16},{"name":"1 Corinthians","chapters":16},{"name":"2 Corinthians","chapters":13},{"name":"Galatians","chapters":6},{"name":"Ephesians","chapters":6},{"name":"Philippians","chapters":4},{"name":"Colossians","chapters":4},{"name":"1 Thessalonians","chapters":5},{"name":"2 Thessalonians","chapters":3},{"name":"1 Timothy","chapters":6},{"name":"2 Timothy","chapters":4},{"name":"Titus","chapters":3},{"name":"Philemon","chapters":1},{"name":"Hebrews","chapters":13},{"name":"James","chapters":5},{"name":"1 Peter","chapters":5},{"name":"2 Peter","chapters":3},{"name":"1 John","chapters":5},{"name":"2 John","chapters":1},{"name":"3 John","chapters":1},{"name":"Jude","chapters":1},{"name":"Revelation","chapters":22}]}],"phases":[{"days":82,"rates":[1,2]},{"days":7,"rates":[1,1]}]}
;

fn expectChapter(actual: Chapter, book: []const u8, chapter: u16) !void {
    try std.testing.expectEqualStrings(book, actual.book);
    try std.testing.expectEqual(chapter, actual.chapter);
}

test "Optina boundaries days 1 82 83 89 and 90" {
    var plan = try Plan.init(std.testing.allocator, optina);
    defer plan.deinit();
    try std.testing.expectEqualStrings("Optina", plan.name());
    try std.testing.expectEqual(@as(usize, 89), plan.totalDays());
    const cases = [_]struct { ordinal: usize, gospel: u16, first: u16, count: usize }{
        .{ .ordinal = 0, .gospel = 1, .first = 1, .count = 3 },
        .{ .ordinal = 81, .gospel = 14, .first = 14, .count = 3 },
        .{ .ordinal = 82, .gospel = 15, .first = 16, .count = 2 },
        .{ .ordinal = 88, .gospel = 21, .first = 22, .count = 2 },
        .{ .ordinal = 89, .gospel = 1, .first = 1, .count = 3 },
    };
    for (cases) |case| {
        const chapters = try plan.assignments(std.testing.allocator, case.ordinal);
        defer std.testing.allocator.free(chapters);
        try std.testing.expectEqual(case.count, chapters.len);
        try expectChapter(chapters[0], if (case.ordinal == 0 or case.ordinal == 89) "Matthew" else "John", case.gospel);
        try expectChapter(chapters[1], if (case.ordinal == 0 or case.ordinal == 89) "Acts" else "Revelation", case.first);
        if (case.count == 3) try expectChapter(chapters[2], chapters[1].book, case.first + 1);
    }
}

test "Optina covers every chapter exactly once in ordered streams" {
    var plan = try Plan.init(std.testing.allocator, optina);
    defer plan.deinit();
    var book_indices = [_]usize{ 0, 0 };
    var next_chapters = [_]u16{ 1, 1 };
    var counts = [_]usize{ 0, 0 };
    for (0..plan.totalDays()) |day| {
        const chapters = try plan.assignments(std.testing.allocator, day);
        defer std.testing.allocator.free(chapters);
        const rates = if (day < 82) [_]usize{ 1, 2 } else [_]usize{ 1, 1 };
        var cursor: usize = 0;
        for (rates, 0..) |rate, stream| {
            for (0..rate) |_| {
                const book = plan.parsed.value.streams[stream].books[book_indices[stream]];
                try expectChapter(chapters[cursor], book.name, next_chapters[stream]);
                cursor += 1;
                counts[stream] += 1;
                if (next_chapters[stream] == book.chapters) {
                    book_indices[stream] += 1;
                    next_chapters[stream] = 1;
                } else next_chapters[stream] += 1;
            }
        }
        try std.testing.expectEqual(cursor, chapters.len);
    }
    try std.testing.expectEqualSlices(usize, &.{ 89, 171 }, &counts);
    for (book_indices, 0..) |index, stream| try std.testing.expectEqual(plan.parsed.value.streams[stream].books.len, index);
    const ordinal = std.math.maxInt(usize);
    const expected = try plan.assignments(std.testing.allocator, ordinal % plan.totalDays());
    defer std.testing.allocator.free(expected);
    const actual = try plan.assignments(std.testing.allocator, ordinal);
    defer std.testing.allocator.free(actual);
    try std.testing.expectEqual(expected.len, actual.len);
    for (actual, expected) |a, e| try expectChapter(a, e.book, e.chapter);
}

test "JSON parsing rejects malformed missing unknown and out of u16 bounds fields" {
    const cases = [_][]const u8{
        "", "{}", "not JSON",
        \\{"name":"X","repeat":true,"extra":1,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":65536}]}],"phases":[{"days":1,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":65536,"rates":[1]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[65536]}]}
        ,
        \\{"name":"X","repeat":true,"streams":[{"books":[{"name":"John","chapters":-1}]}],"phases":[{"days":1,"rates":[1]}]}
    };
    for (cases) |bytes| {
        if (Plan.init(std.testing.allocator, bytes)) |value| {
            var plan = value;
            plan.deinit();
            return error.TestExpectedError;
        } else |err| try std.testing.expect(err != error.OutOfMemory);
    }
}

test "u16 chapter limits and durations beyond u16 are safe" {
    const bytes =
        \\{"name":"Long","repeat":true,"streams":[{"books":[{"name":"First","chapters":65535},{"name":"Second","chapters":65535}]}],"phases":[{"days":65535,"rates":[1]},{"days":65535,"rates":[1]}]}
    ;
    var plan = try Plan.init(std.testing.allocator, bytes);
    defer plan.deinit();
    try std.testing.expectEqual(@as(usize, 131070), plan.totalDays());
    const ordinals = [_]usize{ 65534, 65535, 131069, 131070 };
    for (ordinals, 0..) |ordinal, index| {
        const chapters = try plan.assignments(std.testing.allocator, ordinal);
        defer std.testing.allocator.free(chapters);
        try std.testing.expectEqual(@as(usize, 1), chapters.len);
        try expectChapter(chapters[0], if (index == 0 or index == 3) "First" else "Second", if (index == 0 or index == 2) 65535 else 1);
    }
}

test "Plan owns parsed strings independently of the input" {
    const input = try std.testing.allocator.dupe(u8, simple);
    defer std.testing.allocator.free(input);
    var plan = try Plan.init(std.testing.allocator, input);
    defer plan.deinit();
    @memset(input, 'x');
    try std.testing.expectEqualStrings("Simple", plan.name());
    const chapters = try plan.assignments(std.testing.allocator, 1);
    defer std.testing.allocator.free(chapters);
    try expectChapter(chapters[0], "1 John", 2);
}

fn allocationScenario(allocator: std.mem.Allocator) !void {
    var plan = try Plan.init(allocator, optina);
    defer plan.deinit();
    const chapters = try plan.assignments(allocator, 0);
    defer allocator.free(chapters);
    try std.testing.expectEqual(@as(usize, 3), chapters.len);
}

fn invalidAllocationScenario(allocator: std.mem.Allocator) !void {
    const bytes =
        \\{"name":"Invalid","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":2,"rates":[1]}]}
    ;
    if (Plan.init(allocator, bytes)) |value| {
        var plan = value;
        plan.deinit();
        return error.TestExpectedError;
    } else |err| {
        if (err == error.OutOfMemory) return err;
        try std.testing.expectEqual(error.InvalidPlan, err);
    }
}

test "allocation failures release valid and rejected parsing and assignment allocations" {
    try std.testing.checkAllAllocationFailures(std.testing.allocator, allocationScenario, .{});
    try std.testing.checkAllAllocationFailures(std.testing.allocator, invalidAllocationScenario, .{});
}
