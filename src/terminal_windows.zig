const std = @import("std");
const types = @import("terminal_types.zig");
const geometry = @import("console_geometry.zig");
const buffering = @import("console_queue.zig");
const c = @cImport({
    @cDefine("WIN32_LEAN_AND_MEAN", "1");
    @cInclude("windows.h");
});

var interrupted = std.atomic.Value(bool).init(false);
var active = std.atomic.Value(bool).init(false);
var queue: buffering.Queue(8192) = .{};
var reader_stopping = std.atomic.Value(bool).init(false);
var reader_failed = std.atomic.Value(bool).init(false);

// VT input translation is available through ReadFile/ReadConsole, not the
// low-level ReadConsoleInput API. A cancellable producer lets the UI keep an
// 80 ms interbyte deadline even while ReadFile waits for character input.
fn produce(context: c.LPVOID) callconv(.winapi) c.DWORD {
    const in: c.HANDLE = context;
    var bytes: [256]u8 = undefined;
    while (!reader_stopping.load(.acquire)) {
        var count: c.DWORD = 0;
        if (c.ReadFile(in, &bytes, bytes.len, &count, null) == 0 or count == 0) {
            if (!reader_stopping.load(.acquire)) reader_failed.store(true, .release);
            return 0;
        }
        for (bytes[0..count]) |value| {
            while (!queue.push(value)) {
                if (reader_stopping.load(.acquire)) return 0;
                c.Sleep(1);
            }
        }
    }
    return 0;
}
fn joinReader(thread: c.HANDLE) void {
    reader_stopping.store(true, .release);
    // Repeat cancellation to cover the race between the producer's stop check
    // and entering ReadFile. Never destroy its queue while it is still running.
    while (c.WaitForSingleObject(thread, 10) == c.WAIT_TIMEOUT) _ = c.CancelSynchronousIo(thread);
    _ = c.CloseHandle(thread);
}

fn stop(event: c.DWORD) callconv(.winapi) c.BOOL {
    switch (event) {
        c.CTRL_C_EVENT, c.CTRL_BREAK_EVENT, c.CTRL_CLOSE_EVENT, c.CTRL_LOGOFF_EVENT, c.CTRL_SHUTDOWN_EVENT => {},
        else => return 0,
    }
    if (!active.load(.acquire)) return 0;
    interrupt();
    // Close/logoff handlers must remain alive while the application saves state
    // and restores modes: Windows terminates the process when this callback returns.
    if (event != c.CTRL_C_EVENT and event != c.CTRL_BREAK_EVENT) {
        var waited: usize = 0;
        while (active.load(.acquire) and waited < 4000) : (waited += 10) c.Sleep(10);
    }
    return 1;
}

pub fn isInterrupted() bool {
    return interrupted.load(.monotonic);
}
pub fn interrupt() void {
    interrupted.store(true, .monotonic);
}

fn handle(which: c.DWORD) !c.HANDLE {
    const result = c.GetStdHandle(which);
    if (result == null or result == c.INVALID_HANDLE_VALUE) return error.InteractiveTerminalRequired;
    return result;
}

pub fn writeAll(bytes: []const u8) !void {
    const output = try handle(c.STD_OUTPUT_HANDLE);
    var mode: c.DWORD = 0;
    const console = c.GetConsoleMode(output, &mode) != 0;
    const previous = if (console) c.GetConsoleOutputCP() else 0;
    if (console and c.SetConsoleOutputCP(c.CP_UTF8) == 0) return error.TerminalWrite;
    defer if (console) {
        _ = c.SetConsoleOutputCP(previous);
    };
    var offset: usize = 0;
    while (offset < bytes.len) {
        var written: c.DWORD = 0;
        const length: c.DWORD = @intCast(@min(bytes.len - offset, 0x7fffffff));
        if (c.WriteFile(output, bytes.ptr + offset, length, &written, null) == 0 or written == 0) return error.TerminalWrite;
        offset += written;
    }
}

