//! EPUB ingest pipeline (SKELETON — see docs/ingest-plan.md).
//!
//!   kataIngest/<name>.epub  --convert-->  kataLibrary/<name>      (one per book)
//!   kataLibrary/*           --index---->  kataLibrary/.kata-index  (library menu)
//!   manifest (fingerprints, converter version) skips unchanged work.
//!
//! Folder locations are chosen by the user on first run and stored in
//! <config>/kata/folders.json; Kata never guesses OS-specific paths.
const std = @import("std");

/// Bump when conversion output changes; every book is then re-converted.
pub const converter_version: u32 = 1;

/// User-chosen folder locations (config file `kata/folders.json`).
pub const Folders = struct {
    version: u8 = 1,
    ingest: []const u8,
    library: []const u8,
};

/// Identity of a source EPUB. Cheap fields first; hash only when they change.
pub const Fingerprint = struct {
    size: u64,
    mtime_ns: i128,
    sha256: [64]u8 = @splat('0'),
};

/// One manifest record per source EPUB (stored in kataLibrary/.kata-manifest.json).
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
};

/// What `plan` decided for each EPUB in the ingest folder.
pub const Work = union(enum) {
    convert: []const u8, // new or changed source
    skip: []const u8, // fingerprint and converter version match
    orphan: []const u8, // library doc whose source EPUB is gone (reported, never deleted)
};

/// Output name for a source: the file name without its ".epub" extension.
pub fn outputName(source_name: []const u8) []const u8 {
    if (std.ascii.endsWithIgnoreCase(source_name, ".epub")) return source_name[0 .. source_name.len - 5];
    return source_name;
}

/// Converted document model (TODO: serialize as the library file format).
/// Blocks are ordered; locators (chapter/verse/page) are optional per block.
pub const Block = struct {
    kind: Kind,
    text: []const u8,
    chapter: ?u32 = null, // section index from spine/TOC
    verse: ?u32 = null, // explicit or inferred verse number (religious texts)
    page: ?[]const u8 = null, // print page label from epub:type="pagebreak"

    pub const Kind = enum { heading, subheading, paragraph, verse, superscription, rubric, code, quote, list_item, table_row, figure, footnote, page_marker };
};

pub const Document = struct {
    title: []const u8,
    author: []const u8 = "",
    chapters: []const Chapter,

    pub const Chapter = struct { title: []const u8, blocks: []const Block };
};

// ---------------------------------------------------------------------------
// Pipeline stages. Each returns error.NotImplemented until built out.
// ---------------------------------------------------------------------------

/// Stage 0: load folders.json, or null when the user has not chosen yet.
pub fn loadFolders(allocator: std.mem.Allocator, io: std.Io, config_home: []const u8) !?Folders {
    _ = .{ allocator, io, config_home };
    return error.NotImplemented;
}

/// Stage 1: compare ingest-folder EPUBs against the manifest.
pub fn plan(allocator: std.mem.Allocator, io: std.Io, folders: Folders, manifest: Manifest) ![]Work {
    _ = .{ allocator, io, folders, manifest };
    return error.NotImplemented;
}

/// Stage 2: read container.xml → OPF → spine/nav, parse XHTML, build Document.
pub fn convert(allocator: std.mem.Allocator, io: std.Io, epub_path: []const u8) !Document {
    _ = .{ allocator, io, epub_path };
    return error.NotImplemented;
}

/// Stage 3: write kataLibrary/<name> atomically (tmp + rename).
pub fn write(allocator: std.mem.Allocator, io: std.Io, library_dir: []const u8, name: []const u8, document: Document) !void {
    _ = .{ allocator, io, library_dir, name, document };
    return error.NotImplemented;
}

/// Stage 4: rebuild kataLibrary/.kata-index when any document changed.
pub fn index(allocator: std.mem.Allocator, io: std.Io, library_dir: []const u8) !void {
    _ = .{ allocator, io, library_dir };
    return error.NotImplemented;
}

test "output name drops the epub extension only" {
    try std.testing.expectEqualStrings("buildingmicroservices2ndedition", outputName("buildingmicroservices2ndedition.epub"));
    try std.testing.expectEqualStrings("The Psalter (1)", outputName("The Psalter (1).EPUB"));
    try std.testing.expectEqualStrings("notes.txt", outputName("notes.txt"));
}
