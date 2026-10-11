//! Reading-plan units for ingested books. Front/back matter (cover, title,
//! copyright, dedication, contents, preface, foreword, introduction, index,
//! glossary, colophon, tiny pages) is never part of a plan.
//!
//! * Psalters (10+ "Kathisma" chapters): chapter = kathisma, section =
//!   stasis (split after each "Glory…" rubric).
//! * Books with section headings (technical books): chapter, or section =
//!   the chapter split at its top-level section headings (small sections
//!   merge with their neighbour).
//! * Books without them (fiction): chapter, or section = quarter chapter
//!   (never smaller than a quarter; short chapters stay whole).
const std = @import("std");
const document = @import("document.zig");

pub const Kind = enum { chapter, section };

/// Blocks [start, end) of chapter `chapter`.
pub const Unit = struct { chapter: usize, start: usize, end: usize, label: []const u8 };

const min_body = 2000; // chapters with less text are matter/dividers
const min_section = 1500; // smaller sections merge into a neighbour
const quarter_from = 8000; // shorter chapters are not split into quarters

fn textSize(blocks: []const document.Block) usize {
    var n: usize = 0;
    for (blocks) |b| if (b.kind != .heading and b.kind != .image) {
        n += b.text.len;
    };
    return n;
}

const exact_matter = [_][]const u8{ "cover", "titlepage", "title page", "copyright", "dedication", "contents", "preface", "foreword", "introduction", "acknowledgments", "acknowledgements", "index", "glossary", "bibliography", "colophon", "footnotes", "frontispiece", "epigraph" };
const prefix_matter = [_][]const u8{ "praise for", "about the author", "about the technical", "brief contents", "contents in detail", "table of contents", "further resources", "online appendix", "also by" };

pub fn isMatter(doc: document.Document, index: usize) bool {
    const chapter = doc.chapters[index];
    const title = std.mem.trim(u8, chapter.title, " ");
    var lower_buf: [96]u8 = undefined;
    const lower = std.ascii.lowerString(&lower_buf, title[0..@min(title.len, lower_buf.len)]);
    for (exact_matter) |word| if (std.mem.eql(u8, lower, word)) return true;
    for (prefix_matter) |word| if (std.mem.startsWith(u8, lower, word)) return true;
    // Untitled pages keep their file stem ("copy", "ded01", "bm01").
    var has_upper = false;
    for (title) |c| has_upper = has_upper or std.ascii.isUpper(c);
    if (std.mem.indexOfScalar(u8, title, ' ') == null and !has_upper) return true;
    return textSize(chapter.blocks) < min_body;
}

fn isPsalter(doc: document.Document) bool {
    var n: usize = 0;
    for (doc.chapters) |c| n += @intFromBool(std.mem.indexOf(u8, c.title, "Kathisma") != null);
    return n >= 10;
}

/// Chapters that count for plans, in reading order.
pub fn bodyChapters(allocator: std.mem.Allocator, doc: document.Document) ![]usize {
    var out: std.ArrayList(usize) = .empty;
    const psalter = isPsalter(doc);
    for (doc.chapters, 0..) |c, i| {
        const keep = if (psalter) std.mem.indexOf(u8, c.title, "Kathisma") != null else !isMatter(doc, i);
        if (keep) try out.append(allocator, i);
    }
    return out.items;
}

/// Name of the section unit for this book (shown in the plan builder).
pub fn sectionName(doc: document.Document) []const u8 {
    if (isPsalter(doc)) return "stases";
    for (doc.chapters, 0..) |c, i| if (!isMatter(doc, i) and sectionLevel(c.blocks) != null) return "sections";
    return "quarter chapters";
}

pub fn chapterName(doc: document.Document) []const u8 {
    return if (isPsalter(doc)) "kathismata" else "chapters";
}

/// First block after the chapter's leading title headings.
fn leadEnd(blocks: []const document.Block) usize {
    var i: usize = 0;
    while (i < blocks.len and blocks[i].kind == .heading) i += 1;
    return i;
}

/// Level of the chapter's top-level section headings, if it has any.
fn sectionLevel(blocks: []const document.Block) ?u8 {
    var level: ?u8 = null;
    for (blocks[leadEnd(blocks)..]) |b| if (b.kind == .heading and b.level > 0) {
        level = if (level) |l| @min(l, b.level) else b.level;
    };
    return level;
}

