const std = @import("std");
const layout = @import("layout.zig");
const storage = @import("state.zig");
const picking = @import("picker.zig");
const catalog = @import("catalog.zig");
const scheduling = @import("plan.zig");
const c = @cImport({
    @cInclude("unistd.h");
    @cInclude("termios.h");
    @cInclude("sys/ioctl.h");
    @cInclude("poll.h");
    @cInclude("signal.h");
    @cInclude("locale.h");
});

// Call the libc symbol directly: Zig 0.16 cannot translate this host's
// optimized glibc fortify inline wrapper for poll. The caller supplies one
// real pollfd record and the exact count, so no inferred object size is needed.
extern "c" fn poll(fds: [*]c.struct_pollfd, count: c.nfds_t, timeout: c_int) c_int;

var interrupted = std.atomic.Value(bool).init(false);
fn stop(_: c_int) callconv(.c) void {
    interrupted.store(true, .monotonic);
}

pub fn writeAll(bytes: []const u8) !void {
    var offset: usize = 0;
    while (offset < bytes.len) {
        const count = c.write(1, bytes.ptr + offset, bytes.len - offset);
        if (count <= 0) return error.TerminalWrite;
        offset += @intCast(count);
    }
}

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
    if (picker.invalid_input) try w.writeAll("Invalid day. Use a number within this plan; Backspace edits.") else if (picker.digit_count > 0) try w.print("Go to day: {s}", .{picker.digits[0..picker.digit_count]}) else try w.writeAll(clipped(options.notice, columns));
}

pub const Terminal = struct {
    original: c.struct_termios,
    old_int: @TypeOf(c.signal(c.SIGINT, stop)),
    old_term: @TypeOf(c.signal(c.SIGTERM, stop)),
    old_hup: @TypeOf(c.signal(c.SIGHUP, stop)),

    pub fn init() !Terminal {
        if (c.isatty(0) != 1 or c.isatty(1) != 1) return error.InteractiveTerminalRequired;
        _ = c.setlocale(c.LC_CTYPE, "");
        var original: c.struct_termios = undefined;
        if (c.tcgetattr(0, &original) != 0) return error.TerminalSetup;
        var raw = original;
        c.cfmakeraw(&raw);
        if (c.tcsetattr(0, c.TCSAFLUSH, &raw) != 0) return error.TerminalSetup;
        errdefer _ = c.tcsetattr(0, c.TCSAFLUSH, &original);
        interrupted.store(false, .monotonic);
        const terminal: Terminal = .{
            .original = original,
            .old_int = c.signal(c.SIGINT, stop),
            .old_term = c.signal(c.SIGTERM, stop),
            .old_hup = c.signal(c.SIGHUP, stop),
        };
        errdefer {
            _ = c.signal(c.SIGINT, terminal.old_int);
            _ = c.signal(c.SIGTERM, terminal.old_term);
            _ = c.signal(c.SIGHUP, terminal.old_hup);
            writeAll("\x1b[0m\x1b[?25h\x1b[?1049l") catch {};
        }
        try writeAll("\x1b[?1049h\x1b[?25l");
        return terminal;
    }
    pub fn deinit(self: *Terminal) void {
        writeAll("\x1b[0m\x1b[?25h\x1b[?1049l") catch {};
        _ = c.tcsetattr(0, c.TCSAFLUSH, &self.original);
        _ = c.signal(c.SIGINT, self.old_int);
        _ = c.signal(c.SIGTERM, self.old_term);
        _ = c.signal(c.SIGHUP, self.old_hup);
    }
};

pub fn run(allocator: std.mem.Allocator, rows: []const layout.Row, state: *storage.State, options: Options) !?picking.Action {
    var terminal = try Terminal.init();
    defer terminal.deinit();
    return runInTerminal(allocator, rows, state, options);
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

    while (!interrupted.load(.monotonic)) {
        var size: c.struct_winsize = std.mem.zeroes(c.struct_winsize);
        _ = c.ioctl(1, c.TIOCGWINSZ, &size);
        const columns: usize = if (size.ws_col > 0) size.ws_col else 100;
        const height: usize = if (size.ws_row > 0) size.ws_row else 30;
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
                try w.writeAll(" K A T A  |  Plan complete. p chooses a plan; d restarts at a day; q quits.");
            } else {
                try w.writeAll("\x1b[1;1H\x1b[1m\x1b[38;2;210;178;116m K A T A  \x1b[0m\x1b[48;2;21;25;34m\x1b[38;2;224;215;191m");
                const title = try std.fmt.allocPrint(allocator, "{s}  [{s}]", .{ options.title, if (state.linked) "LINKED" else "INDEPENDENT" });
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
                try w.writeAll(clipped(if (options.plan_mode) "p plans · d choose day · c complete day · q save and quit" else "p plans · q save and quit  |  Ad-hoc passage: daily plan progress is unchanged.", columns));
            }
            try writeAll(frame.written());
            redraw = false;
        }
        var event = c.struct_pollfd{ .fd = 0, .events = c.POLLIN, .revents = 0 };
        const polled = poll(@ptrCast(&event), 1, 150);
        if (polled < 0) continue;
        if (polled == 0) continue;
        if ((event.revents & (c.POLLHUP | c.POLLERR)) != 0) break;
        var key: u8 = 0;
        if (c.read(0, &key, 1) != 1) break;
        if (key == 3) break;
        if (picker.kind != .closed) {
            const menu_count = if (picker.kind == .plans) options.plans.len else options.plan.?.totalDays();
            if (picker.handle(key, menu_count)) |action| {
                remember(lines, tops, state);
                return action;
            }
            redraw = true;
            continue;
        }
        if (confirm) {
            confirm = false;
            if (key == 'y') {
                state.complete(options.today) catch {
                    notice = "Already complete today. Your next assignment becomes available tomorrow.";
                    redraw = true;
                    continue;
                };
                remember(lines, tops, state);
                try storage.save(allocator, options.io, options.state_path, state.*);
                notice = "Completed and saved. Next assignment tomorrow; no catch-up workload.";
            }
            redraw = true;
            continue;
        }
        switch (key) {
            'q', 3 => break,
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
