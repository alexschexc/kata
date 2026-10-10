//! EPUB content → Kata document blocks (ingest stage 3). Rules are generic
//! (semantic tags, EPUB/O'Reilly data-type, common class names) rather than
//! per-book: see docs/ingest-plan.md for the samples they were designed on.
const std = @import("std");
const xhtml = @import("xhtml.zig");
const epub = @import("epub.zig");
const document = @import("document.zig");
const Kind = document.Kind;

pub const version: u32 = 3;

pub const Stats = struct {
    chapters: usize = 0,
    blocks: usize = 0,
    verses: usize = 0,
    /// Explicit verse numbers that disagreed with the inferred count.
    verse_mismatches: usize = 0,
    pages: usize = 0,
    figures: usize = 0,
    code: usize = 0,
    footnotes: usize = 0,
    images: usize = 0,
    /// Spine documents with no readable text (cover, TOC, index).
    skipped: usize = 0,
};

/// An image copied out of the EPUB, stored as `<document>.assets/<name>`.
pub const Asset = struct { name: []const u8, bytes: []const u8 };

pub const Result = struct { doc: document.Document, stats: Stats, assets: []const Asset = &.{} };

/// Zip member path for `href` relative to content document `member`.
fn resolveMember(allocator: std.mem.Allocator, member: []const u8, href: []const u8) !?[]const u8 {
    if (std.mem.indexOf(u8, href, "://") != null or std.mem.startsWith(u8, href, "data:")) return null;
    const clean = href[0 .. std.mem.indexOfAny(u8, href, "#?") orelse href.len];
    var parts: std.ArrayList([]const u8) = .empty;
    if (!std.mem.startsWith(u8, clean, "/")) if (std.fs.path.dirnamePosix(member)) |dir| {
        var it = std.mem.splitScalar(u8, dir, '/');
        while (it.next()) |part| if (part.len > 0) try parts.append(allocator, part);
    };
    var it = std.mem.splitScalar(u8, clean, '/');
    while (it.next()) |part| {
        if (part.len == 0 or std.mem.eql(u8, part, ".")) continue;
        if (std.mem.eql(u8, part, "..")) {
            _ = parts.pop();
            continue;
        }
        try parts.append(allocator, part);
    }
    if (parts.items.len == 0) return null;
    return try std.mem.join(allocator, "/", parts.items);
}

/// Whole subtrees that carry no reading text in a terminal.
fn skipped(tag: xhtml.Tag) bool {
    const name = tag.local();
    for ([_][]const u8{ "head", "script", "style", "svg", "math", "nav", "template", "noscript" }) |skip| {
        if (std.ascii.eqlIgnoreCase(name, skip)) return true;
    }
    for ([_][]const u8{ "data-type", "epub:type" }) |attr| {
        for ([_][]const u8{ "index", "toc", "landmarks", "page-list" }) |value| if (tag.hasToken(attr, value)) return true;
    }
    if (tag.hasToken("class", "ornament") or tag.hasToken("class", "tailpiece-art")) return true;
    if (tag.hasToken("data-type", "indexterm")) return true;
    return false;
}

fn isPageBreak(tag: xhtml.Tag) bool {
    return tag.hasToken("epub:type", "pagebreak") or tag.hasToken("role", "doc-pagebreak");
}

const containers = [_][]const u8{ "body", "div", "section", "article", "main", "header", "footer", "aside", "blockquote", "ul", "ol", "dl", "table", "thead", "tbody", "tfoot", "figure", "hgroup" };

fn isContainer(name: []const u8) bool {
    for (containers) |c| if (std.ascii.eqlIgnoreCase(name, c)) return true;
    return false;
}

fn quoteContainer(tag: xhtml.Tag) bool {
    const name = tag.local();
    if (std.ascii.eqlIgnoreCase(name, "blockquote")) return true;
    if (std.ascii.eqlIgnoreCase(name, "aside") and !tag.hasToken("epub:type", "footnote")) return true;
    for ([_][]const u8{ "sidebar", "note", "tip", "warning", "caution", "important", "epigraph" }) |kind| {
        if (tag.hasToken("data-type", kind)) return true;
    }
    return false;
}

const liturgical = [_][]const u8{ "rubric", "stasis", "refrain", "response", "chant", "liturgical" };

const Frame = struct {
    name: []const u8,
    /// This element opened a block.
    block: bool = false,
    kind: Kind = .paragraph,
    level: u8 = 0,
    verse: u32 = 0,
    container: bool = false,
    quote: bool = false,
    pre: bool = false,
    hide: bool = false,
    number: bool = false,
    sup: bool = false,
    figure: bool = false,
};

