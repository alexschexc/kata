const std = @import("std");

pub const Position = struct { row: usize = 0, line: usize = 0 };
pub const State = struct {
    version: u8 = 1,
    plan_hash: u64 = 0,
    next_day: usize = 0,
    completed_on: i32 = 0,
    scope_hash: u64 = 0,
    positions: [3]Position = .{ .{}, .{}, .{} },
    enabled: [3]bool = .{ true, true, true },
    linked: bool = true,
    focus: usize = 0,

    pub fn assignment(self: State, today: i32) usize {
        return if (self.completed_on >= today and self.next_day > 0) self.next_day - 1 else self.next_day;
    }

    pub fn complete(self: *State, today: i32) !void {
        if (self.completed_on >= today) return error.AlreadyCompletedToday;
        if (self.next_day == std.math.maxInt(usize)) return error.ProgressOverflow;
        self.next_day += 1;
        self.completed_on = today;
    }

    pub fn startAtDay(self: *State, day: usize, duration: usize, today: i32) !void {
        if (duration == 0 or day == 0 or day > duration) return error.InvalidPlanDay;
        const current = self.assignment(today);
        const cycle_start = current - current % duration;
        const target = std.math.add(usize, cycle_start, day - 1) catch return error.ProgressOverflow;
        // Progress is a completed prefix, not a fabricated calendar history.
        // Repositioning makes the selected day pending, even after a completion.
        self.next_day = target;
        self.completed_on = 0;
        self.scope_hash = 0;
        self.positions = .{ .{}, .{}, .{} };
    }

    pub fn validate(self: State) !void {
        if (self.version != 1 or self.focus >= 3) return error.InvalidState;
        if (!self.enabled[0] and !self.enabled[1] and !self.enabled[2]) return error.InvalidState;
        if (self.completed_on < 0) return error.InvalidState;
    }
};

pub fn load(allocator: std.mem.Allocator, io: std.Io, path: []const u8) !State {
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(65536)) catch |err| switch (err) {
        error.FileNotFound => return .{},
        else => return err,
    };
    defer allocator.free(bytes);
    const parsed = try std.json.parseFromSlice(State, allocator, bytes, .{});
    defer parsed.deinit();
    try parsed.value.validate();
    return parsed.value;
}

pub fn save(allocator: std.mem.Allocator, io: std.Io, path: []const u8, state: State) !void {
    try state.validate();
    const cwd = std.Io.Dir.cwd();
    if (std.fs.path.dirname(path)) |parent| try cwd.createDirPath(io, parent);
    const bytes = try std.json.Stringify.valueAlloc(allocator, state, .{ .whitespace = .indent_2 });
    defer allocator.free(bytes);
    const temporary = try std.fmt.allocPrint(allocator, "{s}.tmp", .{path});
    defer allocator.free(temporary);
    try cwd.writeFile(io, .{ .sub_path = temporary, .data = bytes });
    try cwd.rename(temporary, cwd, path, io);
}

test "missed calendar days do not advance or accumulate assignments" {
    var state: State = .{};
    try std.testing.expectEqual(@as(usize, 0), state.assignment(20261003));
    try state.complete(20261003);
    try std.testing.expectEqual(@as(usize, 0), state.assignment(20261003));
    try std.testing.expectEqual(@as(usize, 1), state.assignment(20261004));
    try std.testing.expectEqual(@as(usize, 1), state.assignment(20261008));
    try state.complete(20261008);
    try std.testing.expectEqual(@as(usize, 2), state.assignment(20261009));
}

test "manual completion only once per calendar day" {
    var state: State = .{};
    try state.complete(20261003);
    try std.testing.expectError(error.AlreadyCompletedToday, state.complete(20261003));
    try std.testing.expectError(error.AlreadyCompletedToday, state.complete(20261002));
    try std.testing.expectEqual(@as(usize, 1), state.next_day);
}

test "invalid persisted pane selections fail rather than reset" {
    try std.testing.expectError(error.InvalidState, (State{ .enabled = .{ false, false, false } }).validate());
    try std.testing.expectError(error.InvalidState, (State{ .focus = 99 }).validate());
}

test "import print progress leaves the selected assignment pending today" {
    var state: State = .{ .linked = false, .focus = 1, .scope_hash = 123, .positions = .{ .{ .row = 8 }, .{}, .{} } };
    try state.startAtDay(88, 89, 20261007);
    try std.testing.expectEqual(@as(usize, 87), state.next_day);
    try std.testing.expectEqual(@as(usize, 87), state.assignment(20261007));
    try std.testing.expectEqual(@as(i32, 0), state.completed_on);
    try std.testing.expectEqual(@as(u64, 0), state.scope_hash);
    try std.testing.expectEqual(Position{}, state.positions[0]);
    try std.testing.expect(!state.linked);
    try std.testing.expectEqual(@as(usize, 1), state.focus);
    try state.complete(20261007);
    try std.testing.expectEqual(@as(usize, 88), state.next_day);
    try std.testing.expectEqual(@as(usize, 88), state.assignment(20261008));
}

test "plan jumps reject invalid day ranges without modifying state" {
    var state: State = .{ .next_day = 10, .completed_on = 20261006 };
    const original = state;
    for ([_][2]usize{ .{ 90, 89 }, .{ 0, 89 }, .{ 1, 0 } }) |case| {
        try std.testing.expectError(error.InvalidPlanDay, state.startAtDay(case[0], case[1], 20261007));
        try std.testing.expectEqualDeep(original, state);
    }
}

test "plan jumps preserve finished cycles and allow restarting earlier in this cycle" {
    var state: State = .{ .next_day = 100, .completed_on = 20261007 };
    try state.startAtDay(88, 89, 20261007);
    try std.testing.expectEqual(@as(usize, 176), state.next_day);
    try state.startAtDay(1, 89, 20261007);
    try std.testing.expectEqual(@as(usize, 89), state.next_day);
    try std.testing.expectEqual(@as(i32, 0), state.completed_on);
    // The last assignment is still today's visible cycle after completion.
    state = .{ .next_day = 89, .completed_on = 20261007 };
    try state.startAtDay(88, 89, 20261007);
    try std.testing.expectEqual(@as(usize, 87), state.next_day);
    // On the next date the repeat has begun, so its completed predecessor stays.
    state = .{ .next_day = 89, .completed_on = 20261007 };
    try state.startAtDay(88, 89, 20261008);
    try std.testing.expectEqual(@as(usize, 176), state.next_day);
}

test "overflowing progress jumps fail without modifying state" {
    var state: State = .{ .next_day = std.math.maxInt(usize) };
    const original = state;
    try std.testing.expectError(error.ProgressOverflow, state.startAtDay(2, std.math.maxInt(usize), 20261007));
    try std.testing.expectEqualDeep(original, state);
}
