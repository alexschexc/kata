//! EPUB ingest pipeline (see docs/ingest-plan.md).
//!
//!   <ingest>/<name>.epub  --convert-->  <library>/<name>             (one per book)
//!   <library>/*           --index---->  <library>/.kata-index.json    (library menu)
//!   <library>/.kata-manifest.json: fingerprints + converter version, so
//!   unchanged books are neither re-converted nor re-indexed.
//!
//! Folder locations are chosen by the user (first-run prompt) and stored in
//! <config>/kata/folders.json; Kata never guesses OS-specific paths.
const std = @import("std");
const convert = @import("convert.zig");
const document = @import("document.zig");

pub const converter_version = convert.version;
pub const manifest_name = ".kata-manifest.json";
pub const index_name = ".kata-index.json";
pub const ingest_folder = "kataIngest";
pub const library_folder = "kataLibrary";

/// User-chosen folder locations (config file `kata/folders.json`).
pub const Folders = struct {
    version: u8 = 1,
    ingest: []const u8 = "",
    library: []const u8 = "",

    pub fn configured(self: Folders) bool {
        return self.ingest.len > 0 and self.library.len > 0;
    }
};

/// Identity of a source EPUB. Cheap fields first; hash only when they change.
pub const Fingerprint = struct {
    size: u64,
    mtime_ns: i128,
    sha256: []const u8 = "",
};

pub const ManifestEntry = struct {
    /// EPUB file name inside the ingest folder, e.g. "foo.epub".
    source: []const u8,
    /// Output document name inside the library folder, e.g. "foo".
    output: []const u8,
    fingerprint: Fingerprint,
    converter_version: u32,
    title: []const u8 = "",
};

pub const Manifest = struct {
    version: u8 = 1,
    entries: []ManifestEntry = &.{},

    pub fn find(self: Manifest, source: []const u8) ?ManifestEntry {
        for (self.entries) |entry| if (std.mem.eql(u8, entry.source, source)) return entry;
        return null;
    }
};

/// One readable document in the library folder (`.kata-index.json`).
pub const IndexEntry = struct {
    /// File name in the library folder.
    file: []const u8,
    title: []const u8,
    author: []const u8 = "",
    chapters: u32,
    blocks: u32,
    size: u64,
    mtime_ns: i128,
};

pub const Index = struct {
    version: u8 = 1,
    entries: []IndexEntry = &.{},
};

pub const Outcome = enum { converted, unchanged, failed, orphaned };

pub const Report = struct {
    name: []const u8,
    outcome: Outcome,
    detail: []const u8 = "",
};

pub const Summary = struct {
    reports: []Report,
    converted: usize = 0,
    unchanged: usize = 0,
    failed: usize = 0,
    orphaned: usize = 0,
    indexed: usize = 0,
    index_rebuilt: bool = false,
};

/// Output name for a source: the file name without its ".epub" extension.
pub fn outputName(source_name: []const u8) []const u8 {
    if (std.ascii.endsWithIgnoreCase(source_name, ".epub")) return source_name[0 .. source_name.len - 5];
    return source_name;
}

fn foldersPath(allocator: std.mem.Allocator, config_home: []const u8) ![]const u8 {
    return std.fs.path.join(allocator, &.{ config_home, "kata", "folders.json" });
}

fn readJson(comptime T: type, allocator: std.mem.Allocator, io: std.Io, dir: std.Io.Dir, path: []const u8) !?T {
    const bytes = dir.readFileAlloc(io, path, allocator, .limited(16 << 20)) catch |err| switch (err) {
        error.FileNotFound => return null,
        else => return err,
    };
    return try std.json.parseFromSliceLeaky(T, allocator, bytes, .{ .ignore_unknown_fields = true, .allocate = .alloc_always });
}

/// Writes `bytes` to dir/path atomically (temporary file, then rename).
fn writeAtomic(allocator: std.mem.Allocator, io: std.Io, dir: std.Io.Dir, path: []const u8, bytes: []const u8) !void {
    const temporary = try std.fmt.allocPrint(allocator, "{s}.tmp", .{path});
    defer allocator.free(temporary);
    if (std.fs.path.dirname(path)) |parent| try dir.createDirPath(io, parent);
    try dir.writeFile(io, .{ .sub_path = temporary, .data = bytes });
    try dir.rename(temporary, dir, path, io);
}

fn writeJson(allocator: std.mem.Allocator, io: std.Io, dir: std.Io.Dir, path: []const u8, value: anytype) !void {
    const bytes = try std.json.Stringify.valueAlloc(allocator, value, .{ .whitespace = .indent_2 });
    defer allocator.free(bytes);
    try writeAtomic(allocator, io, dir, path, bytes);
}

