//! Application palette. Defaults are the Omarchy "Ethereal" theme
//! (/usr/share/omarchy/themes/ethereal/colors.toml), compiled in so every
//! platform gets the same look without reading any system file.
//!
//! Roles: `paper` page background, `ink` body text, `dim` secondary text,
//! `accent` pointer/titles/focus, `rule` separators and gutters, `code`
//! code blocks. `accent_bg` + `on_accent` form the search-match highlight.
const std = @import("std");

fn fg(comptime r: u8, comptime g: u8, comptime b: u8) *const [std.fmt.count("\x1b[38;2;{d};{d};{d}m", .{ r, g, b }):0]u8 {
    return std.fmt.comptimePrint("\x1b[38;2;{d};{d};{d}m", .{ r, g, b });
}

fn bg(comptime r: u8, comptime g: u8, comptime b: u8) *const [std.fmt.count("\x1b[48;2;{d};{d};{d}m", .{ r, g, b }):0]u8 {
    return std.fmt.comptimePrint("\x1b[48;2;{d};{d};{d}m", .{ r, g, b });
}

/// Ethereal: background #060B1E.
pub const paper = bg(6, 11, 30);
/// Ethereal: foreground #ffcead.
pub const ink = fg(255, 206, 173);
/// Ethereal: muted #6d7db6.
pub const dim = fg(109, 125, 182);
/// Ethereal: accent #7d82d9.
pub const accent = fg(125, 130, 217);
/// Midpoint of Ethereal selection #252e56 and muted #6d7db6, so rules stay
/// visible on the dark background.
pub const rule = fg(73, 86, 134);
/// Ethereal: green #92a593.
pub const code = fg(146, 165, 147);
/// Search-match highlight: accent background, page-coloured text.
pub const accent_bg = bg(125, 130, 217);
pub const on_accent = fg(6, 11, 30);

/// Nearest xterm-256 palette index (6x6x6 cube or 24-step gray ramp).
pub fn rgbTo256(r: u8, g: u8, b: u8) u8 {
    const levels = [6]i32{ 0, 95, 135, 175, 215, 255 };
    const rgb = [3]i32{ r, g, b };
    var idx: [3]usize = undefined;
    var cube_err: i32 = 0;
    for (rgb, 0..) |v, k| {
        var best: usize = 0;
        for (levels, 0..) |l, i| {
            if (@abs(l - v) < @abs(levels[best] - v)) best = i;
        }
        idx[k] = best;
        const d = levels[best] - v;
        cube_err += d * d;
    }
    const avg = @divTrunc(rgb[0] + rgb[1] + rgb[2], 3);
    const step: i32 = std.math.clamp(@divTrunc(avg - 3, 10), 0, 23);
    const gray = 8 + step * 10;
    var gray_err: i32 = 0;
    for (rgb) |v| gray_err += (gray - v) * (gray - v);
    if (gray_err < cube_err) return @intCast(232 + step);
    return @intCast(16 + 36 * idx[0] + 6 * idx[1] + idx[2]);
}

/// Rewrites 24-bit colour sequences (`ESC[38;2;R;G;Bm` / `ESC[48;2;R;G;Bm`)
/// in `bytes` to 256-colour ones (`ESC[38;5;Nm` / `ESC[48;5;Nm`) for terminals
/// such as macOS Terminal.app that do not understand truecolor. Everything
/// else passes through unchanged. `sink` receives output in order.
pub fn downsample(bytes: []const u8, sink: anytype) !void {
    var i: usize = 0;
    var start: usize = 0;
    while (i < bytes.len) {
        if (bytes[i] == 0x1b and i + 7 <= bytes.len and bytes[i + 1] == '[' and
            (bytes[i + 2] == '3' or bytes[i + 2] == '4') and bytes[i + 3] == '8' and
            bytes[i + 4] == ';' and bytes[i + 5] == '2' and bytes[i + 6] == ';')
        {
            var vals: [3]u16 = undefined;
            var j = i + 7;
            var ok = true;
            for (0..3) |n| {
                var v: u16 = 0;
                var digits: usize = 0;
                while (j < bytes.len and bytes[j] >= '0' and bytes[j] <= '9') : (j += 1) {
                    v = v *| 10 +| (bytes[j] - '0');
                    digits += 1;
                }
                const want: u8 = if (n == 2) 'm' else ';';
                if (digits == 0 or v > 255 or j >= bytes.len or bytes[j] != want) {
                    ok = false;
                    break;
                }
                vals[n] = v;
                j += 1;
            }
            if (ok) {
                try sink.write(bytes[start..i]);
                var buf: [24]u8 = undefined;
                const out = std.fmt.bufPrint(&buf, "\x1b[{c}8;5;{d}m", .{ bytes[i + 2], rgbTo256(@intCast(vals[0]), @intCast(vals[1]), @intCast(vals[2])) }) catch unreachable;
                try sink.write(out);
                i = j;
                start = j;
                continue;
            }
        }
        i += 1;
    }
    try sink.write(bytes[start..]);
}

test "downsample maps truecolor to 256 and leaves other bytes alone" {
    const Collect = struct {
        list: *std.ArrayList(u8),
        fn write(self: @This(), b: []const u8) !void {
            try self.list.appendSlice(std.testing.allocator, b);
        }
    };
    var list: std.ArrayList(u8) = .empty;
    defer list.deinit(std.testing.allocator);
    try downsample("a\x1b[1m" ++ paper ++ ink ++ "┃x\x1b[38;2;999;1;1m", Collect{ .list = &list });
    try std.testing.expectEqualStrings("a\x1b[1m\x1b[48;5;233m\x1b[38;5;223m┃x\x1b[38;2;999;1;1m", list.items);
    try std.testing.expectEqual(@as(u8, 16), rgbTo256(0, 0, 0));
    try std.testing.expectEqual(@as(u8, 231), rgbTo256(255, 255, 255));
}

test "palette escapes are well formed" {
    try std.testing.expectEqualStrings("\x1b[48;2;6;11;30m", paper);
    try std.testing.expectEqualStrings("\x1b[38;2;125;130;217m", accent);
    try std.testing.expectEqualStrings("\x1b[48;2;125;130;217m\x1b[38;2;6;11;30m", accent_bg ++ on_accent);
}