const Builder = struct {
    allocator: std.mem.Allocator,
    stats: *Stats,
    blocks: std.ArrayList(document.Block) = .empty,
    frames: std.ArrayList(Frame) = .empty,
    buf: std.ArrayList(u8) = .empty,
    number_buf: std.ArrayList(u8) = .empty,
    figure_alt: []const u8 = "",
    page: *[]const u8,
    buf_page: []const u8 = "",
    quote: usize = 0,
    pre: usize = 0,
    hide: usize = 0,
    number: usize = 0,
    verse_counter: u32 = 0,
    member: []const u8 = "",
    archive: ?*const epub.Archive = null,
    images: ?*std.ArrayList([]const u8) = null,

    /// Emits an image block and records the zip member to copy out.
    fn image(self: *Builder, src: []const u8) !void {
        const path = try resolveMember(self.allocator, self.member, src) orelse return;
        const archive = self.archive orelse return; // unit tests: no assets
        if (archive.find(path) == null) return;
        try self.flush();
        const name = try std.mem.replaceOwned(u8, self.allocator, path, "/", "_");
        try self.emit(.image, 0, 0, name);
        if (self.images) |list| {
            for (list.items) |known| if (std.mem.eql(u8, known, path)) return;
            try list.append(self.allocator, path);
        }
    }

    fn innermostBlock(self: *Builder) ?*Frame {
        var i = self.frames.items.len;
        while (i > 0) {
            i -= 1;
            if (self.frames.items[i].block) return &self.frames.items[i];
        }
        return null;
    }

    fn flush(self: *Builder) !void {
        defer self.buf.clearRetainingCapacity();
        const frame = self.innermostBlock();
        const kind: Kind = if (frame) |f| f.kind else if (self.quote > 0) .quote else .paragraph;
        const raw = self.buf.items;
        const body = if (kind == .code) std.mem.trim(u8, raw, "\n") else std.mem.trim(u8, raw, " \n");
        if (body.len == 0) {
            if (kind == .figure and frame != null and self.figure_alt.len > 0) {
                try self.emit(.figure, 0, 0, try std.fmt.allocPrint(self.allocator, "[image: {s}]", .{self.figure_alt}));
                self.figure_alt = "";
            }
            return;
        }
        try self.emit(kind, if (frame) |f| f.level else 0, if (frame) |f| f.verse else 0, try self.allocator.dupe(u8, body));
        if (kind == .figure) self.figure_alt = "";
    }

    fn emit(self: *Builder, kind: Kind, level: u8, verse: u32, text: []const u8) !void {
        try self.blocks.append(self.allocator, .{ .kind = kind, .level = level, .verse = verse, .page = if (self.buf_page.len > 0) self.buf_page else self.page.*, .text = text });
        self.buf_page = "";
        switch (kind) {
            .verse => self.stats.verses += 1,
            .figure => self.stats.figures += 1,
            .code => self.stats.code += 1,
            .footnote => self.stats.footnotes += 1,
            .image => self.stats.images += 1,
            else => {},
        }
    }

    fn appendCodepoint(self: *Builder, cp: u21) !void {
        var c = cp;
        if (c == 0xA0) c = ' ';
        if (c == 0xAD or c == 0xFEFF or c == 0x7F or (c >= 0x80 and c <= 0x9F)) return;
        if (self.number > 0) {
            if (c < 0x80 and std.ascii.isDigit(@intCast(c))) try self.number_buf.append(self.allocator, @intCast(c));
            return;
        }
        if (self.hide > 0) return;
        if (self.pre > 0) {
            if (c == '\r') return;
            if (c < 0x20 and c != '\n' and c != '\t') return;
        } else {
            if (c == '\n' or c == '\t' or c == '\r' or c == ' ' or c == 0x0C) {
                if (self.buf.items.len > 0) {
                    const last = self.buf.items[self.buf.items.len - 1];
                    if (last != ' ' and last != '\n') try self.buf.append(self.allocator, ' ');
                }
                return;
            }
            if (c < 0x20) return;
        }
        if (self.buf.items.len == 0 or std.mem.trim(u8, self.buf.items, " \n").len == 0) self.buf_page = self.page.*;
        var bytes: [4]u8 = undefined;
        const n = std.unicode.utf8Encode(c, &bytes) catch return;
        try self.buf.appendSlice(self.allocator, bytes[0..n]);
    }

    fn addText(self: *Builder, raw: []const u8, decode: bool) !void {
        var i: usize = 0;
        while (i < raw.len) {
            if (decode and raw[i] == '&') {
                if (xhtml.entityAt(raw, i)) |entity| {
                    try self.appendCodepoint(entity.cp);
                    i += entity.len;
                    continue;
                }
            }
            const len = std.unicode.utf8ByteSequenceLength(raw[i]) catch {
                i += 1;
                continue;
            };
            if (i + len > raw.len) break;
            const cp = std.unicode.utf8Decode(raw[i..][0..len]) catch {
                i += len;
                continue;
            };
            try self.appendCodepoint(cp);
            i += len;
        }
    }

    fn lineBreak(self: *Builder) !void {
        if (self.hide > 0 or self.number > 0) return;
        // Headings render on one line: a break is just a space there.
        if (self.innermostBlock()) |f| if (f.kind == .heading) return self.appendCodepoint(' ');
        while (self.buf.items.len > 0 and self.buf.items[self.buf.items.len - 1] == ' ') self.buf.items.len -= 1;
        if (self.buf.items.len > 0) try self.buf.append(self.allocator, '\n');
    }

    /// Block kind for a block-level element, or null if it is not one.
    fn blockKind(self: *Builder, tag: xhtml.Tag) ?struct { kind: Kind, level: u8 = 0 } {
        const name = tag.local();
        if (name.len == 2 and (name[0] == 'h' or name[0] == 'H') and name[1] >= '1' and name[1] <= '6') return .{ .kind = .heading, .level = name[1] - '0' };
        if (std.ascii.eqlIgnoreCase(name, "pre")) return .{ .kind = .code };
        if (std.ascii.eqlIgnoreCase(name, "li")) return .{ .kind = .list_item };
        if (std.ascii.eqlIgnoreCase(name, "dt")) return .{ .kind = .term };
        if (std.ascii.eqlIgnoreCase(name, "tr")) return .{ .kind = .table_row };
        if (std.ascii.eqlIgnoreCase(name, "figure")) return .{ .kind = .figure };
        if (tag.hasToken("data-type", "footnote") or tag.hasToken("epub:type", "footnote")) return .{ .kind = .footnote };
        const paragraph_like = std.ascii.eqlIgnoreCase(name, "p") or std.ascii.eqlIgnoreCase(name, "dd") or std.ascii.eqlIgnoreCase(name, "caption") or std.ascii.eqlIgnoreCase(name, "figcaption");
        if (!paragraph_like) return null;
        if (tag.hasToken("class", "verse")) return .{ .kind = .verse };
        if (tag.hasToken("class", "superscription")) return .{ .kind = .superscription };
        for (liturgical) |class| if (tag.hasToken("class", class)) return .{ .kind = .rubric };
        if (self.quote > 0) return .{ .kind = .quote };
        return .{ .kind = .paragraph };
    }

    fn start(self: *Builder, tag: xhtml.Tag) !void {
        const name = tag.local();
        if (std.ascii.eqlIgnoreCase(name, "br")) return self.lineBreak();
        if (std.ascii.eqlIgnoreCase(name, "img")) {
            const outer = self.innermostBlock();
            // Figures and free-standing images; inline icons stay out.
            if (outer == null or outer.?.kind == .figure) if (tag.attr("src")) |src| try self.image(src);
            if (outer) |f| if (f.kind == .figure) {
                self.figure_alt = tag.attr("alt") orelse "";
            };
            return;
        }
        if (std.ascii.eqlIgnoreCase(name, "hr")) return;
        var frame: Frame = .{ .name = name };
        if (isPageBreak(tag)) {
            if (tag.attr("aria-label") orelse tag.attr("title")) |label| {
                self.page.* = label;
                self.stats.pages += 1;
            }
            frame.hide = true;
        } else if (tag.hasToken("class", "verse-number")) {
            frame.number = true;
            self.number_buf.clearRetainingCapacity();
        } else if (std.ascii.eqlIgnoreCase(name, "sup")) {
            frame.sup = true;
            try self.appendCodepoint('[');
        } else if (std.ascii.eqlIgnoreCase(name, "td") or std.ascii.eqlIgnoreCase(name, "th")) {
            if (std.mem.trim(u8, self.buf.items, " ").len > 0) try self.addText(" │ ", false);
        } else if (self.blockKind(tag)) |info| {
            const outer = self.innermostBlock();
            const inline_in_outer = outer != null and switch (outer.?.kind) {
                .list_item, .table_row, .term, .footnote, .figure => info.kind != .list_item and info.kind != .table_row and info.kind != .code and info.kind != .figure,
                else => false,
            };
            if (inline_in_outer) {
                try self.appendCodepoint(' ');
            } else {
                try self.flush();
                frame.block = true;
                frame.kind = info.kind;
                frame.level = info.level;
                if (info.kind == .heading) self.verse_counter = 0;
                if (info.kind == .verse) {
                    self.verse_counter += 1;
                    frame.verse = self.verse_counter;
                }
                if (info.kind == .code) frame.pre = true;
                if (info.kind == .figure) {
                    self.figure_alt = "";
                    frame.container = true;
                }
            }
        } else if (isContainer(name)) {
            try self.flush();
            frame.container = true;
            frame.quote = quoteContainer(tag);
        }
        if (tag.self_closing) return;
        if (frame.quote) self.quote += 1;
        if (frame.pre) self.pre += 1;
        if (frame.hide) self.hide += 1;
        if (frame.number) self.number += 1;
        try self.frames.append(self.allocator, frame);
    }

    fn end(self: *Builder, raw_name: []const u8) !void {
        const name = xhtml.localName(raw_name);
        // Tolerate unclosed inner elements: pop to the nearest match.
        var i = self.frames.items.len;
        const match = while (i > 0) {
            i -= 1;
            if (std.ascii.eqlIgnoreCase(self.frames.items[i].name, name)) break i;
        } else return;
        while (self.frames.items.len > match) {
            const frame = self.frames.items[self.frames.items.len - 1];
            if (frame.block or frame.container) try self.flush();
            if (frame.sup) {
                if (self.buf.items.len > 0 and self.buf.items[self.buf.items.len - 1] == '[') self.buf.items.len -= 1 else try self.appendCodepoint(']');
            }
            if (frame.number) {
                self.number -= 1;
                if (std.fmt.parseInt(u32, self.number_buf.items, 10)) |explicit| {
                    if (self.innermostBlock()) |block| if (block.kind == .verse and block.verse != explicit) {
                        self.stats.verse_mismatches += 1;
                        block.verse = explicit;
                        self.verse_counter = explicit;
                    };
                } else |_| {}
            }
            if (frame.quote) self.quote -= 1;
            if (frame.pre) self.pre -= 1;
            if (frame.hide) self.hide -= 1;
            self.frames.items.len -= 1;
        }
    }
};

