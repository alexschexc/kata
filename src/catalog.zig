const std = @import("std");
const scheduling = @import("plan.zig");

pub const Entry = struct { id: []const u8, plan: scheduling.Plan, hash: u64 };
pub const Catalog = struct {
    allocator: std.mem.Allocator,
    entries: std.ArrayList(Entry) = .empty,
    skipped: usize = 0,

    pub fn add(self: *Catalog, id: []const u8, bytes: []const u8) !usize {
        const hash = std.hash.Wyhash.hash(0, bytes);
        for (self.entries.items, 0..) |entry, i| if (entry.hash == hash) return i;
        var plan = try scheduling.Plan.init(self.allocator, bytes);
        errdefer plan.deinit();
        if (!std.unicode.utf8ValidateSlice(plan.name())) return error.InvalidPlanName;
        for (plan.name()) |ch| if (ch < 32 or ch == 127) return error.InvalidPlanName;
        const owned_id = try self.allocator.dupe(u8, id);
        errdefer self.allocator.free(owned_id);
        try self.entries.append(self.allocator, .{ .id = owned_id, .plan = plan, .hash = hash });
        return self.entries.items.len - 1;
    }
    pub fn addFile(self: *Catalog, io: std.Io, path: []const u8) !usize {
        const cwd = try std.process.currentPathAlloc(io, self.allocator);
        defer self.allocator.free(cwd);
        const absolute = try std.fs.path.resolve(self.allocator, &.{ cwd, path });
        defer self.allocator.free(absolute);
        const bytes = try std.Io.Dir.cwd().readFileAlloc(io, absolute, self.allocator, .limited(1024 * 1024));
        defer self.allocator.free(bytes);
        return self.add(absolute, bytes);
    }
    pub fn discover(self: *Catalog, io: std.Io, path: []const u8) !void {
        const dir = std.Io.Dir.cwd().openDir(io, path, .{ .iterate = true }) catch |err| switch (err) {
            error.FileNotFound => return,
            else => return err,
        };
        defer dir.close(io);
        var names: std.ArrayList([]const u8) = .empty;
        defer {
            for (names.items) |name| self.allocator.free(name);
            names.deinit(self.allocator);
        }
        var iterator = dir.iterate();
        while (try iterator.next(io)) |entry| {
            if (entry.kind != .file or !std.mem.endsWith(u8, entry.name, ".json")) continue;
            const name = try self.allocator.dupe(u8, entry.name);
            errdefer self.allocator.free(name);
            try names.append(self.allocator, name);
        }
        std.mem.sort([]const u8, names.items, {}, struct {
            fn less(_: void, a: []const u8, b: []const u8) bool {
                return std.mem.lessThan(u8, a, b);
            }
        }.less);
        for (names.items) |name| {
            const full = try std.fs.path.join(self.allocator, &.{ path, name });
            defer self.allocator.free(full);
            _ = self.addFile(io, full) catch |err| switch (err) {
                error.OutOfMemory => return err,
                else => {
                    self.skipped += 1;
                    continue;
                },
            };
        }
    }
    pub fn deinit(self: *Catalog) void {
        for (self.entries.items) |*entry| {
            self.allocator.free(entry.id);
            entry.plan.deinit();
        }
        self.entries.deinit(self.allocator);
    }
};

pub fn progressPath(allocator: std.mem.Allocator, base: []const u8, base_hash: u64, hash: u64) ![]const u8 {
    if (base_hash == 0 or base_hash == hash) return allocator.dupe(u8, base);
    return std.fmt.allocPrint(allocator, "{s}.plans/{x}.json", .{ base, hash });
}

pub fn loadSelected(allocator: std.mem.Allocator, io: std.Io, base: []const u8) !?[]const u8 {
    const path = try std.fmt.allocPrint(allocator, "{s}.selection.json", .{base});
    defer allocator.free(path);
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(65536)) catch |err| switch (err) {
        error.FileNotFound => return null,
        else => return err,
    };
    defer allocator.free(bytes);
    const parsed = try std.json.parseFromSlice(struct { id: []const u8 }, allocator, bytes, .{});
    defer parsed.deinit();
    return try allocator.dupe(u8, parsed.value.id);
}

pub fn saveSelected(allocator: std.mem.Allocator, io: std.Io, base: []const u8, id: []const u8) !void {
    const path = try std.fmt.allocPrint(allocator, "{s}.selection.json", .{base});
    defer allocator.free(path);
    const temporary = try std.fmt.allocPrint(allocator, "{s}.tmp", .{path});
    defer allocator.free(temporary);
    const bytes = try std.json.Stringify.valueAlloc(allocator, .{ .id = id }, .{});
    defer allocator.free(bytes);
    const cwd = std.Io.Dir.cwd();
    if (std.fs.path.dirname(path)) |parent| try cwd.createDirPath(io, parent);
    try cwd.writeFile(io, .{ .sub_path = temporary, .data = bytes });
    try cwd.rename(temporary, cwd, path, io);
}

test "catalog lists named plans and deduplicates identical config content" {
    var catalog: Catalog = .{ .allocator = std.testing.allocator };
    defer catalog.deinit();
    const bytes = "{\"name\":\"Short\",\"repeat\":true,\"streams\":[{\"books\":[{\"name\":\"John\",\"chapters\":1}]}],\"phases\":[{\"days\":1,\"rates\":[1]}]}";
    try std.testing.expectEqual(@as(usize, 0), try catalog.add("builtin:test", bytes));
    try std.testing.expectEqual(@as(usize, 0), try catalog.add("copy.json", bytes));
    try std.testing.expectEqual(@as(usize, 1), catalog.entries.items.len);
    try std.testing.expectEqualStrings("Short", catalog.entries.items[0].plan.name());
}

test "plan progress uses existing base state or an isolated sidecar" {
    const same = try progressPath(std.testing.allocator, "/tmp/state.json", 10, 10);
    defer std.testing.allocator.free(same);
    try std.testing.expectEqualStrings("/tmp/state.json", same);
    const other = try progressPath(std.testing.allocator, "/tmp/state.json", 10, 11);
    defer std.testing.allocator.free(other);
    try std.testing.expectEqualStrings("/tmp/state.json.plans/b.json", other);
    const fresh = try progressPath(std.testing.allocator, "/tmp/state.json", 0, 10);
    defer std.testing.allocator.free(fresh);
    try std.testing.expectEqualStrings("/tmp/state.json", fresh);
}
