const std = @import("std");
const catalog = @import("catalog.zig");
const storage = @import("state.zig");
const scheduling = @import("plan.zig");
const source = @import("source.zig");
const layout = @import("layout.zig");
const tui = @import("tui.zig");
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
        var streams: [3][]const source.Verse = undefined;
        for (source.tools, 0..) |tool, pane| streams[pane] = try source.fetch(allocator, io, tool, reference);
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
    const config_home = init.environ_map.get("XDG_CONFIG_HOME") orelse try std.fmt.allocPrint(allocator, "{s}/.config", .{init.environ_map.get("HOME") orelse return error.HomeUnavailable});
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
    var passage = initial_passage;
    var terminal: ?tui.Terminal = null;
    defer if (terminal) |*active_terminal| active_terminal.deinit();
    while (true) {
        const entry = plans.entries.items[selected];
        const path = try catalog.progressPath(allocator, base, base_hash, entry.hash);
        var state = try storage.load(allocator, init.io, path);
        if (state.plan_hash != 0 and state.plan_hash != entry.hash and passage == null) return error.PlanChangedUseSeparateState;
        const date = try currentDate();
        var session_arena = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer session_arena.deinit();
        const session = try prepare(session_arena.allocator(), init.io, entry, &state, date, passage);
        if (terminal == null) terminal = try tui.Terminal.init();
        const action = try tui.runInTerminal(session_arena.allocator(), session.rows, &state, .{
            .title = session.title,
            .plan_mode = passage == null,
            .today = date,
            .state_path = path,
            .io = init.io,
            .plans = plans.entries.items,
            .active_plan = selected,
            .plan = &plans.entries.items[selected].plan,
            .notice = notice,
        });
        try storage.save(allocator, init.io, path, state);
        if (std.mem.eql(u8, base, path)) base_hash = state.plan_hash;
        if (action == null) return;
        var target = selected;
        var changed = state;
        var target_path = path;
        switch (action.?) {
            .choose_plan => |index| {
                target = index;
                target_path = try catalog.progressPath(allocator, base, base_hash, plans.entries.items[target].hash);
                changed = storage.load(allocator, init.io, target_path) catch |err| {
                    notice = try std.fmt.allocPrint(allocator, "Cannot open plan progress: {s}. Previous plan retained.", .{@errorName(err)});
                    continue;
                };
                if (changed.plan_hash != 0 and changed.plan_hash != plans.entries.items[target].hash) {
                    notice = "Plan progress identity mismatch. Previous plan retained.";
                    continue;
                }
            },
            .start_day => |day| {
                if (!entry.plan.parsed.value.repeat) {
                    changed.next_day = 0;
                    changed.completed_on = 0;
                }
                changed.startAtDay(day, entry.plan.totalDays(), date) catch |err| {
                    notice = try std.fmt.allocPrint(allocator, "Cannot change reading day: {s}", .{@errorName(err)});
                    continue;
                };
            },
        }
        // Fetch and validate before committing a switch or confirmed day change.
        var validation = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer validation.deinit();
        _ = prepare(validation.allocator(), init.io, plans.entries.items[target], &changed, date, null) catch |err| {
            notice = try std.fmt.allocPrint(allocator, "Cannot load requested reading: {s}. Previous plan/day retained.", .{@errorName(err)});
            continue;
        };
        try storage.save(allocator, init.io, target_path, changed);
        try catalog.saveSelected(allocator, init.io, base, plans.entries.items[target].id);
        if (std.mem.eql(u8, base, target_path)) base_hash = changed.plan_hash;
        selected = target;
        passage = null;
        notice = switch (action.?) {
            .start_day => "Reading position saved. Preceding days count as complete; selected day is pending.",
            .choose_plan => "Plan selected. Its own progress restored. Verse-number variants are still unmapped.",
        };
    }
}