/// Converts one XHTML content document. Returns null when it has no text.
pub fn chapter(allocator: std.mem.Allocator, source: []const u8, page: *[]const u8, stats: *Stats) !?document.Chapter {
    return chapterIn(allocator, source, page, stats, .{});
}

pub const Context = struct { member: []const u8 = "", archive: ?*const epub.Archive = null, images: ?*std.ArrayList([]const u8) = null };

pub fn chapterIn(allocator: std.mem.Allocator, source: []const u8, page: *[]const u8, stats: *Stats, context: Context) !?document.Chapter {
    var builder: Builder = .{ .allocator = allocator, .stats = stats, .page = page, .member = context.member, .archive = context.archive, .images = context.images };
    var tokens: xhtml.Tokenizer = .{ .src = source };
    var skip_name: ?[]const u8 = null;
    var skip_depth: usize = 0;
    while (tokens.next()) |token| {
        if (skip_name) |skip| {
            switch (token) {
                .start => |tag| if (!tag.self_closing and std.ascii.eqlIgnoreCase(tag.local(), skip)) {
                    skip_depth += 1;
                },
                .end => |name| if (std.ascii.eqlIgnoreCase(xhtml.localName(name), skip)) {
                    skip_depth -= 1;
                    if (skip_depth == 0) skip_name = null;
                },
                else => {},
            }
            continue;
        }
        switch (token) {
            .text => |raw| try builder.addText(raw, true),
            .cdata => |raw| try builder.addText(raw, false),
            .start => |tag| {
                if (skipped(tag)) {
                    if (!tag.self_closing) {
                        skip_name = tag.local();
                        skip_depth = 1;
                    }
                    continue;
                }
                try builder.start(tag);
            },
            .end => |name| try builder.end(name),
        }
    }
    try builder.flush();
    if (builder.blocks.items.len == 0) return null;
    var title: []const u8 = "";
    for (builder.blocks.items) |block| if (block.kind == .heading) {
        title = block.text;
        break;
    };
    return .{ .title = title, .blocks = try builder.blocks.toOwnedSlice(allocator) };
}

