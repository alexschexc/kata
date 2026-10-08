const std = @import("std");
pub const Kind = enum { state, config };

pub fn root(allocator: std.mem.Allocator, env: anytype, windows: bool, kind: Kind) ![]const u8 {
    const xdg = if (kind == .state) "XDG_STATE_HOME" else "XDG_CONFIG_HOME";
    if (env.get(xdg)) |value| return allocator.dupe(u8, value);
    if (windows) {
        const appdata = if (kind == .state) "LOCALAPPDATA" else "APPDATA";
        if (env.get(appdata)) |value| return allocator.dupe(u8, value);
        if (env.get("USERPROFILE")) |profile| return std.fmt.allocPrint(allocator, "{s}/AppData/{s}", .{ profile, if (kind == .state) "Local" else "Roaming" });
    }
    const home = env.get("HOME");
    return std.fmt.allocPrint(allocator, "{s}/{s}", .{ home orelse return error.HomeUnavailable, if (kind == .state) ".local/state" else ".config" });
}

test "Windows uses LocalAppData for state and roaming AppData for plans without HOME" {
    var env = std.StringHashMap([]const u8).init(std.testing.allocator);
    defer env.deinit();
    try env.put("LOCALAPPDATA", "C:/Users/Reader/AppData/Local");
    try env.put("APPDATA", "C:/Users/Reader/AppData/Roaming");
    const state = try root(std.testing.allocator, env, true, .state);
    defer std.testing.allocator.free(state);
    const config = try root(std.testing.allocator, env, true, .config);
    defer std.testing.allocator.free(config);
    try std.testing.expectEqualStrings("C:/Users/Reader/AppData/Local", state);
    try std.testing.expectEqualStrings("C:/Users/Reader/AppData/Roaming", config);
}

test "Windows USERPROFILE fallback follows native AppData layout" {
    var env = std.StringHashMap([]const u8).init(std.testing.allocator);
    defer env.deinit();
    try env.put("USERPROFILE", "C:/Users/Reader");
    try env.put("HOME", "/different-home");
    const state = try root(std.testing.allocator, env, true, .state);
    defer std.testing.allocator.free(state);
    const config = try root(std.testing.allocator, env, true, .config);
    defer std.testing.allocator.free(config);
    try std.testing.expectEqualStrings("C:/Users/Reader/AppData/Local", state);
    try std.testing.expectEqualStrings("C:/Users/Reader/AppData/Roaming", config);
}

test "XDG precedence and Unix HOME layout are preserved" {
    var env = std.StringHashMap([]const u8).init(std.testing.allocator);
    defer env.deinit();
    try env.put("HOME", "/reader");
    try env.put("LOCALAPPDATA", "C:/Local");
    const unix = try root(std.testing.allocator, env, false, .state);
    defer std.testing.allocator.free(unix);
    try std.testing.expectEqualStrings("/reader/.local/state", unix);
    try env.put("XDG_STATE_HOME", "/xdg-state");
    try env.put("XDG_CONFIG_HOME", "/xdg-config");
    const state = try root(std.testing.allocator, env, true, .state);
    defer std.testing.allocator.free(state);
    const config = try root(std.testing.allocator, env, true, .config);
    defer std.testing.allocator.free(config);
    try std.testing.expectEqualStrings("/xdg-state", state);
    try std.testing.expectEqualStrings("/xdg-config", config);
}

test "missing environment reports HomeUnavailable rather than guessing" {
    var env = std.StringHashMap([]const u8).init(std.testing.allocator);
    defer env.deinit();
    try std.testing.expectError(error.HomeUnavailable, root(std.testing.allocator, env, true, .state));
    try std.testing.expectError(error.HomeUnavailable, root(std.testing.allocator, env, false, .config));
}
