const std = @import("std");
const layout = @import("layout.zig");
const storage = @import("state.zig");
const picking = @import("picker.zig");
const catalog = @import("catalog.zig");
const scheduling = @import("plan.zig");
const input = @import("input.zig");
var decoder: input.Decoder = .{};
const backend = if (@import("builtin").os.tag == .windows) @import("terminal_windows.zig") else @import("terminal_posix.zig");
pub const writeAll = backend.writeAll;

fn clipped(text: []const u8, width: usize) []const u8 {
    var end: usize = 0;
    while (end < text.len) {
        const length = std.unicode.utf8ByteSequenceLength(text[end]) catch break;
        if (end + length > text.len or layout.displayWidth(text[0 .. end + length]) > width) break;
        end += length;
    }
    return text[0..end];
}

fn cell(writer: *std.Io.Writer, text: []const u8, width: usize) !void {
    const visible = clipped(text, width);
    try writer.writeAll(visible);
    for (layout.displayWidth(visible)..width) |_| try writer.writeByte(' ');
}

fn selectedCount(enabled: [3]bool) usize {
    var count: usize = 0;
    for (enabled) |on| if (on) {
        count += 1;
    };
    return count;
}

fn moveFocus(state: *storage.State, direction: i32) void {
    var next = state.focus;
    for (0..3) |_| {
        next = if (direction > 0) (next + 1) % 3 else (next + 2) % 3;
        if (state.enabled[next]) {
            state.focus = next;
            return;
        }
    }
}

fn maxTop(lines: []const layout.Line, height: usize) usize {
    return lines.len -| height;
}

fn shift(tops: *[3]usize, state: storage.State, delta: i64, maximum: usize) void {
    for (0..3) |pane| {
        if (!state.linked and pane != state.focus) continue;
        tops[pane] = if (delta < 0) tops[pane] -| @as(usize, @intCast(-delta)) else @min(maximum, tops[pane] +| @as(usize, @intCast(delta)));
    }
}

fn restore(lines: []const layout.Line, position: storage.Position) usize {
    for (lines, 0..) |line, i| {
        if (line.row >= position.row) {
            var end = i;
            while (end < lines.len and lines[end].row == line.row) end += 1;
            return i + @min(position.line, end - i - 1);
        }
    }
    return 0;
}

fn remember(lines: []const layout.Line, tops: [3]usize, state: *storage.State) void {
    if (lines.len == 0) return;
    for (tops, 0..) |top, pane| {
        const at = @min(top, lines.len - 1);
        var first = at;
        while (first > 0 and lines[first - 1].row == lines[at].row) first -= 1;
        state.positions[pane] = .{ .row = lines[at].row, .line = at - first };
    }
}

pub const Options = struct {
    title: []const u8,
    plan_mode: bool,
    today: i32,
    state_path: []const u8,
    io: std.Io,
    plans: []const catalog.Entry = &.{},
    active_plan: usize = 0,
    plan: ?*const scheduling.Plan = null,
    notice: []const u8 = "Verse-label alignment; numbering variants are not yet mapped.",
};

fn completeDay(state: *storage.State, today: i32) !bool {
    state.complete(today) catch |err| switch (err) {
        error.AlreadyCompletedToday => return false,
        else => return err,
    };
    return true;
}

test "completion preserves real state overflow errors" {
    var state: storage.State = .{ .next_day = std.math.maxInt(usize) };
    try std.testing.expectError(error.ProgressOverflow, completeDay(&state, 20261007));
}

fn dayLabel(allocator: std.mem.Allocator, plan: scheduling.Plan, day: usize) ![]const u8 {
    var label = std.Io.Writer.Allocating.init(allocator);
    defer label.deinit();
    try label.writer.print("Day {d} · ", .{day + 1});
    const assignments = try plan.assignments(allocator, day);
    defer allocator.free(assignments);
    for (assignments) |chapter| try label.writer.print("{s}:{d}  ", .{ chapter.book, chapter.chapter });
    return allocator.dupe(u8, label.written());
}