/// Converts a whole EPUB held in memory. Strings borrow allocator memory.
pub fn convert(allocator: std.mem.Allocator, bytes: []const u8, source_name: []const u8) !Result {
    const archive = try epub.Archive.init(allocator, bytes);
    const pkg = try epub.package(allocator, archive);
    var stats: Stats = .{};
    var chapters: std.ArrayList(document.Chapter) = .empty;
    var page: []const u8 = "";
    var images: std.ArrayList([]const u8) = .empty;
    for (pkg.spine) |member| {
        const content = try archive.read(allocator, member);
        if (try chapterIn(allocator, content, &page, &stats, .{ .member = member, .archive = &archive, .images = &images })) |converted| {
            var c = converted;
            if (c.title.len == 0) {
                const base = std.fs.path.basenamePosix(member);
                c.title = base[0 .. std.mem.lastIndexOfScalar(u8, base, '.') orelse base.len];
            }
            stats.blocks += c.blocks.len;
            try chapters.append(allocator, c);
        } else stats.skipped += 1;
    }
    stats.chapters = chapters.items.len;
    if (chapters.items.len == 0) return error.NoReadableText;
    var assets: std.ArrayList(Asset) = .empty;
    for (images.items) |path| {
        const data = archive.read(allocator, path) catch continue;
        try assets.append(allocator, .{ .name = try std.mem.replaceOwned(u8, allocator, path, "/", "_"), .bytes = data });
    }
    return .{
        .assets = assets.items,
        .doc = .{ .title = if (pkg.title.len > 0) pkg.title else source_name, .author = pkg.author, .source = source_name, .converter = version, .chapters = chapters.items },
        .stats = stats,
    };
}

