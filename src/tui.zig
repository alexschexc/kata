const std = @import("std");
const layout = @import("layout.zig");
const storage = @import("state.zig");
const picking = @import("picker.zig");
const catalog = @import("catalog.zig");
const scheduling = @import("plan.zig");
const input = @import("input.zig");
const search = @import("search.zig");
const theme = @import("theme.zig");
var decoder: input.Decoder = .{};
const backend = if (@import("builtin").os.tag == .windows) @import("terminal_windows.zig") else @import("terminal_posix.zig");

/// True when the terminal cannot show 24-bit colour (macOS Terminal.app), so
/// theme escapes are rewritten to the 256-colour palette on output.
var colors_256 = false;

/// Picks the colour depth. KATA_COLORS=256|truecolor overrides detection;
/// otherwise Apple Terminal without COLORTERM=truecolor/24bit gets 256 colours.
fn detectColorDepth() void {
    if (@import("builtin").os.tag == .windows) return;
    const c = @cImport(@cInclude("stdlib.h"));
    const env = struct {
        fn get(name: [*:0]const u8) ?[]const u8 {
            const p = c.getenv(name) orelse return null;
            return std.mem.span(p);
        }
    };
    if (env.get("KATA_COLORS")) |v| {
        if (std.mem.eql(u8, v, "256")) colors_256 = true;
        if (std.mem.eql(u8, v, "truecolor")) colors_256 = false;
        if (std.mem.eql(u8, v, "256") or std.mem.eql(u8, v, "truecolor")) return;
    }
    if (env.get("COLORTERM")) |v| {
        if (std.mem.eql(u8, v, "truecolor") or std.mem.eql(u8, v, "24bit")) return;
    }
    if (env.get("TERM_PROGRAM")) |v| colors_256 = std.mem.eql(u8, v, "Apple_Terminal");
}

const ChunkSink = struct {
    buf: [16384]u8 = undefined,
    len: usize = 0,
    pub fn write(self: *ChunkSink, bytes: []const u8) !void {
        var rest = bytes;
        while (rest.len > 0) {
            if (self.len == self.buf.len) try self.flush();
            const n = @min(rest.len, self.buf.len - self.len);
            @memcpy(self.buf[self.len..][0..n], rest[0..n]);
            self.len += n;
            rest = rest[n..];
        }
    }
    fn flush(self: *ChunkSink) !void {
        if (self.len == 0) return;
        try backend.writeAll(self.buf[0..self.len]);
        self.len = 0;
    }
};

pub fn writeAll(bytes: []const u8) !void {
    if (!colors_256) return backend.writeAll(bytes);
    var sink: ChunkSink = .{};
    try theme.downsample(bytes, &sink);
    try sink.flush();
}

pub fn clipped(text: []const u8, width: usize) []const u8 {
    var end: usize = 0;
    while (end < text.len) {
        const length = std.unicode.utf8ByteSequenceLength(text[end]) catch break;
        if (end + length > text.len or layout.displayWidth(text[0 .. end + length]) > width) break;
        end += length;
    }
    return text[0..end];
}

pub fn cell(writer: *std.Io.Writer, text: []const u8, width: usize) !void {
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
    /// `c` may mark the plan day complete (always true in plan_mode; set for plan days opened from a hub).
    completable: bool = false,
    today: i32,
    state_path: []const u8,
    io: std.Io,
    plans: []const catalog.Entry = &.{},
    active_plan: usize = 0,
    plan: ?*const scheduling.Plan = null,
    notice: []const u8 = "Verse-label alignment; numbering variants are not yet mapped.",
    /// Owned by the application so results survive opening another chapter.
    search: ?*search.State = null,
    /// Editions available per column (1 = bundled only) and display names.
    editions: [3]u8 = .{ 1, 1, 1 },
    names: [3][]const u8 = .{ "kjv", "grb", "vul" },
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
            try w.print("\x1b[{d};1H{s}", .{ index - first + 3, if (index == picker.selected) theme.accent ++ "▶ " else theme.ink ++ "  " });
            const label = if (is_plans)
                try std.fmt.allocPrint(allocator, "{s} · {d} days{s}", .{ options.plans[index].plan.name(), options.plans[index].plan.totalDays(), if (index == options.active_plan) " · active" else "" })
            else
                try dayLabel(allocator, options.plan.?.*, index);
            defer allocator.free(label);
            try w.writeAll(clipped(label, columns - 2));
        }
    }
    try w.print("\x1b[{d};1H" ++ theme.accent, .{height - 1});
    const help = if (picker.kind == .confirm_day) "y confirms · any other key cancels confirmation · Esc/q returns" else if (is_plans) "j/k select · Enter open plan · Esc/q cancel · progress kept per plan" else "j/k select · type day number · Enter preview · Esc/q cancel";
    try w.writeAll(clipped(help, columns));
    try w.print("\x1b[{d};1H" ++ theme.dim, .{height});
    if (picker.digit_count > 0) try w.print("Go to day: {s}", .{picker.digits[0..picker.digit_count]}) else try w.writeAll(clipped(options.notice, columns));
}