pub fn units(allocator: std.mem.Allocator, doc: document.Document, kind: Kind) ![]Unit {
    var out: std.ArrayList(Unit) = .empty;
    const psalter = isPsalter(doc);
    for (try bodyChapters(allocator, doc)) |ci| {
        const chapter = doc.chapters[ci];
        const blocks = chapter.blocks;
        if (kind == .chapter) {
            try out.append(allocator, .{ .chapter = ci, .start = 0, .end = blocks.len, .label = chapter.title });
            continue;
        }
        // Cut points (block indices where a new unit starts) and names.
        var cuts: std.ArrayList(usize) = .empty;
        var names: std.ArrayList([]const u8) = .empty;
        try cuts.append(allocator, 0);
        try names.append(allocator, chapter.title);
        if (psalter) {
            var stasis: usize = 1;
            for (blocks, 0..) |b, i| if (b.kind == .rubric and std.mem.startsWith(u8, b.text, "Glory") and i + 1 < blocks.len) {
                stasis += 1;
                try cuts.append(allocator, i + 1);
                try names.append(allocator, try std.fmt.allocPrint(allocator, "{s} · Stasis {d}", .{ chapter.title, stasis }));
            };
            if (names.items.len > 1) names.items[0] = try std.fmt.allocPrint(allocator, "{s} · Stasis 1", .{chapter.title});
        } else if (sectionLevel(blocks)) |level| {
            for (blocks[leadEnd(blocks)..], leadEnd(blocks)..) |b, i| if (b.kind == .heading and b.level == level) {
                try cuts.append(allocator, i);
                try names.append(allocator, try std.fmt.allocPrint(allocator, "{s} › {s}", .{ chapter.title, b.text }));
            };
        } else if (textSize(blocks) >= quarter_from) {
            const total = textSize(blocks);
            var acc: usize = 0;
            var part: usize = 1;
            for (blocks, 0..) |b, i| {
                if (part < 4 and acc >= total * part / 4 and b.kind != .heading) {
                    part += 1;
                    try cuts.append(allocator, i);
                    try names.append(allocator, try std.fmt.allocPrint(allocator, "{s} · part {d}/4", .{ chapter.title, part }));
                }
                if (b.kind != .heading and b.kind != .image) acc += b.text.len;
            }
            if (names.items.len > 1) names.items[0] = try std.fmt.allocPrint(allocator, "{s} · part 1/4", .{chapter.title});
        }
        // Emit, merging small sections forward (and a small tail backward).
        const first = out.items.len;
        var start: usize = 0;
        var name = names.items[0];
        for (cuts.items, 0..) |_, k| {
            const end = if (k + 1 < cuts.items.len) cuts.items[k + 1] else blocks.len;
            if (end <= start) continue;
            const merge_small = !psalter and textSize(blocks[start..end]) < min_section and k + 1 < cuts.items.len;
            if (merge_small) continue;
            try out.append(allocator, .{ .chapter = ci, .start = start, .end = end, .label = name });
            start = end;
            if (k + 1 < cuts.items.len) name = names.items[k + 1];
        }
        if (start < blocks.len) {
            if (out.items.len > first) out.items[out.items.len - 1].end = blocks.len else try out.append(allocator, .{ .chapter = ci, .start = start, .end = blocks.len, .label = name });
        }
        if (!psalter and out.items.len > first + 1 and textSize(blocks[out.items[out.items.len - 1].start..out.items[out.items.len - 1].end]) < min_section) {
            const tail = out.pop().?;
            out.items[out.items.len - 1].end = tail.end;
        }
    }
    return out.items;
}

test "sample books: matter skipped, kathismata/stases, sections, quarters" {
    const io = std.testing.io;
    var dir = std.Io.Dir.cwd().openDir(io, "examplePubs", .{}) catch return error.SkipZigTest;
    dir.close(io);
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const convert = @import("convert.zig");
    const Case = struct { file: []const u8, chapters: usize, first: []const u8, section_name: []const u8 };
    const cases = [_]Case{
        .{ .file = "The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub", .chapters = 20, .first = "The First Kathisma", .section_name = "stases" },
        .{ .file = "buildingmicroservices2ndedition.epub", .chapters = 17, .first = "Chapter 1. What Are Microservices?", .section_name = "sections" },
        .{ .file = "secretlifeofprograms.epub", .chapters = 15, .first = "1 THE INTERNAL LANGUAGE OF COMPUTERS", .section_name = "sections" },
        .{ .file = "thinklikeaprogrammer.epub", .chapters = 8, .first = "1 STRATEGIES FOR PROBLEM SOLVING", .section_name = "sections" },
    };
    for (cases) |case| {
        const bytes = try std.Io.Dir.cwd().readFileAlloc(io, try std.fs.path.join(a, &.{ "examplePubs", case.file }), a, .limited(128 << 20));
        const doc = (try convert.convert(a, bytes, case.file)).doc;
        const chapters = try units(a, doc, .chapter);
        std.testing.expectEqual(case.chapters, chapters.len) catch |err| {
            for (chapters) |u| std.debug.print("  {s}\n", .{u.label});
            return err;
        };
        try std.testing.expectEqualStrings(case.first, chapters[0].label);
        try std.testing.expectEqualStrings(case.section_name, sectionName(doc));
        const sections = try units(a, doc, .section);
        try std.testing.expect(sections.len > chapters.len);
        // Sections tile each body chapter exactly, in order.
        var at: usize = 0;
        for (chapters) |c| {
            var next: usize = c.start;
            while (at < sections.len and sections[at].chapter == c.chapter) : (at += 1) {
                try std.testing.expectEqual(next, sections[at].start);
                next = sections[at].end;
            }
            try std.testing.expectEqual(c.end, next);
        }
        try std.testing.expectEqual(sections.len, at);
        if (std.mem.eql(u8, case.section_name, "stases")) {
            try std.testing.expectEqual(@as(usize, 60), sections.len);
            try std.testing.expectEqualStrings("The First Kathisma · Stasis 2", sections[1].label);
        }
    }
}

test "chapters without section headings split into quarters" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    var blocks: std.ArrayList(document.Block) = .empty;
    try blocks.append(a, .{ .kind = .heading, .level = 1, .text = "Chapter One" });
    for (0..40) |_| try blocks.append(a, .{ .kind = .paragraph, .text = "x" ** 300 });
    const doc: document.Document = .{ .title = "Novel", .chapters = &.{
        .{ .title = "Copyright", .blocks = &.{.{ .kind = .paragraph, .text = "c" ** 3000 }} },
        .{ .title = "Chapter One", .blocks = blocks.items },
    } };
    const parts = try units(a, doc, .section);
    try std.testing.expectEqual(@as(usize, 4), parts.len);
    try std.testing.expectEqualStrings("Chapter One · part 1/4", parts[0].label);
    try std.testing.expectEqualStrings("quarter chapters", sectionName(doc));
    try std.testing.expectEqual(@as(usize, 1), (try units(a, doc, .chapter)).len);
}