/// Stage 0: the saved folder choice, or null when the user has not chosen.
pub fn loadFolders(allocator: std.mem.Allocator, io: std.Io, config_home: []const u8) !?Folders {
    const path = try foldersPath(allocator, config_home);
    const folders = try readJson(Folders, allocator, io, std.Io.Dir.cwd(), path) orelse return null;
    if (folders.version != 1 or !folders.configured()) return error.InvalidFolders;
    return folders;
}

/// Saves the folder choice and creates both folders.
pub fn saveFolders(allocator: std.mem.Allocator, io: std.Io, config_home: []const u8, folders: Folders) !void {
    const cwd = std.Io.Dir.cwd();
    try cwd.createDirPath(io, folders.ingest);
    try cwd.createDirPath(io, folders.library);
    try writeJson(allocator, io, cwd, try foldersPath(allocator, config_home), folders);
}

/// Expands a leading `~/` and requires an absolute result, so the saved
/// folders do not depend on the directory Kata happens to be launched from.
pub fn normalizeFolder(allocator: std.mem.Allocator, raw: []const u8, home: ?[]const u8) ![]const u8 {
    const trimmed = std.mem.trim(u8, raw, " \t");
    const expanded = if (std.mem.eql(u8, trimmed, "~") or std.mem.startsWith(u8, trimmed, "~/"))
        try std.fs.path.join(allocator, &.{ home orelse return error.HomeUnavailable, trimmed[@min(trimmed.len, 2)..] })
    else
        trimmed;
    if (!std.fs.path.isAbsolute(expanded)) return error.FolderMustBeAbsolute;
    return std.fs.path.resolve(allocator, &.{expanded});
}

test "folder paths expand home and must be absolute" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("/home/r/kataLibrary", try normalizeFolder(a, " ~/kataLibrary/ ", "/home/r"));
    try std.testing.expectEqualStrings("/srv/books", try normalizeFolder(a, "/srv/x/../books", null));
    try std.testing.expectError(error.FolderMustBeAbsolute, normalizeFolder(a, "books", "/home/r"));
}

/// Defaults offered by the first-run prompt: `kataIngest`/`kataLibrary`
/// inside a parent directory the user confirms or edits.
pub fn defaultFolders(allocator: std.mem.Allocator, parent: []const u8) !Folders {
    return .{
        .ingest = try std.fs.path.join(allocator, &.{ parent, ingest_folder }),
        .library = try std.fs.path.join(allocator, &.{ parent, library_folder }),
    };
}

fn sha256Hex(allocator: std.mem.Allocator, bytes: []const u8) ![]const u8 {
    var digest: [32]u8 = undefined;
    std.crypto.hash.sha2.Sha256.hash(bytes, &digest, .{});
    return std.fmt.allocPrint(allocator, "{x}", .{&digest});
}

fn listFiles(allocator: std.mem.Allocator, io: std.Io, path: []const u8) ![]const []const u8 {
    var dir = std.Io.Dir.cwd().openDir(io, path, .{ .iterate = true }) catch |err| switch (err) {
        error.FileNotFound => return &.{},
        else => return err,
    };
    defer dir.close(io);
    var names: std.ArrayList([]const u8) = .empty;
    var it = dir.iterate();
    while (try it.next(io)) |entry| {
        if (entry.kind != .file and entry.kind != .sym_link) continue;
        try names.append(allocator, try allocator.dupe(u8, entry.name));
    }
    std.mem.sort([]const u8, names.items, {}, struct {
        fn less(_: void, a: []const u8, b: []const u8) bool {
            return std.mem.lessThan(u8, a, b);
        }
    }.less);
    return names.items;
}