fn renderPicker(allocator: std.mem.Allocator, w: *std.Io.Writer, picker: picking.Picker, options: Options, columns: usize, height: usize) !void {
    if (columns < 24 or height < 8) {
        try w.writeAll("Enlarge terminal. Esc/q cancels picker.");
        return;
    }
    const is_plans = picker.kind == .plans;
    try w.print(" K A T A  |  {s}\x1b[2;1H", .{if (is_plans) "Choose plan" else if (picker.kind == .confirm_day) "Confirm reading position" else "Choose plan day"});
    if (!is_plans) try w.writeAll(clipped(options.plan.?.name(), columns));
    if (picker.kind == .confirm_day) {
        const label = try dayLabel(allocator, options.plan.?.*, picker.selected);
        defer allocator.free(label);
        try w.writeAll("\x1b[4;1H");
        try w.writeAll(clipped(label, columns));
        try w.print("\x1b[6;1H{d} preceding days count as completed elsewhere.", .{picker.selected});
        try w.writeAll("\x1b[7;1HSelected day stays pending. Later progress in this cycle is replaced.");
    } else {
        const count = if (is_plans) options.plans.len else options.plan.?.totalDays();
        const capacity = height - 6;
        const first = @min(picker.selected -| (capacity / 2), count -| capacity);
        for (first..@min(count, first + capacity)) |index| {
            try w.print("\x1b[{d};1H{s}", .{ index - first + 3, if (index == picker.selected) "\x1b[38;2;210;178;116m▶ " else "\x1b[38;2;224;215;191m  " });
            const label = if (is_plans)
                try std.fmt.allocPrint(allocator, "{s} · {d} days{s}", .{ options.plans[index].plan.name(), options.plans[index].plan.totalDays(), if (index == options.active_plan) " · active" else "" })
            else
                try dayLabel(allocator, options.plan.?.*, index);
            defer allocator.free(label);
            try w.writeAll(clipped(label, columns - 2));
        }
    }
    try w.print("\x1b[{d};1H\x1b[38;2;210;178;116m", .{height - 1});
    const help = if (picker.kind == .confirm_day) "y confirms · any other key cancels confirmation · Esc/q returns" else if (is_plans) "j/k select · Enter open plan · Esc/q cancel · progress kept per plan" else "j/k select · type day number · Enter preview · Esc/q cancel";
    try w.writeAll(clipped(help, columns));
    try w.print("\x1b[{d};1H\x1b[38;2;134;145;156m", .{height});
    if (picker.digit_count > 0) try w.print("Go to day: {s}", .{picker.digits[0..picker.digit_count]}) else try w.writeAll(clipped(options.notice, columns));
}

pub const Terminal = struct {
    native: backend.Terminal,
    pub fn init() !Terminal {
        var native = try backend.Terminal.init();
        errdefer native.deinit();
        decoder = .{};
        errdefer writeAll("\x1b[?2004l\x1b[0m\x1b[?25h\x1b[?1049l") catch {};
        try writeAll("\x1b[?1049h\x1b[?25l\x1b[?2004h");
        return .{ .native = native };
    }
    pub fn deinit(self: *Terminal) void {
        writeAll("\x1b[?2004l\x1b[0m\x1b[?25h\x1b[?1049l") catch {};
        self.native.deinit();
    }
};

pub fn run(allocator: std.mem.Allocator, rows: []const layout.Row, state: *storage.State, options: Options) !?picking.Action {
    var terminal = try Terminal.init();
    defer terminal.deinit();
    return runInTerminal(allocator, rows, state, options);
}

pub const isInterrupted = backend.isInterrupted;

fn readKey() !?u8 {
    if (isInterrupted()) return null;
    const value = try backend.readByte();
    const key = switch (value) {
        .byte => |byte| decoder.feed(byte),
        .timeout => decoder.timeout(),
        .ignored => null,
    };
    if (key == 3) {
        backend.interrupt();
        return null;
    }
    return key;
}

fn handlePicker(picker: *picking.Picker, key: u8, count: usize, redraw: *bool) ?picking.Action {
    const kind = picker.kind;
    const selected = picker.selected;
    const digits = picker.digit_count;
    const invalid = picker.invalid_input;
    const action = picker.handle(key, count);
    if (kind != picker.kind or selected != picker.selected or digits != picker.digit_count or invalid != picker.invalid_input) redraw.* = true;
    return action;
}

