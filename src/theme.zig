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

test "palette escapes are well formed" {
    try std.testing.expectEqualStrings("\x1b[48;2;6;11;30m", paper);
    try std.testing.expectEqualStrings("\x1b[38;2;125;130;217m", accent);
    try std.testing.expectEqualStrings("\x1b[48;2;125;130;217m\x1b[38;2;6;11;30m", accent_bg ++ on_accent);
}