/// Runs the whole pipeline: convert new/changed EPUBs, report orphans, and
/// rebuild the index only when the library changed. Never deletes anything.
/// `force` reconverts every EPUB regardless of the manifest.
pub fn run(allocator: std.mem.Allocator, io: std.Io, folders: Folders, force: bool) !Summary {
    const cwd = std.Io.Dir.cwd();
    try cwd.createDirPath(io, folders.ingest);
    try cwd.createDirPath(io, folders.library);
    const manifest_path = try std.fs.path.join(allocator, &.{ folders.library, manifest_name });
    const old = try readJson(Manifest, allocator, io, cwd, manifest_path) orelse Manifest{};
    var entries: std.ArrayList(ManifestEntry) = .empty;
    var reports: std.ArrayList(Report) = .empty;
    var summary: Summary = .{ .reports = &.{} };
    var changed = false;

    const sources = try listFiles(allocator, io, folders.ingest);
    for (sources) |name| {
        if (!std.ascii.endsWithIgnoreCase(name, ".epub")) continue;
        const path = try std.fs.path.join(allocator, &.{ folders.ingest, name });
        const output = outputName(name);
        const output_path = try std.fs.path.join(allocator, &.{ folders.library, output });
        const stat = try cwd.statFile(io, path, .{});
        var fingerprint: Fingerprint = .{ .size = stat.size, .mtime_ns = stat.mtime.nanoseconds };
        const previous = old.find(name);
        const output_exists = if (cwd.statFile(io, output_path, .{})) |_| true else |_| false;
        const reusable = !force and output_exists and previous != null and previous.?.converter_version == converter_version;
        if (reusable and previous.?.fingerprint.size == fingerprint.size and previous.?.fingerprint.mtime_ns == fingerprint.mtime_ns) {
            try entries.append(allocator, previous.?);
            try reports.append(allocator, .{ .name = name, .outcome = .unchanged });
            summary.unchanged += 1;
            continue;
        }
        const bytes = cwd.readFileAlloc(io, path, allocator, .limited(256 << 20)) catch |err| {
            try reports.append(allocator, .{ .name = name, .outcome = .failed, .detail = @errorName(err) });
            summary.failed += 1;
            if (previous) |p| try entries.append(allocator, p);
            continue;
        };
        fingerprint.sha256 = try sha256Hex(allocator, bytes);
        // Touched but byte-identical: refresh the fingerprint, keep the output.
        if (reusable and std.mem.eql(u8, previous.?.fingerprint.sha256, fingerprint.sha256)) {
            var refreshed = previous.?;
            refreshed.fingerprint = fingerprint;
            try entries.append(allocator, refreshed);
            try reports.append(allocator, .{ .name = name, .outcome = .unchanged });
            summary.unchanged += 1;
            changed = true;
            continue;
        }
        var work = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer work.deinit();
        const result = convert.convert(work.allocator(), bytes, name) catch |err| {
            try reports.append(allocator, .{ .name = name, .outcome = .failed, .detail = @errorName(err) });
            summary.failed += 1;
            if (previous) |p| try entries.append(allocator, p);
            continue;
        };
        var out: std.Io.Writer.Allocating = .init(work.allocator());
        try document.write(&out.writer, result.doc);
        // Images first, so a document never references missing assets.
        if (result.assets.len > 0) {
            const assets_dir = try std.fmt.allocPrint(allocator, "{s}.assets", .{output_path});
            try cwd.createDirPath(io, assets_dir);
            for (result.assets) |asset| {
                const asset_path = try std.fs.path.join(allocator, &.{ assets_dir, asset.name });
                try writeAtomic(allocator, io, cwd, asset_path, asset.bytes);
            }
        }
        try writeAtomic(allocator, io, cwd, output_path, out.written());
        const s = result.stats;
        try entries.append(allocator, .{ .source = name, .output = output, .fingerprint = fingerprint, .converter_version = converter_version, .title = try allocator.dupe(u8, result.doc.title) });
        try reports.append(allocator, .{ .name = name, .outcome = .converted, .detail = try std.fmt.allocPrint(allocator, "{d} chapters, {d} blocks, {d} verses, {d} pages, {d} code, {d} figures, {d} images, {d} notes{s}", .{ s.chapters, s.blocks, s.verses, s.pages, s.code, s.figures, s.images, s.footnotes, if (s.verse_mismatches > 0) " (verse numbering corrected)" else "" }) });
        summary.converted += 1;
        changed = true;
    }
    for (old.entries) |entry| {
        var present = false;
        for (sources) |name| if (std.mem.eql(u8, name, entry.source)) {
            present = true;
        };
        if (present) continue;
        try reports.append(allocator, .{ .name = entry.output, .outcome = .orphaned, .detail = "source EPUB missing; document kept" });
        summary.orphaned += 1;
        try entries.append(allocator, entry);
    }
    if (changed or old.entries.len != entries.items.len) try writeJson(allocator, io, cwd, manifest_path, Manifest{ .entries = entries.items });
    const index_result = try index(allocator, io, folders.library, changed);
    summary.indexed = index_result.entries.len;
    summary.index_rebuilt = index_result.rebuilt;
    summary.reports = reports.items;
    return summary;
}

pub const IndexResult = struct { entries: []IndexEntry, rebuilt: bool };

