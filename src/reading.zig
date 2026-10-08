const std = @import("std");
const library = @import("library.zig");
const source = @import("source.zig");
const layout = @import("layout.zig");
const storage = @import("state.zig");

pub const Bookmark = struct { version: u8 = 1, title_id: []const u8, section: []const u8, chapter: u16 };

pub fn path(allocator: std.mem.Allocator, base: []const u8, at: library.Location, override: ?[]const u8) ![]const u8 {
    const full = try library.reference(allocator, at);
    defer allocator.free(full);
    const reference = try source.Reference.init(allocator, override orelse full);
    defer allocator.free(reference.query);
    const id = library.titles[at.title].id;
    const hash = std.hash.Wyhash.hash(std.hash.Wyhash.hash(0, id), reference.query);
    return std.fmt.allocPrint(allocator, "{s}.library/{s}/{x}.json", .{ base, id, hash });
}
pub fn anchor(rows: []const layout.Row, state: *storage.State, at: library.Location) !void {
    const verse = at.verse orelse return;
    if (!library.valid(at)) return error.InvalidLocation;
    const sections = library.titles[at.title].sections;
    for (rows, 0..) |row, index| {
        if (std.mem.eql(u8, row.book, sections[at.section].name) and row.chapter == at.chapter and row.number == verse) {
            state.positions = .{ .{ .row = index }, .{ .row = index }, .{ .row = index } };
            return;
        }
    }
    return error.InvalidVerse;
}

pub fn fromReference(allocator: std.mem.Allocator, raw: []const u8) !library.Location {
    const reference = try source.Reference.init(allocator, raw);
    defer allocator.free(reference.query);
    for (library.titles, 0..) |title, title_index| {
        for (title.sections, 0..) |section, section_index| {
            if (!std.mem.eql(u8, section.name, reference.book)) continue;
            var numbers = std.mem.tokenizeAny(u8, reference.query[reference.book.len..], ":-,");
            const chapter = if (numbers.next()) |number| try std.fmt.parseInt(u16, number, 10) else 1;
            const at: library.Location = .{ .title = title_index, .section = section_index, .chapter = chapter };
            const checked = try library.reference(allocator, at);
            allocator.free(checked);
            return at;
        }
    }
    return error.InvalidReference;
}

pub fn loadBookmark(allocator: std.mem.Allocator, io: std.Io, base: []const u8) !library.Location {
    const filename = try std.fmt.allocPrint(allocator, "{s}.free-selection.json", .{base});
    defer allocator.free(filename);
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, filename, allocator, .limited(65536)) catch |err| switch (err) {
        error.FileNotFound => return .{},
        else => return err,
    };
    defer allocator.free(bytes);
    const parsed = try std.json.parseFromSlice(Bookmark, allocator, bytes, .{});
    defer parsed.deinit();
    if (parsed.value.version != 1) return error.InvalidFreeBookmark;
    for (library.titles, 0..) |title, title_index| {
        if (!std.mem.eql(u8, title.id, parsed.value.title_id)) continue;
        for (title.sections, 0..) |section, section_index| {
            if (!std.mem.eql(u8, section.name, parsed.value.section)) continue;
            const at: library.Location = .{ .title = title_index, .section = section_index, .chapter = parsed.value.chapter };
            const checked = try library.reference(allocator, at);
            allocator.free(checked);
            return at;
        }
    }
    return error.InvalidFreeBookmark;
}

pub fn saveBookmark(allocator: std.mem.Allocator, io: std.Io, base: []const u8, at: library.Location) !void {
    const checked = try library.reference(allocator, at);
    defer allocator.free(checked);
    const title = library.titles[at.title];
    const bookmark: Bookmark = .{ .title_id = title.id, .section = title.sections[at.section].name, .chapter = at.chapter };
    const bytes = try std.json.Stringify.valueAlloc(allocator, bookmark, .{});
    defer allocator.free(bytes);
    const filename = try std.fmt.allocPrint(allocator, "{s}.free-selection.json", .{base});
    defer allocator.free(filename);
    const temporary = try std.fmt.allocPrint(allocator, "{s}.tmp", .{filename});
    defer allocator.free(temporary);
    const cwd = std.Io.Dir.cwd();
    if (std.fs.path.dirname(filename)) |parent| try cwd.createDirPath(io, parent);
    try cwd.writeFile(io, .{ .sub_path = temporary, .data = bytes });
    try cwd.rename(temporary, cwd, filename, io);
}

test "free chapter state is isolated from base and other chapter state" {
    const first = try path(std.testing.allocator, "/tmp/progress.json", .{ .section = 3, .chapter = 20 }, null);
    defer std.testing.allocator.free(first);
    const next = try path(std.testing.allocator, "/tmp/progress.json", .{ .section = 3, .chapter = 21 }, null);
    defer std.testing.allocator.free(next);
    try std.testing.expect(std.mem.startsWith(u8, first, "/tmp/progress.json.library/new-testament/"));
    try std.testing.expect(!std.mem.eql(u8, first, next));
}

test "starting verse anchors all panes without changing plan counters" {
    const rows = [_]layout.Row{ .{ .book = "John", .chapter = 20, .number = 1 }, .{ .book = "John", .chapter = 20, .number = 5 } };
    var state: storage.State = .{ .next_day = 87, .completed_on = 20261007 };
    try anchor(&rows, &state, .{ .section = 3, .chapter = 20, .verse = 5 });
    try std.testing.expectEqual(@as(usize, 1), state.positions[0].row);
    try std.testing.expectEqual(state.positions[0], state.positions[2]);
    try std.testing.expectEqual(@as(usize, 87), state.next_day);
    try std.testing.expectEqual(@as(i32, 20261007), state.completed_on);
    const before = state;
    try std.testing.expectError(error.InvalidVerse, anchor(&rows, &state, .{ .section = 3, .chapter = 20, .verse = 8 }));
    try std.testing.expectEqualDeep(before, state);
}

test "full collection reference and anchor retain source chapter zero" {
    const at = try fromReference(std.testing.allocator, "Sirach:0");
    try std.testing.expectEqual(@as(u16, 0), at.chapter);
    const rows = [_]layout.Row{.{ .book = "Sirach", .chapter = 0, .number = 1 }};
    var state: storage.State = .{};
    var anchored = at;
    anchored.verse = 1;
    try anchor(&rows, &state, anchored);
}
