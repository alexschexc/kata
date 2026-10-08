const std = @import("std");
const types = @import("terminal_types.zig");
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
        }
        return terminal;
    }
    pub fn deinit(self: *Terminal) void {
        _ = c.tcsetattr(0, c.TCSAFLUSH, &self.original);
        _ = c.signal(c.SIGINT, self.old_int);
        _ = c.signal(c.SIGTERM, self.old_term);
        _ = c.signal(c.SIGHUP, self.old_hup);
    }
};

pub fn isInterrupted() bool {
    return interrupted.load(.monotonic);
}
pub fn interrupt() void {
    interrupted.store(true, .monotonic);
}
pub fn size() !types.Size {
    var value: c.struct_winsize = std.mem.zeroes(c.struct_winsize);
    if (c.ioctl(1, c.TIOCGWINSZ, &value) != 0) return error.TerminalSize;
    return .{ .columns = if (value.ws_col > 0) value.ws_col else 100, .rows = if (value.ws_row > 0) value.ws_row else 30 };
}
pub fn readByte() !types.Read {
    var descriptor = c.struct_pollfd{ .fd = 0, .events = c.POLLIN, .revents = 0 };
    const ready = poll(@ptrCast(&descriptor), 1, 80);
    if (ready < 0) {
        if (isInterrupted()) return .ignored;
        return error.TerminalRead;
    }
    if (ready == 0) return .timeout;
    if ((descriptor.revents & (c.POLLHUP | c.POLLERR | c.POLLNVAL)) != 0) return error.TerminalRead;
    var value: u8 = 0;
    if (c.read(0, &value, 1) != 1) return error.TerminalRead;
    return .{ .byte = value };
}