fn testChapter(a: std.mem.Allocator, source: []const u8, stats: *Stats) !document.Chapter {
    var page: []const u8 = "";
    return (try chapter(a, source, &page, stats)).?;
}

test "verses are inferred, checked against explicit numbers, and reset at headings" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var stats: Stats = .{};
    const c = try testChapter(arena.allocator(),
        \\<body><h2>Psalm I. 1</h2><p class="superscription">David's.</p>
        \\<p class="verse opening"><span class="dropcap">B</span>lessed is the man</p>
        \\<p class="verse">second</p><p class="verse"><span class="verse-number">3</span>Third &amp; last</p>
        \\<span epub:type="pagebreak" role="doc-pagebreak" aria-label="26"></span>
        \\<h2>Psalm II. 2</h2><p class="verse">Why</p><p class="stasis">Glory.</p></body>
    , &stats);
    const b = c.blocks;
    try std.testing.expectEqualStrings("Psalm I. 1", c.title);
    try std.testing.expectEqual(Kind.superscription, b[1].kind);
    try std.testing.expectEqualStrings("Blessed is the man", b[2].text);
    try std.testing.expectEqual(@as(u32, 1), b[2].verse);
    try std.testing.expectEqualStrings("Third & last", b[4].text);
    try std.testing.expectEqual(@as(u32, 3), b[4].verse);
    try std.testing.expectEqual(@as(usize, 0), stats.verse_mismatches);
    try std.testing.expectEqualStrings("26", b[5].page);
    try std.testing.expectEqual(@as(u32, 1), b[6].verse);
    try std.testing.expectEqual(Kind.rubric, b[7].kind);
}