pub const Terminal = struct {
    native: backend.Terminal,
    pub fn init() !Terminal {
        var native = try backend.Terminal.init();
        errdefer native.deinit();
        decoder = .{};
        detectColorDepth();
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
/// Input already waiting: skip redrawing until the burst is applied.
pub const inputPending = backend.pending;
pub const Size = @import("terminal_types.zig").Size;

/// Queries the terminal's image support. Call after Terminal.init (raw mode),
/// before any key reading. Bounded by `timeout_ms`; never blocks longer.
pub fn detectGraphics(timeout_ms: u32) @import("graphics.zig").Capabilities {
    const graphics = @import("graphics.zig");
    writeAll(graphics.query) catch return .{};
    var reply: [512]u8 = undefined;
    var len: usize = 0;
    var waited: u32 = 0;
    while (waited < timeout_ms and len < reply.len) {
        const value = backend.readByte() catch return .{};
        switch (value) {
            .byte => |byte| {
                reply[len] = byte;
                len += 1;
                if (byte == 'c') if (graphics.parse(reply[0..len])) |caps| {
                    const size = backend.size() catch return caps;
                    return caps.withCellsFrom(size.columns, size.rows).withWindowPixels(size.columns, size.rows, size.pixel_width, size.pixel_height);
                };
            },
            .timeout => waited += 80,
            .ignored => {},
        }
    }
    return .{};
}

pub fn screenSize() !Size {
    return backend.size();
}

/// Text entry: printable and UTF-8 bytes (including pasted text) reach the caller.
pub fn setTextInput(on: bool) void {
    decoder.text = on;
}

/// One-line status screen shown while slow work (EPUB conversion) runs.
pub fn status(text: []const u8) !void {
    const size = backend.size() catch Size{ .columns = 80, .rows = 24 };
    var buffer: [512]u8 = undefined;
    var w: std.Io.Writer = .fixed(&buffer);
    w.writeAll("\x1b[H\x1b[2J" ++ theme.paper ++ theme.ink ++ " K A T A  |  ") catch {};
    w.writeAll(clipped(text, size.columns -| 14)) catch {};
    try writeAll(w.buffered());
}

/// Single-line text prompt inside the application terminal. Returns the
/// entered text (owned by `allocator`), or null when cancelled with Esc.
pub fn prompt(allocator: std.mem.Allocator, title: []const u8, description: []const u8, initial: []const u8) !?[]const u8 {
    return promptWith(allocator, title, description, initial, false);
}

/// Tab in a path prompt: returned text starts with this byte, followed by
/// the text typed so far (the caller opens a folder browser there).
pub const browse_marker = '\t';

/// As `prompt`; with `browsable`, Tab returns `browse_marker` ++ text.
pub fn promptWith(allocator: std.mem.Allocator, title: []const u8, description: []const u8, initial: []const u8, browsable: bool) !?[]const u8 {
    var buffer: [1024]u8 = undefined;
    var len: usize = @min(initial.len, buffer.len);
    @memcpy(buffer[0..len], initial[0..len]);
    decoder.text = true;
    defer decoder.text = false;
    var redraw = true;
    var old: Size = .{ .columns = 0, .rows = 0 };
    var invalid = false;
    while (!isInterrupted()) {
        const size = try backend.size();
        if (size.columns != old.columns or size.rows != old.rows) redraw = true;
        if (redraw) {
            var frame = std.Io.Writer.Allocating.init(allocator);
            defer frame.deinit();
            const w = &frame.writer;
            try w.writeAll("\x1b[H\x1b[2J" ++ theme.paper ++ theme.ink ++ " K A T A  |  ");
            try w.writeAll(clipped(title, size.columns -| 14));
            if (size.columns < 24 or size.rows < 10) {
                try w.writeAll("\r\nEnlarge terminal. Esc cancels.");
            } else {
                var wrapping = std.heap.ArenaAllocator.init(allocator);
                defer wrapping.deinit();
                const lines = layout.wrap(wrapping.allocator(), description, size.columns -| 2) catch &[_][]const u8{description};
                for (lines[0..@min(lines.len, size.rows -| 7)], 0..) |line, i| {
                    try w.print("\x1b[{d};1H" ++ theme.dim, .{i + 3});
                    try w.writeAll(clipped(line, size.columns));
                }
                const text = buffer[0..len];
                const shown = if (layout.displayWidth(text) + 4 > size.columns) blk: {
                    // Show the end of a long entry.
                    var start: usize = 0;
                    while (start < text.len and layout.displayWidth(text[start..]) + 4 > size.columns) start += 1;
                    while (start < text.len and (text[start] & 0xC0) == 0x80) start += 1;
                    break :blk text[start..];
                } else text;
                try w.print("\x1b[{d};1H" ++ theme.accent ++ "> " ++ theme.ink, .{@min(lines.len, size.rows -| 7) + 4});
                try w.writeAll(shown);
                try w.writeAll("█");
                try w.print("\x1b[{d};1H" ++ theme.accent, .{size.rows - 1});
                try w.writeAll(clipped(if (browsable) "Type or paste · Tab browses folders · Enter accepts · Backspace deletes · Ctrl-u clears · Esc cancels" else "Type or paste · Enter accepts · Backspace deletes · Ctrl-u clears · Esc cancels", size.columns));
                if (invalid) {
                    try w.print("\x1b[{d};1H" ++ theme.dim, .{size.rows});
                    try w.writeAll(clipped("Enter some text first (it must be valid UTF-8).", size.columns));
                }
            }
            try writeAll(frame.written());
            old = size;
            redraw = false;
        }
        const key = try readKey() orelse continue;
        redraw = true;
        invalid = false;
        switch (key) {
            27 => return null,
            10, 13 => {
                const text = std.mem.trim(u8, buffer[0..len], " ");
                if (text.len == 0 or !std.unicode.utf8ValidateSlice(text)) {
                    invalid = true;
                    continue;
                }
                return try allocator.dupe(u8, text);
            },
            127, 8 => {
                if (len > 0) len -= 1;
                while (len > 0 and (buffer[len] & 0xC0) == 0x80) len -= 1;
            },
            21 => len = 0,
            9 => if (browsable) {
                const out = try allocator.alloc(u8, len + 1);
                out[0] = browse_marker;
                @memcpy(out[1..], buffer[0..len]);
                return out;
            },
            else => if (key >= 0x20 and key != 0x7f and len < buffer.len) {
                buffer[len] = key;
                len += 1;
            },
        }
    }
    return null;
}

pub fn readKey() !?u8 {
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

pub const Choice = union(enum) { pick: usize, remove: usize };

pub const ChooseOptions = struct {
    /// Text each row is matched against by the `/` filter (default: the label).
    haystack: ?[]const []const u8 = null,
    /// The first N rows can be removed with `x` (after a y confirmation).
    removable_rows: usize = 0,
};

/// True when every space-separated word of `filter` starts a word in `haystack`
/// (case- and accent-insensitive, same matcher as in-book search).
pub fn matchesFilter(haystack: []const u8, filter: []const u8) bool {
    var tokens = std.mem.tokenizeScalar(u8, filter, ' ');
    while (tokens.next()) |token| {
        const query = search.Query.init(token);
        if (query.empty()) continue;
        if (search.next(haystack, &query, 0) == null) return false;
    }
    return true;
}

fn refilter(allocator: std.mem.Allocator, visible: *std.ArrayList(usize), labels: []const []const u8, haystack: ?[]const []const u8, filter: []const u8) !void {
    visible.clearRetainingCapacity();
    for (labels, 0..) |label, index| {
        const text = if (haystack) |h| h[index] else label;
        if (filter.len == 0 or matchesFilter(text, filter)) try visible.append(allocator, index);
    }
}

/// Generic numbered menu, inside an already-owned application terminal.
pub fn choose(allocator: std.mem.Allocator, title: []const u8, description: []const u8, labels: []const []const u8, initial: usize, number_base: usize) !?usize {
    const picked = try chooseOpts(allocator, title, description, labels, initial, number_base, .{}) orelse return null;
    return picked.pick;
}

/// As `choose`, with a vim-style `/` filter and optional `x` removal.
/// Numbers shown and typed refer to positions in the (filtered) list; the
/// returned index is always into `labels`.
pub fn chooseOpts(allocator: std.mem.Allocator, title: []const u8, description: []const u8, labels: []const []const u8, initial: usize, number_base: usize, options: ChooseOptions) !?Choice {
    if (labels.len == 0) return error.EmptyMenu;
    var picker: picking.Picker = .{ .kind = .plans, .selected = @min(initial, labels.len - 1), .numeric = true, .number_base = number_base };
    var filter_buf: [64]u8 = undefined;
    var filter_len: usize = 0;
    var filtering = false;
    var confirming = false;
    var visible: std.ArrayList(usize) = .empty;
    defer visible.deinit(allocator);
    try refilter(allocator, &visible, labels, options.haystack, "");
    defer decoder.text = false;
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
            try w.writeAll("\x1b[H\x1b[2J" ++ theme.paper ++ theme.ink ++ " K A T A  |  ");
            try w.writeAll(clipped(title, columns -| 14));
            if (columns < 24 or height < 8) {
                try w.writeAll("\r\nEnlarge terminal. Esc/q returns.");
            } else {
                try w.writeAll("\x1b[2;1H" ++ theme.dim);
                try w.writeAll(clipped(description, columns));
                const capacity = height - 6;
                const count = visible.items.len;
                const first = @min(picker.selected -| (capacity / 2), count -| capacity);
                for (first..@min(count, first + capacity)) |position| {
                    try w.print("\x1b[{d};1H{s}{d}. ", .{ position - first + 3, if (position == picker.selected) theme.accent ++ "▶ " else theme.ink ++ "  ", position + number_base });
                    try w.writeAll(clipped(labels[visible.items[position]], columns -| 9));
                }
                if (count == 0) try w.writeAll("\x1b[3;1H" ++ theme.dim ++ "  No matches.");
                try w.print("\x1b[{d};1H" ++ theme.accent, .{height - 1});
                if (confirming) {
                    var ask: [160]u8 = undefined;
                    const text = std.fmt.bufPrint(&ask, "Remove \"{s}\"? y confirms · any other key cancels", .{labels[visible.items[picker.selected]]}) catch "Remove this entry? y confirms";
                    try w.writeAll(clipped(text, columns));
                } else if (filtering) {
                    try w.writeAll(clipped("Type to filter · Enter keeps the filter · Esc clears · Backspace deletes", columns));
                } else {
                    try w.writeAll(clipped(if (options.removable_rows > 0) "j/k select · number · Enter opens · / filter · x remove · Esc/q back" else "j/k select · type number · Enter opens · / filter · Esc/q back (or quit at Library)", columns));
                }
                try w.print("\x1b[{d};1H" ++ theme.dim, .{height});
                if (filtering) {
                    try w.print("/{s}█", .{clipped(filter_buf[0..filter_len], columns -| 3)});
                } else if (filter_len > 0) {
                    try w.print("Filter: {s} · {d} of {d} shown · Esc clears", .{ clipped(filter_buf[0..filter_len], columns -| 40), count, labels.len });
                } else if (picker.digit_count > 0) try w.print("Selection: {s}", .{picker.digits[0..picker.digit_count]});
            }
            try writeAll(frame.written());
            old_columns = columns;
            old_height = height;
            redraw = false;
        }
        const key = try readKey() orelse continue;
        redraw = true;
        if (confirming) {
            confirming = false;
            if (key == 'y') return .{ .remove = visible.items[picker.selected] };
            continue;
        }
        if (filtering) {
            switch (key) {
                10, 13 => {
                    filtering = false;
                    decoder.text = false;
                },
                27 => {
                    filter_len = 0;
                    filtering = false;
                    decoder.text = false;
                    try refilter(allocator, &visible, labels, options.haystack, "");
                    picker.selected = 0;
                },
                127, 8 => {
                    if (filter_len == 0) {
                        filtering = false;
                        decoder.text = false;
                    } else {
                        filter_len -= 1;
                        while (filter_len > 0 and (filter_buf[filter_len] & 0xC0) == 0x80) filter_len -= 1;
                        try refilter(allocator, &visible, labels, options.haystack, filter_buf[0..filter_len]);
                        picker.selected = 0;
                    }
                },
                21 => {
                    filter_len = 0;
                    try refilter(allocator, &visible, labels, options.haystack, "");
                    picker.selected = 0;
                },
                else => if (key >= 0x20 and key != 0x7f and filter_len < filter_buf.len) {
                    filter_buf[filter_len] = key;
                    filter_len += 1;
                    try refilter(allocator, &visible, labels, options.haystack, filter_buf[0..filter_len]);
                    picker.selected = 0;
                },
            }
            continue;
        }
        if (key == '/') {
            filtering = true;
            decoder.text = true;
            picker.digit_count = 0;
            continue;
        }
        if ((key == 27 or key == 'q') and filter_len > 0) {
            // Clear the filter first, keeping the highlighted row highlighted.
            const kept = if (picker.selected < visible.items.len) visible.items[picker.selected] else 0;
            filter_len = 0;
            try refilter(allocator, &visible, labels, options.haystack, "");
            picker.selected = @min(kept, visible.items.len -| 1);
            continue;
        }
        if (key == 'x' and options.removable_rows > 0) {
            if (picker.selected < visible.items.len and visible.items[picker.selected] < options.removable_rows) confirming = true;
            continue;
        }
        if (handlePicker(&picker, key, visible.items.len, &redraw)) |action| return .{ .pick = visible.items[action.choose_plan] };
        if (picker.kind == .closed) return null;
    }
    return null;
}

test "menu filter matches words by prefix, any order, accent-insensitively" {
    try std.testing.expect(matchesFilter("Building Microservices · Sam Newman", "micro"));
    try std.testing.expect(matchesFilter("Building Microservices · Sam Newman", "newman build"));
    try std.testing.expect(!matchesFilter("Building Microservices · Sam Newman", "services"));
    try std.testing.expect(matchesFilter("Élan Vital · Bergson", "elan"));
    try std.testing.expect(matchesFilter("anything", ""));
    try std.testing.expect(!matchesFilter("", "x"));
}

const mark_on = theme.accent_bg ++ theme.on_accent;
const mark_off = theme.paper ++ theme.ink;

/// Writes full[from..to], highlighting query matches found from `search_from`.
fn writeMarked(w: *std.Io.Writer, full: []const u8, from: usize, to: usize, query: *const search.Query, search_from: usize) !void {
    return writeMarkedStyled(w, full, from, to, query, search_from, mark_off);
}

/// As `writeMarked`, restoring `restore` (colour escape) after each match.
pub fn writeMarkedStyled(w: *std.Io.Writer, full: []const u8, from: usize, to: usize, query: *const search.Query, search_from: usize, after_match: []const u8) !void {
    var at = from;
    var cursor = search_from;
    while (search.next(full, query, cursor)) |range| {
        if (range.start >= to) break;
        cursor = range.end;
        if (range.end <= at) continue;
        const first = @max(range.start, at);
        const last = @min(range.end, to);
        try w.writeAll(full[at..first]);
        try w.writeAll(mark_on);
        try w.writeAll(full[first..last]);
        try w.writeAll(after_match);
        at = last;
    }
    try w.writeAll(full[at..to]);
}

/// A reader cell with search matches highlighted. Cells are slices of the
/// line's full "chapter:verse text", so matches split by wrapping still mark.
fn markedCell(w: *std.Io.Writer, line: layout.Line, pane: usize, query: ?*const search.Query, width: usize) !void {
    const text = line.cells[pane];
    const q = query orelse return cell(w, text, width);
    const full = line.texts[pane];
    const visible = clipped(text, width);
    const origin = @intFromPtr(full.ptr);
    const begin = @intFromPtr(visible.ptr);
    if (visible.len == 0 or begin < origin or begin + visible.len > origin + full.len or std.mem.endsWith(u8, full, layout.absent)) return cell(w, text, width);
    const from = begin - origin;
    const body = @min(full.len, (std.mem.indexOfScalar(u8, full, ' ') orelse full.len) + 1);
    try writeMarked(w, full, from, from + visible.len, q, body);
    for (layout.displayWidth(visible)..width) |_| try w.writeByte(' ');
}

fn locate(rows: []const layout.Row, hit: search.Hit) ?usize {
    for (rows, 0..) |row, index| {
        if (row.chapter == hit.chapter and row.number == hit.number and std.mem.eql(u8, row.book, hit.book())) return index;
    }
    return null;
}

fn renderPanel(allocator: std.mem.Allocator, w: *std.Io.Writer, finder: *const search.State, x: usize, width: usize, top: usize, bottom: usize) !void {
    const inner = width -| 3;
    const tool = finder.names[finder.pane];
    var row = top;
    const header = try std.fmt.allocPrint(allocator, "Search {s} · “{s}”", .{ tool, finder.submittedText() });
    defer allocator.free(header);
    try w.print("\x1b[{d};{d}H" ++ theme.rule ++ "┃ {s}", .{ row, x, if (finder.focus) "\x1b[1m" ++ theme.accent else theme.dim });
    try cell(w, header, inner);
    try w.writeAll("\x1b[0m" ++ theme.paper);
    row += 1;
    const summary = try std.fmt.allocPrint(allocator, "{d} verses · {d} matches · {d}/{d}", .{ finder.hits.len, finder.occurrences, if (finder.hits.len == 0) 0 else finder.selected + 1, finder.hits.len });
    defer allocator.free(summary);
    try w.print("\x1b[{d};{d}H" ++ theme.rule ++ "┃ " ++ theme.dim, .{ row, x });
    try cell(w, summary, inner);
    row += 1;
    const capacity = (bottom + 1) -| row;
    if (capacity == 0) return;
    const first = @min(finder.selected -| (capacity / 2), finder.hits.len -| capacity);
    for (0..capacity) |offset| {
        try w.print("\x1b[{d};{d}H" ++ theme.rule ++ "┃ ", .{ row + offset, x });
        const index = first + offset;
        if (index >= finder.hits.len) {
            try cell(w, "", inner);
            continue;
        }
        const hit = finder.hits[index];
        const chosen = index == finder.selected;
        const own = hit.label.chapter != hit.chapter or hit.label.number != hit.number;
        const place = if (own)
            try std.fmt.allocPrint(allocator, "{s}{s} {d}:{d} ({s} {d}:{d}) ", .{ if (chosen) "▶ " else "  ", hit.book(), hit.chapter, hit.number, tool, hit.label.chapter, hit.label.number })
        else
            try std.fmt.allocPrint(allocator, "{s}{s} {d}:{d} ", .{ if (chosen) "▶ " else "  ", hit.book(), hit.chapter, hit.number });
        defer allocator.free(place);
        try w.writeAll(if (chosen) theme.accent else theme.dim);
        const place_visible = clipped(place, inner);
        try w.writeAll(place_visible);
        var used = layout.displayWidth(place_visible);
        try w.writeAll(theme.ink);
        // Show a little context before the first match, from a word start.
        var from: usize = 0;
        if (hit.match.start > 24) {
            from = hit.match.start - 24;
            while (from < hit.match.start and (hit.text[from] & 0xC0) == 0x80) from += 1;
            if (std.mem.indexOfScalarPos(u8, hit.text[0..hit.match.start], from, ' ')) |space| from = space + 1;
            if (used + 1 < inner) {
                try w.writeAll("…");
                used += 1;
            }
        }
        const visible = clipped(hit.text[from..], inner -| used);
        try writeMarked(w, hit.text, from, from + visible.len, &finder.active, 0);
        used += layout.displayWidth(visible);
        for (used..inner) |_| try w.writeByte(' ');
    }
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
    const finder = options.search;
    var pending_row: ?usize = null;
    if (finder) |f| if (f.reveal) {
        // A search result opened this reading: show it in the searched source.
        f.reveal = false;
        state.enabled[f.pane] = true;
        state.focus = f.pane;
    };
    if (!state.enabled[state.focus]) moveFocus(state, 1);
    defer decoder.text = false;

    while (!isInterrupted()) {
        const size = try backend.size();
        const columns = size.columns;
        const height = size.rows;
        if (columns != screen_columns or height != screen_rows) {
            screen_columns = columns;
            screen_rows = height;
            redraw = true;
        }
        const panel_open = if (finder) |f| f.open else false;
        const panel_width: usize = if (!panel_open) 0 else if (columns >= 72) @min(64, @max(30, columns * 2 / 5)) else columns;
        const reader_visible = panel_width < columns;
        const count = selectedCount(state.enabled);
        const pane_width = (columns - panel_width) / count -| 3;
        const body_height = height -| 6;
        const usable = body_height >= 3 and (!reader_visible or pane_width >= 12);
        if (usable and reader_visible and (pane_width != old_width or height != old_height)) {
            remember(lines, tops, state);
            _ = wrapping.reset(.retain_capacity);
            lines = try layout.renderLines(wrapping.allocator(), rows, pane_width, state.enabled);
            for (0..3) |pane| tops[pane] = @min(restore(lines, state.positions[pane]), maxTop(lines, body_height));
            if (state.linked) tops = .{ tops[state.focus], tops[state.focus], tops[state.focus] };
            old_width = pane_width;
            old_height = height;
            redraw = true;
        }
        if (pending_row) |target| if (lines.len > 0 and old_width != 0) {
            pending_row = null;
            var line_index: usize = 0;
            for (lines, 0..) |line, index| if (line.row == target) {
                line_index = index;
                break;
            };
            const top = @min(line_index, maxTop(lines, body_height));
            if (state.linked) tops = .{ top, top, top } else tops[state.focus] = top;
            redraw = true;
        };
        if (redraw) {
            var frame = std.Io.Writer.Allocating.init(allocator);
            defer frame.deinit();
            const w = &frame.writer;
            try w.writeAll("\x1b[H\x1b[2J" ++ theme.paper ++ theme.ink);
            const prompting = if (finder) |f| f.prompt else false;
            const panel_focus = if (finder) |f| f.open and f.focus else false;
            if (picker.kind != .closed) {
                try renderPicker(allocator, w, picker, options, columns, height);
            } else if (!usable) {
                try w.writeAll("Kata: enlarge the terminal or press 1/2/3 to hide panes. q quits.");
            } else if (rows.len == 0) {
                try w.writeAll(" K A T A  |  Plan complete. m opens Library; p chooses a plan; d restarts at a day; q quits.");
            } else {
                try w.writeAll("\x1b[1;1H\x1b[1m" ++ theme.accent ++ " K A T A  \x1b[0m" ++ theme.paper ++ theme.ink);
                const title = try std.fmt.allocPrint(allocator, "{s}  [{s}]{s}", .{ options.title, if (state.linked) "LINKED" else "INDEPENDENT", if (options.plan_mode) "" else " [FREE]" });
                defer allocator.free(title);
                try w.writeAll(clipped(title, columns -| 12));
                try w.writeAll("\x1b[2;1H" ++ theme.dim);
                try w.writeAll(clipped(notice, columns));
                const highlight = if (finder) |f| f.highlight() else null;
                if (reader_visible) {
                    try w.writeAll("\x1b[3;1H");
                    for (0..3) |pane| {
                        if (!state.enabled[pane]) continue;
                        const row_index = if (lines.len > 0) lines[@min(tops[pane], lines.len - 1)].row else 0;
                        const row = rows[row_index];
                        const label = try std.fmt.allocPrint(allocator, "{s}{s} · {s} {d}", .{ if (state.focus == pane) "▶ " else "  ", options.names[pane], row.book, row.label(pane).chapter });
                        defer allocator.free(label);
                        try w.writeAll(if (state.focus == pane and !panel_focus) theme.accent else theme.dim);
                        try cell(w, label, pane_width);
                        try w.writeAll(" │ ");
                    }
                    for (0..body_height) |line_index| {
                        try w.print("\x1b[{d};1H", .{line_index + 4});
                        for (0..3) |pane| {
                            if (!state.enabled[pane]) continue;
                            try w.writeAll(theme.ink);
                            const at = tops[pane] + line_index;
                            const query = if (highlight) |h| (if (h.pane == pane) h.query else null) else null;
                            if (at < lines.len) try markedCell(w, lines[at], pane, query, pane_width) else try cell(w, "", pane_width);
                            try w.writeAll(theme.rule ++ " │ ");
                        }
                    }
                }
                if (panel_open) try renderPanel(allocator, w, finder.?, columns - panel_width + 1, panel_width, 3, height - 2);
                try w.print("\x1b[{d};1H" ++ theme.accent, .{height - 1});
                const help = if (confirm)
                    "Mark today's assignment complete? y confirms · any other key cancels"
                else if (prompting)
                    "Type a word or phrase · matches highlight as you type · Enter searches the whole source · Esc cancels"
                else if (panel_focus)
                    "j/k select · Enter open · n/N next/previous · Tab/Esc back to reader · / new search · x close search"
                else if (panel_open)
                    "j/k scroll · h/l focus · / search · r results · n/N next/previous match · x close search"
                else
                    "j/k or ↑/↓ scroll · Ctrl-d/u page · h/l or ←/→ focus · s sync · 1/2/3 panes · g/G ends · / search · e edition";
                try w.writeAll(clipped(help, columns));
                try w.print("\x1b[{d};1H" ++ theme.dim, .{height});
                if (prompting) {
                    const f = finder.?;
                    const line = try std.fmt.allocPrint(allocator, "/{s}█   searching {s}", .{ f.promptText(), f.names[f.prompt_pane] });
                    defer allocator.free(line);
                    try w.writeAll(theme.ink);
                    try w.writeAll(clipped(line, columns));
                } else try w.writeAll(clipped(if (options.plan_mode) "m library · o free reading · p plans · d choose day · c complete day · q quit" else if (options.completable) "m plan · c complete day · q quit" else "m library · o choose place · H/L (Shift+←/→) previous/next chapter · p plans · q quit", columns));
            }
            try writeAll(frame.written());
            redraw = false;
        }
        decoder.text = if (finder) |f| f.prompt and picker.kind == .closed else false;
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
        // Search prompt and results panel take keys before the reader.
        var want_jump = false;
        if (finder) |f| {
            if (f.prompt) {
                switch (key) {
                    27 => f.cancel(),
                    10, 13 => {
                        const query = try allocator.dupe(u8, f.promptText());
                        f.submit() catch |err| {
                            notice = try std.fmt.allocPrint(allocator, "Search failed: {s}", .{@errorName(err)});
                        };
                        if (f.open and f.hits.len == 0) {
                            notice = try std.fmt.allocPrint(allocator, "No matches for “{s}” in {s}.", .{ query, f.names[f.pane] });
                            f.close();
                        } else if (f.open) {
                            // Start the list at the first result in this reading, if any.
                            for (f.hits, 0..) |hit, index| if (locate(rows, hit) != null) {
                                f.selected = index;
                                break;
                            };
                            notice = try std.fmt.allocPrint(allocator, "{d} verses contain “{s}” in {s}. Enter opens a result; n/N steps through them.", .{ f.hits.len, query, f.names[f.pane] });
                        }
                    },
                    127, 8 => f.backspace(),
                    21 => f.clear(),
                    else => f.input(key),
                }
                redraw = true;
                continue;
            }
            if (f.open and f.focus) {
                const page = @max(1, body_height / 2);
                switch (key) {
                    'j' => f.selected = @min(f.hits.len -| 1, f.selected + 1),
                    'k' => f.selected -|= 1,
                    4, 'f' => f.selected = @min(f.hits.len -| 1, f.selected + page),
                    21, 'b' => f.selected -|= page,
                    'g' => f.selected = 0,
                    'G' => f.selected = f.hits.len -| 1,
                    10, 13 => want_jump = true,
                    'n' => {
                        f.selected = @min(f.hits.len -| 1, f.selected + 1);
                        want_jump = true;
                    },
                    'N' => {
                        f.selected -|= 1;
                        want_jump = true;
                    },
                    27, 9, 'q', 'h' => f.focus = false,
                    'x' => f.close(),
                    '/' => f.begin(state.focus),
                    else => continue,
                }
                if (!want_jump) {
                    redraw = true;
                    continue;
                }
            } else if (f.open and (key == 'n' or key == 'N')) {
                if (key == 'n') f.selected = @min(f.hits.len -| 1, f.selected + 1) else f.selected -|= 1;
                want_jump = true;
            }
            if (want_jump) {
                const hit = f.current() orelse continue;
                if (locate(rows, hit)) |target| {
                    if (!state.enabled[f.pane]) {
                        remember(lines, tops, state);
                        state.enabled[f.pane] = true;
                        old_width = 0;
                    }
                    state.focus = f.pane;
                    pending_row = target;
                    redraw = true;
                    continue;
                }
                remember(lines, tops, state);
                f.reveal = true;
                return .{ .open_location = hit.location() };
            }
        }
        switch (key) {
            'q', 3 => break,
            'e' => {
                if (options.editions[state.focus] < 2) {
                    notice = if (state.focus == 0) "No other edition here. With a Psalter ingested, e switches column 1 between KJV and it in the Psalms." else "Only one edition for this column.";
                } else {
                    remember(lines, tops, state);
                    state.edition[state.focus] = (state.edition[state.focus] + 1) % options.editions[state.focus];
                    return .reload;
                }
            },
            'm', 'o', 'H', 'L' => {
                if ((key == 'H' or key == 'L') and options.plan_mode) continue;
                remember(lines, tops, state);
                return switch (key) {
                    'm' => .home,
                    'o' => .open_place,
                    'H' => .previous_chapter,
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
            'c' => if (options.plan_mode or options.completable) {
                if (rows.len == 0) notice = "Plan complete. Choose a plan or restart at a day." else confirm = true;
            },
            'p' => if (options.plans.len > 0) {
                picker = .{ .kind = .plans, .selected = options.active_plan };
            },
            'd' => if (options.plan_mode and options.plan != null) {
                picker = .{ .kind = .days, .selected = @min(options.plan.?.totalDays() - 1, state.assignment(options.today) % options.plan.?.totalDays()) };
            },
            '/' => {
                const f = finder orelse continue;
                if (rows.len == 0) continue;
                f.begin(state.focus);
            },
            'r' => {
                const f = finder orelse continue;
                if (!f.open) continue;
                f.focus = true;
            },
            'x' => {
                const f = finder orelse continue;
                if (!f.open) continue;
                f.close();
                notice = "Search closed.";
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