/// Generic numbered menu, inside an already-owned application terminal.
pub fn choose(allocator: std.mem.Allocator, title: []const u8, description: []const u8, labels: []const []const u8, initial: usize, number_base: usize) !?usize {
    if (labels.len == 0) return error.EmptyMenu;
    var picker: picking.Picker = .{ .kind = .plans, .selected = @min(initial, labels.len - 1), .numeric = true, .number_base = number_base };
    var redraw = true;
    var old_columns: usize = 0;
    var old_height: usize = 0;
    while (!isInterrupted()) {
        const size = try backend.size();
        const columns = size.columns;
        const height = size.rows;
        if (columns != old_columns or height != old_height) redraw = true;
        if (redraw) {
            var frame = std.Io.Writer.Allocating.init(allocator);
            defer frame.deinit();
            const w = &frame.writer;
            try w.writeAll("\x1b[H\x1b[2J\x1b[48;2;21;25;34m\x1b[38;2;224;215;191m K A T A  |  ");
            try w.writeAll(clipped(title, columns -| 14));
            if (columns < 24 or height < 8) {
                try w.writeAll("\r\nEnlarge terminal. Esc/q returns.");
            } else {
                try w.writeAll("\x1b[2;1H\x1b[38;2;134;145;156m");
                try w.writeAll(clipped(description, columns));
                const capacity = height - 6;
                const first = @min(picker.selected -| (capacity / 2), labels.len -| capacity);
                for (first..@min(labels.len, first + capacity)) |index| {
                    try w.print("\x1b[{d};1H{s}{d}. ", .{ index - first + 3, if (index == picker.selected) "\x1b[38;2;210;178;116m▶ " else "\x1b[38;2;224;215;191m  ", index + number_base });
                    try w.writeAll(clipped(labels[index], columns -| 9));
                }
                try w.print("\x1b[{d};1H\x1b[38;2;210;178;116m", .{height - 1});
                try w.writeAll(clipped("j/k select · type number · Enter opens · Esc/q back (or quit at Library)", columns));
                try w.print("\x1b[{d};1H\x1b[38;2;134;145;156m", .{height});
                if (picker.digit_count > 0) try w.print("Selection: {s}", .{picker.digits[0..picker.digit_count]});
            }
            try writeAll(frame.written());
            old_columns = columns;
            old_height = height;
            redraw = false;
        }
        const key = try readKey() orelse continue;
        if (handlePicker(&picker, key, labels.len, &redraw)) |action| return action.choose_plan;
        if (picker.kind == .closed) return null;
    }
    return null;
}