test "code keeps whitespace; figures, notes, lists, tables, and noise are handled" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    var stats: Stats = .{};
    const c = try testChapter(arena.allocator(),
        \\<html><head><title>x</title></head><body><section data-type="chapter"><h1><span class="label">Chapter 4. </span>Styles</h1>
        \\<p>Fire.<sup><a data-type="noteref" href="#n1">1</a></sup><a data-type="indexterm" id="x"/> More
        \\ text.</p><pre data-type="programlisting"><code class="k">if</code><code> x {
        \\    y();
        \\}</code></pre>
        \\<figure><div class="figure"><img src="a.png" alt="bms2"/><h6><span class="label">Figure 4-9. </span>Caption</h6></div></figure>
        \\<figure><img src="b.png" alt="diagram"/></figure>
        \\<aside data-type="sidebar"><h5>Side</h5><p>Aside text</p></aside>
        \\<ul><li><p>one</p></li><li>two</li></ul><table><tr><td><p>a</p></td><td>b</td></tr></table>
        \\<p class="liturgical">Line one<br/>Line two</p><nav epub:type="toc"><p>skip</p></nav><p class="ornament">✠</p>
        \\</section><div data-type="footnotes"><p data-type="footnote"><sup><a>1</a></sup> True story.</p></div></body></html>
    , &stats);
    const b = c.blocks;
    try std.testing.expectEqualStrings("Chapter 4. Styles", c.title);
    try std.testing.expectEqualStrings("Fire.[1] More text.", b[1].text);
    try std.testing.expectEqual(Kind.code, b[2].kind);
    try std.testing.expectEqualStrings("if x {\n    y();\n}", b[2].text);
    try std.testing.expectEqual(Kind.figure, b[3].kind);
    try std.testing.expectEqualStrings("Figure 4-9. Caption", b[3].text);
    try std.testing.expectEqualStrings("[image: diagram]", b[4].text);
    try std.testing.expectEqual(Kind.heading, b[5].kind);
    try std.testing.expectEqual(Kind.quote, b[6].kind);
    try std.testing.expectEqualStrings("one", b[7].text);
    try std.testing.expectEqual(Kind.list_item, b[8].kind);
    try std.testing.expectEqualStrings("a │ b", b[9].text);
    try std.testing.expectEqualStrings("Line one\nLine two", b[10].text);
    try std.testing.expectEqual(Kind.footnote, b[11].kind);
    try std.testing.expectEqualStrings("[1] True story.", b[11].text);
    try std.testing.expectEqual(@as(usize, 12), b.len);
}

test "both sample EPUBs convert with expected structure" {
    const io = std.testing.io;
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    const psalter_path = "examplePubs/The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub";
    const psalter_bytes = std.Io.Dir.cwd().readFileAlloc(io, psalter_path, a, .limited(64 << 20)) catch |err| switch (err) {
        error.FileNotFound => return error.SkipZigTest,
        else => return err,
    };
    const psalter = try convert(a, psalter_bytes, "psalter.epub");
    try std.testing.expectEqual(@as(usize, 0), psalter.stats.verse_mismatches);
    try std.testing.expect(psalter.stats.pages > 200);
    var psalm_1: usize = 0;
    var psalm_118: u32 = 0;
    var heading: []const u8 = "";
    for (psalter.doc.chapters) |c| for (c.blocks) |block| {
        if (block.kind == .heading) heading = block.text;
        if (block.kind == .verse and std.mem.eql(u8, heading, "Psalm I. 1")) psalm_1 += 1;
        if (block.kind == .verse and std.mem.endsWith(u8, heading, " 118")) psalm_118 = @max(psalm_118, block.verse);
        try std.testing.expect(std.mem.indexOf(u8, block.text, "<") == null or block.kind == .code);
    };
    try std.testing.expectEqual(@as(usize, 6), psalm_1);
    try std.testing.expectEqual(@as(u32, 176), psalm_118);

    const bms_bytes = try std.Io.Dir.cwd().readFileAlloc(io, "examplePubs/buildingmicroservices2ndedition.epub", a, .limited(64 << 20));
    const bms = try convert(a, bms_bytes, "bms.epub");
    try std.testing.expectEqual(@as(usize, 15), bms.stats.code);
    try std.testing.expect(bms.stats.footnotes >= 140);
    try std.testing.expect(bms.stats.figures >= 180);
    var found_figure = false;
    for (bms.doc.chapters) |c| {
        try std.testing.expect(!std.mem.eql(u8, c.title, "Index"));
        for (c.blocks) |block| {
            if (block.kind == .figure and std.mem.startsWith(u8, block.text, "Figure 4-9. ")) found_figure = true;
            for (block.text) |ch| try std.testing.expect(ch >= 0x20 or ch == '\n' or ch == '\t');
        }
    }
    try std.testing.expect(found_figure);
    try std.testing.expect(bms.stats.images >= 180);
    try std.testing.expect(bms.assets.len >= 175 and bms.assets.len <= bms.stats.images); // shared images copied once
    try std.testing.expect(std.mem.startsWith(u8, bms.assets[1].bytes, "\x89PNG"));
}

test "image paths resolve relative to their content document" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const a = arena.allocator();
    try std.testing.expectEqualStrings("OEBPS/images/c.jpg", (try resolveMember(a, "OEBPS/text/x.xhtml", "../images/c.jpg")).?);
    try std.testing.expectEqualStrings("OEBPS/assets/b.png", (try resolveMember(a, "OEBPS/ch4.html", "assets/b.png#x")).?);
    try std.testing.expect(try resolveMember(a, "x.html", "https://e.com/a.png") == null);
}