pub const Terminal = struct {
    input: c.HANDLE,
    output: c.HANDLE,
    input_mode: c.DWORD,
    output_mode: c.DWORD,
    input_cp: c.UINT,
    output_cp: c.UINT,
    reader: c.HANDLE,

    pub fn init() !Terminal {
        const in = try handle(c.STD_INPUT_HANDLE);
        const out = try handle(c.STD_OUTPUT_HANDLE);
        var in_mode: c.DWORD = 0;
        var out_mode: c.DWORD = 0;
        if (c.GetConsoleMode(in, &in_mode) == 0 or c.GetConsoleMode(out, &out_mode) == 0) return error.InteractiveTerminalRequired;
        var old: Terminal = .{ .input = in, .output = out, .input_mode = in_mode, .output_mode = out_mode, .input_cp = c.GetConsoleCP(), .output_cp = c.GetConsoleOutputCP(), .reader = null };
        // Disable line/echo/processed input, mouse and QuickEdit selection; retain
        // VT framing (including paste) and receive window events for live resize.
        const raw = (in_mode & ~@as(c.DWORD, c.ENABLE_LINE_INPUT | c.ENABLE_ECHO_INPUT | c.ENABLE_PROCESSED_INPUT | c.ENABLE_QUICK_EDIT_MODE | c.ENABLE_MOUSE_INPUT)) | c.ENABLE_EXTENDED_FLAGS | c.ENABLE_WINDOW_INPUT | c.ENABLE_VIRTUAL_TERMINAL_INPUT;
        if (c.SetConsoleMode(in, raw) == 0) return error.TerminalSetup;
        errdefer _ = c.SetConsoleMode(in, in_mode);
        if (c.SetConsoleMode(out, out_mode | c.ENABLE_PROCESSED_OUTPUT | c.ENABLE_VIRTUAL_TERMINAL_PROCESSING) == 0) return error.TerminalSetup;
        errdefer _ = c.SetConsoleMode(out, out_mode);
        if (c.SetConsoleCP(c.CP_UTF8) == 0) return error.TerminalSetup;
        errdefer _ = c.SetConsoleCP(old.input_cp);
        if (c.SetConsoleOutputCP(c.CP_UTF8) == 0) return error.TerminalSetup;
        errdefer _ = c.SetConsoleOutputCP(old.output_cp);
        interrupted.store(false, .monotonic);
        queue = .{};
        reader_stopping.store(false, .release);
        reader_failed.store(false, .release);
        old.reader = c.CreateThread(null, 0, produce, in, 0, null);
        if (old.reader == null) return error.TerminalSetup;
        errdefer joinReader(old.reader);
        if (c.SetConsoleCtrlHandler(stop, 1) == 0) return error.TerminalSetup;
        active.store(true, .release);
        return old;
    }
    pub fn deinit(self: *Terminal) void {
        joinReader(self.reader);
        _ = c.SetConsoleMode(self.input, self.input_mode);
        _ = c.SetConsoleMode(self.output, self.output_mode);
        _ = c.SetConsoleCP(self.input_cp);
        _ = c.SetConsoleOutputCP(self.output_cp);
        active.store(false, .release);
        _ = c.SetConsoleCtrlHandler(stop, 0);
    }
};

pub fn size() !types.Size {
    const out = try handle(c.STD_OUTPUT_HANDLE);
    var info: c.CONSOLE_SCREEN_BUFFER_INFO = undefined;
    if (c.GetConsoleScreenBufferInfo(out, &info) == 0) return error.TerminalSize;
    return .{ .columns = geometry.extent(info.srWindow.Left, info.srWindow.Right, 100), .rows = geometry.extent(info.srWindow.Top, info.srWindow.Bottom, 30) };
}

pub fn readByte() !types.Read {
    var waited: usize = 0;
    while (waited < 80) : (waited += 5) {
        if (queue.pop()) |value| return .{ .byte = value };
        if (reader_failed.load(.acquire)) return error.TerminalRead;
        if (isInterrupted()) return .ignored;
        c.Sleep(5);
    }
    return .timeout;
}

test {
    std.testing.refAllDecls(geometry);
    std.testing.refAllDecls(buffering);
}