/// The caller owns terminal mode for the entire application, including plan changes.
pub fn runInTerminal(allocator: std.mem.Allocator, rows: []const layout.Row, state: *storage.State, options: Options) !?picking.Action {
    var wrapping = std.heap.ArenaAllocator.init(allocator);
    defer wrapping.deinit();
    var lines: []const layout.Line = &.{};
    var tops: [3]usize = .{ 0, 0, 0 };
    var old_width: usize = 0;
    var old_height: usize = 0;
    var screen_columns: usize = 0;
    var screen_rows: usize = 0;
    var redraw = true;
    var confirm = false;
    var picker: picking.Picker = .{};
    var notice = options.notice;
    if (!state.enabled[state.focus]) moveFocus(state, 1);

    while (!isInterrupted()) {
        const size = try backend.size();
        const columns = size.columns;
        const height = size.rows;
        if (columns != screen_columns or height != screen_rows) {
            screen_columns = columns;
            screen_rows = height;
            redraw = true;
        }
        const count = selectedCount(state.enabled);
        const pane_width = columns / count -| 3;
        const body_height = height -| 6;
        const usable = pane_width >= 12 and body_height >= 3;
        if (usable and (pane_width != old_width or height != old_height)) {
            remember(lines, tops, state);
            _ = wrapping.reset(.retain_capacity);
            lines = try layout.renderLines(wrapping.allocator(), rows, pane_width, state.enabled);
            for (0..3) |pane| tops[pane] = @min(restore(lines, state.positions[pane]), maxTop(lines, body_height));
            if (state.linked) tops = .{ tops[state.focus], tops[state.focus], tops[state.focus] };
            old_width = pane_width;
            old_height = height;
            redraw = true;
        }
        if (redraw) {
            var frame = std.Io.Writer.Allocating.init(allocator);
            defer frame.deinit();
            const w = &frame.writer;
            try w.writeAll("\x1b[H\x1b[2J\x1b[48;2;21;25;34m\x1b[38;2;224;215;191m");
            if (picker.kind != .closed) {
                try renderPicker(allocator, w, picker, options, columns, height);
            } else if (!usable) {
                try w.writeAll("Kata: enlarge the terminal or press 1/2/3 to hide panes. q quits.");
            } else if (rows.len == 0) {
                try w.writeAll(" K A T A  |  Plan complete. m opens Library; p chooses a plan; d restarts at a day; q quits.");
            } else {
                try w.writeAll("\x1b[1;1H\x1b[1m\x1b[38;2;210;178;116m K A T A  \x1b[0m\x1b[48;2;21;25;34m\x1b[38;2;224;215;191m");
                const title = try std.fmt.allocPrint(allocator, "{s}  [{s}]{s}", .{ options.title, if (state.linked) "LINKED" else "INDEPENDENT", if (options.plan_mode) "" else " [FREE]" });
                defer allocator.free(title);
                try w.writeAll(clipped(title, columns -| 12));
                try w.writeAll("\x1b[2;1H\x1b[38;2;134;145;156m");
                try w.writeAll(clipped(notice, columns));
                try w.writeAll("\x1b[3;1H");
                for (0..3) |pane| {
                    if (!state.enabled[pane]) continue;
                    const row_index = if (lines.len > 0) lines[@min(tops[pane], lines.len - 1)].row else 0;
                    const row = rows[row_index];
                    const label = try std.fmt.allocPrint(allocator, "{s}{s} · {s} {d}", .{ if (state.focus == pane) "▶ " else "  ", @import("source.zig").tools[pane], row.book, row.chapter });
                    defer allocator.free(label);
                    try w.writeAll(if (state.focus == pane) "\x1b[38;2;210;178;116m" else "\x1b[38;2;134;145;156m");
                    try cell(w, label, pane_width);
                    try w.writeAll(" │ ");
                }
                for (0..body_height) |line_index| {
                    try w.print("\x1b[{d};1H", .{line_index + 4});
                    for (0..3) |pane| {
                        if (!state.enabled[pane]) continue;
                        try w.writeAll("\x1b[38;2;224;215;191m");
                        const at = tops[pane] + line_index;
                        try cell(w, if (at < lines.len) lines[at].cells[pane] else "", pane_width);
                        try w.writeAll("\x1b[38;2;74;87;101m │ ");
                    }
                }
                try w.print("\x1b[{d};1H\x1b[38;2;210;178;116m", .{height - 1});
                try w.writeAll(clipped(if (confirm) "Mark today's assignment complete? y confirms · any other key cancels" else "j/k scroll · Ctrl-d/u page · h/l focus · s sync · 1/2/3 panes · g/G ends", columns));
                try w.print("\x1b[{d};1H\x1b[38;2;134;145;156m", .{height});
                try w.writeAll(clipped(if (options.plan_mode) "m library · o free reading · p plans · d choose day · c complete day · q quit" else "m library · o choose place · [/] previous/next chapter · p plans · q quit", columns));
            }
            try writeAll(frame.written());
            redraw = false;
        }
        const key = try readKey() orelse continue;
        if (picker.kind != .closed) {
            const menu_count = if (picker.kind == .plans) options.plans.len else options.plan.?.totalDays();
            if (handlePicker(&picker, key, menu_count, &redraw)) |action| {
                remember(lines, tops, state);
                return action;
            }
            continue;
        }
        if (confirm) {
            confirm = false;
            if (key == 'y') {
                if (!try completeDay(state, options.today)) {
                    notice = "Already complete today. Your next assignment becomes available tomorrow.";
                    redraw = true;
                    continue;
                }
                remember(lines, tops, state);
                try storage.save(allocator, options.io, options.state_path, state.*);
                notice = "Completed and saved. Next assignment tomorrow; no catch-up workload.";
            }
            redraw = true;
            continue;
        }
        switch (key) {
            'q', 3 => break,
            'm', 'o', '[', ']' => {
                if ((key == '[' or key == ']') and options.plan_mode) continue;
                remember(lines, tops, state);
                return switch (key) {
                    'm' => .home,
                    'o' => .open_place,
                    '[' => .previous_chapter,
                    else => .next_chapter,
                };
            },
            'j' => shift(&tops, state.*, 1, maxTop(lines, body_height)),
            'k' => shift(&tops, state.*, -1, maxTop(lines, body_height)),
            4, 'f' => shift(&tops, state.*, @intCast(@max(1, body_height / 2)), maxTop(lines, body_height)),
            21, 'b' => shift(&tops, state.*, -@as(i64, @intCast(@max(1, body_height / 2))), maxTop(lines, body_height)),
            'g' => shift(&tops, state.*, -@as(i64, @intCast(lines.len)), maxTop(lines, body_height)),
            'G' => shift(&tops, state.*, @intCast(lines.len), maxTop(lines, body_height)),
            'h' => moveFocus(state, -1),
            'l', 9 => moveFocus(state, 1),
            's' => {
                state.linked = !state.linked;
                if (state.linked) tops = .{ tops[state.focus], tops[state.focus], tops[state.focus] };
            },
            '1', '2', '3' => {
                const pane = key - '1';
                if (state.enabled[pane] and count == 1) continue;
                remember(lines, tops, state);
                state.enabled[pane] = !state.enabled[pane];
                if (!state.enabled[state.focus]) moveFocus(state, 1);
                old_width = 0;
            },
            'c' => if (options.plan_mode) {
                if (rows.len == 0) notice = "Plan complete. Choose a plan or restart at a day." else confirm = true;
            },
            'p' => if (options.plans.len > 0) {
                picker = .{ .kind = .plans, .selected = options.active_plan };
            },
            'd' => if (options.plan_mode and options.plan != null) {
                picker = .{ .kind = .days, .selected = @min(options.plan.?.totalDays() - 1, state.assignment(options.today) % options.plan.?.totalDays()) };
            },
            else => continue,
        }
        redraw = true;
    }
    remember(lines, tops, state);
    return null;
}

test {
    std.testing.refAllDecls(@import("console_geometry.zig"));
    std.testing.refAllDecls(@import("console_queue.zig"));
}