/// Stage 4: index every document in the library folder. Unchanged files
/// (same size and mtime as indexed) keep their entries without reparsing.
pub fn index(allocator: std.mem.Allocator, io: std.Io, library_dir: []const u8, force: bool) !IndexResult {
    const cwd = std.Io.Dir.cwd();
    const index_path = try std.fs.path.join(allocator, &.{ library_dir, index_name });
    const old = try readJson(Index, allocator, io, cwd, index_path) orelse Index{};
    var entries: std.ArrayList(IndexEntry) = .empty;
    var rebuilt = force;
    for (try listFiles(allocator, io, library_dir)) |name| {
        if (name.len == 0 or name[0] == '.' or std.mem.endsWith(u8, name, ".tmp")) continue;
        const path = try std.fs.path.join(allocator, &.{ library_dir, name });
        const stat = try cwd.statFile(io, path, .{});
        var reused = false;
        for (old.entries) |entry| if (std.mem.eql(u8, entry.file, name) and entry.size == stat.size and entry.mtime_ns == stat.mtime.nanoseconds) {
            try entries.append(allocator, entry);
            reused = true;
            break;
        };
        if (reused) continue;
        rebuilt = true;
        var work = std.heap.ArenaAllocator.init(std.heap.page_allocator);
        defer work.deinit();
        const bytes = cwd.readFileAlloc(io, path, work.allocator(), .limited(256 << 20)) catch continue;
        const doc = document.parse(work.allocator(), bytes) catch continue; // not a Kata document
        try entries.append(allocator, .{
            .file = name,
            .title = try allocator.dupe(u8, doc.title),
            .author = try allocator.dupe(u8, doc.author),
            .chapters = @intCast(doc.chapters.len),
            .blocks = @intCast(doc.blockCount()),
            .size = stat.size,
            .mtime_ns = stat.mtime.nanoseconds,
        });
    }
    if (entries.items.len != old.entries.len) rebuilt = true;
    if (rebuilt) try writeJson(allocator, io, cwd, index_path, Index{ .entries = entries.items });
    return .{ .entries = entries.items, .rebuilt = rebuilt };
}

/// Reads the current index without touching the library (fast startup path).
pub fn loadIndex(allocator: std.mem.Allocator, io: std.Io, library_dir: []const u8) ![]IndexEntry {
    const index_path = try std.fs.path.join(allocator, &.{ library_dir, index_name });
    const loaded = try readJson(Index, allocator, io, std.Io.Dir.cwd(), index_path) orelse return &.{};
    return loaded.entries;
}

test "output name drops the epub extension only" {
    try std.testing.expectEqualStrings("buildingmicroservices2ndedition", outputName("buildingmicroservices2ndedition.epub"));
    try std.testing.expectEqualStrings("The Psalter (1)", outputName("The Psalter (1).EPUB"));
    try std.testing.expectEqualStrings("notes.txt", outputName("notes.txt"));
}

test "pipeline converts once, skips unchanged, indexes, and reports orphans" {
    const io = std.testing.io;
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const sample = "examplePubs/The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub";
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, sample, a, .limited(64 << 20)) catch |err| switch (err) {
        error.FileNotFound => return error.SkipZigTest,
        else => return err,
    };
    var tmp = std.testing.tmpDir(.{});
    defer tmp.cleanup();
    const root = try std.fmt.allocPrint(a, ".zig-cache/tmp/{s}", .{tmp.sub_path});
    const config = try std.fs.path.join(a, &.{ root, "config" });
    try std.testing.expect(try loadFolders(a, io, config) == null);
    const folders = try defaultFolders(a, root);
    try saveFolders(a, io, config, folders);
    try std.testing.expectEqualStrings(folders.library, (try loadFolders(a, io, config)).?.library);
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = try std.fs.path.join(a, &.{ folders.ingest, "Psalter.epub" }), .data = bytes });
    try std.Io.Dir.cwd().writeFile(io, .{ .sub_path = try std.fs.path.join(a, &.{ folders.ingest, "broken.epub" }), .data = "not a zip" });

    const first = try run(a, io, folders, false);
    try std.testing.expectEqual(@as(usize, 1), first.converted);
    try std.testing.expectEqual(@as(usize, 1), first.failed);
    try std.testing.expectEqual(@as(usize, 1), first.indexed);
    try std.testing.expect(first.index_rebuilt);
    const doc_bytes = try std.Io.Dir.cwd().readFileAlloc(io, try std.fs.path.join(a, &.{ folders.library, "Psalter" }), a, .limited(64 << 20));
    const doc = try document.parse(a, doc_bytes);
    try std.testing.expect(std.mem.indexOf(u8, doc.title, "Psalter") != null);

    const second = try run(a, io, folders, false);
    try std.testing.expectEqual(@as(usize, 0), second.converted);
    try std.testing.expectEqual(@as(usize, 1), second.unchanged);
    try std.testing.expect(!second.index_rebuilt);
    const entries = try loadIndex(a, io, folders.library);
    try std.testing.expectEqual(@as(usize, 1), entries.len);
    try std.testing.expectEqualStrings("Psalter", entries[0].file);

    try std.Io.Dir.cwd().deleteFile(io, try std.fs.path.join(a, &.{ folders.ingest, "Psalter.epub" }));
    const third = try run(a, io, folders, false);
    try std.testing.expectEqual(@as(usize, 1), third.orphaned);
    try std.testing.expectEqual(@as(usize, 1), third.indexed); // kept, never deleted
}
