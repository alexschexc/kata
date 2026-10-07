const std = @import("std");
const source = @import("source.zig");
const scheduling = @import("plan.zig");
const storage = @import("state.zig");
const layout = @import("layout.zig");
const tui = @import("tui.zig");
const catalog = @import("catalog.zig");
const c = @cImport({
    @cInclude("time.h");
});

fn output(allocator: std.mem.Allocator, comptime format: []const u8, args: anytype) !void {
    const text = try std.fmt.allocPrint(allocator, format, args);
    defer allocator.free(text);
    try tui.writeAll(text);
}

fn today() !i32 {
    var now = c.time(null);
    const local = c.localtime(&now) orelse return error.ClockUnavailable;
    return (local.*.tm_year + 1900) * 10000 + (local.*.tm_mon + 1) * 100 + local.*.tm_mday;
}

fn parsePlanDay(raw: []const u8) !usize {
    if (raw.len == 0) return error.InvalidPlanDay;
    for (raw) |ch| if (!std.ascii.isDigit(ch)) return error.InvalidPlanDay;
    const day = std.fmt.parseInt(usize, raw, 10) catch return error.InvalidPlanDay;
    if (day == 0) return error.InvalidPlanDay;
    return day;
}

pub fn run(init: std.process.Init, default_plan: []const u8, gospels: []const u8) !void {
    const allocator = init.arena.allocator();
    const args = try init.minimal.args.toSlice(allocator);
    var passage: ?[]const u8 = null;
    var plan_path: ?[]const u8 = null;
    var state_path: ?[]const u8 = null;
    var dump = false;
    var check_plan = false;
    var complete = false;
    var start_day: ?usize = null;
    var confirm = false;
    var i: usize = 1;
    while (i < args.len) : (i += 1) {
        const arg = args[i];
        if (std.mem.eql(u8, arg, "--help")) {
            try tui.writeAll(
                "Kata · parallel terminal reader (Zig prototype)\n" ++
                    "  kata                         today's configured reading\n" ++
                    "  kata --passage 'John:1:1-3'   ad-hoc New Testament passage\n" ++
                    "  --plan PATH                  custom JSON reading plan\n" ++
                    "  --state PATH                 isolated persistent state\n" ++
                    "  --dump                       plain-text output, no state writes\n" ++
                    "  --check-plan                 print complete cycle, no state writes\n" ++
                    "  --complete                   explicitly mark today complete and exit\n" ++
                    "  --start-day N                preview starting at plan day N (1-based)\n" ++
                    "  --start-day N --confirm      save preceding days as complete; N pending\n" ++
                    "Keys: j/k, Ctrl-d/u, g/G; h/l or Tab focus; s linked scroll;\n" ++
                    "      p choose plan; d choose day; 1/2/3 sources; c then y complete; q quit.\n" ++
                    "Verse-label alignment only: numbering variants need explicit mapping.\n",
            );
            return;
        } else if (std.mem.eql(u8, arg, "--passage") or std.mem.eql(u8, arg, "--plan") or std.mem.eql(u8, arg, "--state") or std.mem.eql(u8, arg, "--start-day")) {
            i += 1;
            if (i >= args.len) return error.MissingArgument;
            if (std.mem.eql(u8, arg, "--passage")) passage = args[i] else if (std.mem.eql(u8, arg, "--plan")) plan_path = args[i] else if (std.mem.eql(u8, arg, "--state")) state_path = args[i] else start_day = try parsePlanDay(args[i]);
        } else if (std.mem.eql(u8, arg, "--confirm")) {
            confirm = true;
        } else if (std.mem.eql(u8, arg, "--dump")) dump = true else if (std.mem.eql(u8, arg, "--check-plan")) check_plan = true else if (std.mem.eql(u8, arg, "--complete")) complete = true else return error.UnknownArgument;
    }
    if (complete and (passage != null or dump or check_plan)) return error.ConflictingArguments;
    if (confirm and start_day == null) return error.ConflictingArguments;
    if (start_day != null and (passage != null or complete or dump or check_plan)) return error.ConflictingArguments;
    const base = state_path orelse blk: {
        const home = init.environ_map.get("XDG_STATE_HOME") orelse try std.fmt.allocPrint(allocator, "{s}/.local/state", .{init.environ_map.get("HOME") orelse return error.HomeUnavailable});
        break :blk try std.fmt.allocPrint(allocator, "{s}/kata/state.json", .{home});
    };
    if (!dump and !check_plan and !complete and start_day == null) {
        try @import("app.zig").run(init, default_plan, gospels, base, plan_path, passage);
        return;
    }
    const remembered = if (plan_path == null) try catalog.loadSelected(allocator, init.io, base) else null;
    const plan_bytes = if (plan_path) |path| try std.Io.Dir.cwd().readFileAlloc(init.io, path, allocator, .limited(1024 * 1024)) else if (remembered) |id| blk: {
        if (std.mem.eql(u8, id, "builtin:optina")) break :blk default_plan;
        if (std.mem.eql(u8, id, "builtin:gospels")) break :blk gospels;
        break :blk try std.Io.Dir.cwd().readFileAlloc(init.io, id, allocator, .limited(1024 * 1024));
    } else default_plan;
    var plan = try scheduling.Plan.init(allocator, plan_bytes);
    defer plan.deinit();
    if (!std.unicode.utf8ValidateSlice(plan.name())) return error.InvalidPlanName;
    for (plan.name()) |ch| if (ch < 32 or ch == 127) return error.InvalidPlanName;
    if (check_plan) {
        try output(allocator, "{s}: {d} assignments\n", .{ plan.name(), plan.totalDays() });
        for (0..plan.totalDays()) |day| {
            const assignments = try plan.assignments(allocator, day);
            try output(allocator, "{d}: ", .{day + 1});
            for (assignments) |chapter| try output(allocator, "{s}:{d}  ", .{ chapter.book, chapter.chapter });
            try tui.writeAll("\n");
        }
        return;
    }
    const plan_hash = std.hash.Wyhash.hash(0, plan_bytes);
    const base_state = try storage.load(allocator, init.io, base);
    const path = if (remembered != null) try catalog.progressPath(allocator, base, base_state.plan_hash, plan_hash) else base;
    var state = try storage.load(allocator, init.io, path);
    const date = try today();
    if (passage == null) {
        if (state.plan_hash != 0 and state.plan_hash != plan_hash) return error.PlanChangedUseSeparateState;
        state.plan_hash = plan_hash;
    }
    if (start_day) |day| {
        var changed = state;
        // A nonrepeating plan has only one cycle, even after its final day.
        if (!plan.parsed.value.repeat) {
            changed.next_day = 0;
            changed.completed_on = 0;
        }
        try changed.startAtDay(day, plan.totalDays(), date);
        const assignments = try plan.assignments(allocator, changed.next_day);
        try output(allocator, "{s}: start day {d}/{d}, cycle {d}\nReading: ", .{ plan.name(), day, plan.totalDays(), @as(u128, changed.next_day / plan.totalDays()) + 1 });
        for (assignments) |chapter| try output(allocator, "{s}:{d}  ", .{ chapter.book, chapter.chapter });
        try output(allocator, "\n{d} preceding days in this cycle count as completed elsewhere.\nThe selected day remains pending and can be read and completed today.\nThis replaces progress within this cycle, resets reading positions, and clears today's completion marker.\nState: {s}\n", .{ day - 1, path });
        if (!confirm) {
            try tui.writeAll("Preview only: no state was changed. Repeat this command with --confirm to save.\n");
            return;
        }
        try storage.save(allocator, init.io, path, changed);
        try tui.writeAll("Saved. Launch Kata normally to read the selected assignment.\n");
        return;
    }
    if (complete) {
        _ = try plan.assignments(allocator, state.assignment(date));
        try state.complete(date);
        try storage.save(allocator, init.io, path, state);
        try output(allocator, "Completed today. Next assignment tomorrow. State: {s}\n", .{path});
        return;
    }
    var references: std.ArrayList(source.Reference) = .empty;
    var title: []const u8 = undefined;
    var scope_hash: u64 = undefined;
    if (passage) |raw| {
        const reference = try source.Reference.init(allocator, raw);
        try references.append(allocator, reference);
        title = reference.query;
        scope_hash = std.hash.Wyhash.hash(0, title);
    } else {
        const ordinal = state.assignment(date);
        const assignments = try plan.assignments(allocator, ordinal);
        for (assignments) |chapter| {
            const raw = try std.fmt.allocPrint(allocator, "{s}:{d}", .{ chapter.book, chapter.chapter });
            try references.append(allocator, try source.Reference.init(allocator, raw));
        }
        title = try std.fmt.allocPrint(allocator, "{s} · day {d}/{d}", .{ plan.name(), ordinal % plan.totalDays() + 1, plan.totalDays() });
        scope_hash = std.hash.Wyhash.hash(plan_hash, std.mem.asBytes(&ordinal));
    }
    if (state.scope_hash != scope_hash) {
        state.positions = .{ .{}, .{}, .{} };
        state.scope_hash = scope_hash;
    }
    var rows: std.ArrayList(layout.Row) = .empty;
    for (references.items) |reference| {
        var streams: [3][]const source.Verse = undefined;
        for (source.tools, 0..) |tool, pane| {
            streams[pane] = source.fetch(allocator, init.io, tool, reference) catch |err| {
                std.debug.print("Source {s}, passage {s}: {s}\n", .{ tool, reference.query, @errorName(err) });
                return err;
            };
        }
        try rows.appendSlice(allocator, try layout.alignVerses(allocator, streams));
    }
    if (rows.items.len == 0) return error.NoVerses;
    if (dump) {
        try output(allocator, "{s}\nNOTE: aligned by verse labels; numbering variants are not inferred.\n", .{title});
        for (rows.items) |row| {
            try output(allocator, "\n{s} {d}:{d}\n", .{ row.book, row.chapter, row.number });
            for (source.tools, 0..) |tool, pane| if (state.enabled[pane]) try output(allocator, "{s}: {s}\n", .{ tool, row.texts[pane] orelse "[not present under this verse label]" });
        }
        return;
    }
    unreachable; // Interactive sessions are handled by app.run above.
}
