//! Single-pane reader for ingested library documents (see ingest.zig).
//! Chapters come from the document; `H`/`L` (Shift+←/→) move between them, `/` searches
//! the whole document with the same matching and results panel behaviour as
//! the Bible reader. Positions are saved per document.
const std = @import("std");
const document = @import("document.zig");
const layout = @import("layout.zig");
const search = @import("search.zig");
const tui = @import("tui.zig");
const inline_image = @import("inline_image.zig");
const graphics = @import("graphics.zig");

const paper = "\x1b[48;2;21;25;34m";
const ink = "\x1b[38;2;224;215;191m";
const dim = "\x1b[38;2;134;145;156m";
const gold = "\x1b[38;2;210;178;116m";
const code_ink = "\x1b[38;2;168;196;160m";
const rule = "\x1b[38;2;74;87;101m";
const gutter_width = 6;

/// Saved position for one document (`<state>.books/<file>.json`).
pub const Position = struct { version: u8 = 1, chapter: usize = 0, block: usize = 0 };

/// A plan portion being read: an end-of-reading marker is drawn before
/// block `end` of chapter `chapter` (end == blocks.len: after the chapter).
pub const Portion = struct { chapter: usize, end: usize, label: []const u8 };

/// One rendered line: text is a slice of `full` (the block's display text),
/// so search highlights work across wrapped lines.
const Line = struct {
    block: usize,
    text: []const u8,
    full: []const u8,
    gutter: []const u8 = "",
    style: Style = .body,
    /// For inline images: asset name and which reserved row this line is.
    image: []const u8 = "",
    image_row: u16 = 0,

    const Style = enum { body, heading, code, quote, note, figure, page };
};

fn styleFor(kind: document.Kind) Line.Style {
    return switch (kind) {
        .heading => .heading,
        .code => .code,
        .quote, .superscription, .rubric => .quote,
        .footnote => .note,
        .figure, .image => .figure,
        else => .body,
    };
}

fn wrapCode(allocator: std.mem.Allocator, text: []const u8, width: usize, out: *std.ArrayList([]const u8)) !void {
    var rows = std.mem.splitScalar(u8, text, '\n');
    while (rows.next()) |row| {
        if (row.len == 0) {
            try out.append(allocator, row);
            continue;
        }
        // Code keeps its spacing; long lines are hard-wrapped, not reflowed.
        var start: usize = 0;
        while (start < row.len) {
            var end = start;
            var used: usize = 0;
            while (end < row.len) {
                const n = std.unicode.utf8ByteSequenceLength(row[end]) catch 1;
                const w = layout.displayWidth(row[end..@min(row.len, end + n)]);
                if (used + w > width and end > start) break;
                used += w;
                end = @min(row.len, end + n);
            }
            try out.append(allocator, row[start..end]);
            start = end;
        }
    }
}

/// Lays out one chapter at `width` columns (text column excludes the gutter).
fn render(allocator: std.mem.Allocator, chapter: document.Chapter, width: usize) ![]Line {
    return renderWith(allocator, chapter, width, null, 0, null);
}

/// `images` (when inline images are enabled) reserves rows for each image
/// block instead of the placeholder line; `max_rows` caps image height.
fn renderWith(allocator: std.mem.Allocator, chapter: document.Chapter, width: usize, images: ?*inline_image.Renderer, max_rows: usize, stop: ?usize) ![]Line {
    var lines: std.ArrayList(Line) = .empty;
    var page: []const u8 = "";
    const stacked = try stackedLines(allocator, chapter.blocks);
    for (chapter.blocks, 0..) |block, index| {
        if (stop != null and stop.? == index) try appendStop(allocator, &lines, index);
        if (block.page.len > 0 and !std.mem.eql(u8, block.page, page)) {
            if (index > 0) {
                const label = try std.fmt.allocPrint(allocator, "── page {s} ──", .{block.page});
                try lines.append(allocator, .{ .block = index, .text = label, .full = label, .style = .page });
            }
            page = block.page;
        }
        if (index > 0 and block.kind.spaced() and !stacked[index]) try lines.append(allocator, .{ .block = index, .text = "", .full = "" });
        const text = switch (block.kind) {
            .image => try std.fmt.allocPrint(allocator, "▣ image · i opens it  ({s})", .{block.text}),
            .list_item => try std.fmt.allocPrint(allocator, "• {s}", .{block.text}),
            .table_row => try std.fmt.allocPrint(allocator, "  {s}", .{block.text}),
            .heading => if (block.level <= 1) try std.fmt.allocPrint(allocator, "{s}", .{block.text}) else block.text,
            else => block.text,
        };
        if (block.kind == .table_row) {
            // The first row of a run lays out the whole table as a grid.
            if (index > 0 and chapter.blocks[index - 1].kind == .table_row) continue;
            var end = index;
            while (end < chapter.blocks.len and chapter.blocks[end].kind == .table_row) end += 1;
            if (try appendTable(allocator, &lines, chapter.blocks, index, end, width)) continue;
            // Too narrow for a grid: rows fall through to flattened text.
            for (chapter.blocks[index..end], index..) |row, at| {
                const flat = try std.fmt.allocPrint(allocator, "  {s}", .{row.text});
                for (try layout.wrap(allocator, flat, width)) |piece| try lines.append(allocator, .{ .block = at, .text = piece, .full = flat });
            }
            continue;
        }
        if (block.kind == .image) if (images) |renderer| if (renderer.rows(block.text, width, max_rows)) |reserved| {
            for (0..reserved) |r| try lines.append(allocator, .{ .block = index, .text = "", .full = "", .style = .figure, .image = block.text, .image_row = @intCast(r) });
            continue;
        };
        const gutter = if (block.verse > 0) try std.fmt.allocPrint(allocator, "{d}", .{block.verse}) else "";
        var wrapped: std.ArrayList([]const u8) = .empty;
        if (block.kind == .code) {
            try wrapCode(allocator, text, width -| 2, &wrapped);
        } else {
            var paragraphs = std.mem.splitScalar(u8, text, '\n');
            while (paragraphs.next()) |part| try wrapped.appendSlice(allocator, try layout.wrap(allocator, part, width));
        }
        for (wrapped.items, 0..) |piece, i| {
            try lines.append(allocator, .{ .block = index, .text = piece, .full = text, .gutter = if (i == 0) gutter else "", .style = styleFor(block.kind) });
        }
    }
    if (stop != null and stop.? >= chapter.blocks.len) try appendStop(allocator, &lines, chapter.blocks.len -| 1);
    return lines.toOwnedSlice(allocator);
}

const stop_text = "━━━━━━ end of today's reading · m returns to the plan ━━━━━━";

fn appendStop(allocator: std.mem.Allocator, lines: *std.ArrayList(Line), block: usize) !void {
    try lines.append(allocator, .{ .block = block, .text = "", .full = "" });
    try lines.append(allocator, .{ .block = block, .text = stop_text, .full = stop_text, .style = .heading });
    try lines.append(allocator, .{ .block = block, .text = "", .full = "" });
}

const cell_separator = " │ ";

/// Lays out table rows [start, end) (cells joined by " │ " at conversion)
/// as aligned columns within `width`, wrapping long cells inside their
/// column. Returns false when the columns cannot fit (caller flattens).
fn appendTable(allocator: std.mem.Allocator, lines: *std.ArrayList(Line), blocks: []const document.Block, start: usize, end: usize, width: usize) !bool {
    const indent = 2;
    const min_column = 4;
    var rows: std.ArrayList([][]const u8) = .empty;
    var columns: usize = 0;
    for (blocks[start..end]) |row| {
        var cells: std.ArrayList([]const u8) = .empty;
        // Split on the bar alone: an empty last cell leaves a bare " │".
        var it = std.mem.splitSequence(u8, row.text, "│");
        while (it.next()) |cell| try cells.append(allocator, std.mem.trim(u8, cell, " "));
        columns = @max(columns, cells.items.len);
        try rows.append(allocator, try cells.toOwnedSlice(allocator));
    }
    const separators = (columns - 1) * cell_separator.len;
    if (columns == 0 or indent + separators + columns * min_column > width) return false;
    const natural = try allocator.alloc(usize, columns);
    @memset(natural, 1);
    for (rows.items) |cells| for (cells, 0..) |cell, c| {
        natural[c] = @max(natural[c], layout.displayWidth(cell));
    };
    // Water-fill: columns narrower than an equal share keep their natural
    // width; the rest split what remains evenly.
    const widths = try allocator.alloc(usize, columns);
    const fixed = try allocator.alloc(bool, columns);
    @memset(fixed, false);
    var available = width - indent - separators;
    var open = columns;
    var changed = true;
    while (changed and open > 0) {
        changed = false;
        const share = available / open;
        for (natural, 0..) |n, c| if (!fixed[c] and n <= share) {
            widths[c] = n;
            fixed[c] = true;
            available -= n;
            open -= 1;
            changed = true;
        };
    }
    if (open > 0) {
        const share = available / open;
        if (share < min_column) return false;
        var extra = available - share * open;
        for (fixed, 0..) |f, c| if (!f) {
            widths[c] = share + @intFromBool(extra > 0);
            extra -|= 1;
        };
    }
    // A cell narrower than min_column (or than wrap's 2-cell minimum)
    // would be unreadable: flatten instead.
    for (widths, natural) |w, n| if (w < @min(n, min_column) or w < 2 and n > 1) return false;
    var wrapped_any = false;
    var cell_lines: std.ArrayList([][][]const u8) = .empty;
    for (rows.items) |cells| {
        const per = try allocator.alloc([][]const u8, columns);
        for (per, 0..) |*out, c| {
            const cell = if (c < cells.len) cells[c] else "";
            out.* = if (cell.len == 0) &.{} else layout.wrap(allocator, cell, @max(2, widths[c])) catch return false;
            if (out.len > 1) wrapped_any = true;
        }
        try cell_lines.append(allocator, per);
    }
    var table_width: usize = separators;
    for (widths) |w| table_width += w;
    for (cell_lines.items, 0..) |per, r| {
        // Wrapped tables get a rule between rows so cells stay grouped.
        if (r > 0 and wrapped_any) {
            const rule_line = try allocator.alloc(u8, indent + table_width * "─".len);
            @memset(rule_line[0..indent], ' ');
            for (0..table_width) |i| @memcpy(rule_line[indent + i * "─".len ..][0.."─".len], "─");
            try lines.append(allocator, .{ .block = start + r, .text = rule_line, .full = rule_line, .style = .page });
        }
        var height: usize = 1;
        for (per) |cell| height = @max(height, cell.len);
        for (0..height) |line_index| {
            var text: std.ArrayList(u8) = .empty;
            try text.appendNTimes(allocator, ' ', indent);
            for (per, 0..) |cell, c| {
                if (c > 0) try text.appendSlice(allocator, cell_separator);
                const piece = if (line_index < cell.len) cell[line_index] else "";
                try text.appendSlice(allocator, piece);
                if (c + 1 < columns) try text.appendNTimes(allocator, ' ', widths[c] -| layout.displayWidth(piece));
            }
            const owned = try text.toOwnedSlice(allocator);
            const trimmed = std.mem.trimEnd(u8, owned, " ");
            try lines.append(allocator, .{ .block = start + r, .text = trimmed, .full = trimmed });
        }
    }
    return true;
}

/// Title pages, dedications, and contents lists often use one `<p>` per
/// line. In runs of 3+ short paragraphs, true marks the blocks that follow
/// another in the run: they are drawn without the blank spacer line.
fn stackedLines(allocator: std.mem.Allocator, blocks: []const document.Block) ![]bool {
    const short_limit = 48;
    const stacked = try allocator.alloc(bool, blocks.len);
    @memset(stacked, false);
    var start: usize = 0;
    var index: usize = 0;
    while (index <= blocks.len) : (index += 1) {
        const short = index < blocks.len and blocks[index].kind == .paragraph and blocks[index].verse == 0 and
            std.mem.indexOfScalar(u8, blocks[index].text, '\n') == null and layout.displayWidth(blocks[index].text) <= short_limit and
            (index == start or blocks[index].page.len == 0);
        if (short) continue;
        if (index - start >= 3) @memset(stacked[start + 1 .. index], true);
        start = index + 1;
    }
    return stacked;
}

/// Every block of the document containing the query, as search hits.
/// `Hit.section` = chapter index, `Hit.number` = block index (+1).
pub fn find(allocator: std.mem.Allocator, doc: document.Document, query: *const search.Query) !search.Results {
    var hits: std.ArrayList(search.Hit) = .empty;
    var occurrences: usize = 0;
    for (doc.chapters, 0..) |chapter, c| for (chapter.blocks, 0..) |block, b| {
        const first = search.next(block.text, query, 0) orelse continue;
        const count = search.count(block.text, query);
        occurrences += count;
        try hits.append(allocator, .{ .section = c, .chapter = @intCast(@min(c, std.math.maxInt(u16))), .number = @intCast(@min(b + 1, std.math.maxInt(u16))), .label = .{ .chapter = 0, .number = @intCast(@min(block.verse, std.math.maxInt(u16))) }, .text = block.text, .match = first, .occurrences = @intCast(count) });
    };
    return .{ .hits = try hits.toOwnedSlice(allocator), .occurrences = occurrences };
}

/// Arabic psalm number from a heading "Psalm <roman>. <N>" (or "Psalm N").
pub fn psalmNumber(text: []const u8) ?u16 {
    if (!std.mem.startsWith(u8, text, "Psalm ")) return null;
    var words = std.mem.tokenizeAny(u8, text, " .");
    var last: []const u8 = "";
    while (words.next()) |word| last = word;
    return std.fmt.parseInt(u16, last, 10) catch null;
}

fn statePath(allocator: std.mem.Allocator, base: []const u8, file: []const u8) ![]const u8 {
    const hash = std.hash.Wyhash.hash(0, file);
    return std.fmt.allocPrint(allocator, "{s}.books/{x}.json", .{ base, hash });
}

pub fn loadPosition(allocator: std.mem.Allocator, io: std.Io, base: []const u8, file: []const u8) Position {
    const path = statePath(allocator, base, file) catch return .{};
    const bytes = std.Io.Dir.cwd().readFileAlloc(io, path, allocator, .limited(4096)) catch return .{};
    const parsed = std.json.parseFromSliceLeaky(Position, allocator, bytes, .{ .ignore_unknown_fields = true }) catch return .{};
    return parsed;
}

pub fn savePosition(allocator: std.mem.Allocator, io: std.Io, base: []const u8, file: []const u8, position: Position) !void {
    const path = try statePath(allocator, base, file);
    const bytes = try std.json.Stringify.valueAlloc(allocator, position, .{});
    const temporary = try std.fmt.allocPrint(allocator, "{s}.tmp", .{path});
    const cwd = std.Io.Dir.cwd();
    if (std.fs.path.dirname(path)) |parent| try cwd.createDirPath(io, parent);
    try cwd.writeFile(io, .{ .sub_path = temporary, .data = bytes });
    try cwd.rename(temporary, cwd, path, io);
}

pub const Exit = enum { quit, home };

fn chapterPicker(allocator: std.mem.Allocator, doc: document.Document, current: usize) !?usize {
    var labels: std.ArrayList([]const u8) = .empty;
    for (doc.chapters) |chapter| try labels.append(allocator, try std.fmt.allocPrint(allocator, "{s} · {d} blocks", .{ chapter.title, chapter.blocks.len }));
    return tui.choose(allocator, "Choose chapter", doc.title, labels.items, current, 1);
}

/// Reads `doc` inside the application terminal until the user quits or
/// returns to the library. The position is saved on exit.
/// Image block nearest the top of the view: on screen first, then the
/// closest one above it in this chapter.
fn imageNear(blocks: []const document.Block, lines: []const Line, top: usize, height: usize) ?[]const u8 {
    var i = top;
    while (i < @min(lines.len, top + height)) : (i += 1) {
        const block = blocks[lines[i].block];
        if (block.kind == .image) return block.text;
    }
    var b = if (lines.len > 0) lines[@min(top, lines.len - 1)].block else 0;
    while (true) {
        if (b < blocks.len and blocks[b].kind == .image) return blocks[b].text;
        if (b == 0) return null;
        b -= 1;
    }
}

/// Opens an asset in the desktop's image viewer, detached from the terminal.
fn openImage(io: std.Io, path: []const u8) !void {
    const argv: []const []const u8 = switch (@import("builtin").os.tag) {
        .macos => &.{ "open", path },
        .windows => &.{ "cmd", "/c", "start", "", path },
        else => &.{ "xdg-open", path },
    };
    _ = try std.process.spawn(io, .{ .argv = argv, .stdin = .ignore, .stdout = .ignore, .stderr = .ignore, .pgid = if (@import("builtin").os.tag == .windows) null else 0 });
}

pub fn run(gpa: std.mem.Allocator, io: std.Io, doc: document.Document, file: []const u8, base: []const u8, library_dir: []const u8, caps: graphics.Capabilities, start: ?Position, portion: ?Portion) !Exit {
    var arena = std.heap.ArenaAllocator.init(gpa);
    defer arena.deinit();
    const allocator = arena.allocator();
    var position = start orelse loadPosition(allocator, io, base, file);
    if (position.chapter >= doc.chapters.len) position = .{};
    var finder: search.State = .{};
    defer finder.deinit();
    var chapter = position.chapter;
    var top: usize = 0;
    var anchor_block: ?usize = position.block;
    var lines: []Line = &.{};
    var wrapping = std.heap.ArenaAllocator.init(gpa);
    defer wrapping.deinit();
    var laid_width: usize = 0;
    var laid_chapter: usize = std.math.maxInt(usize);
    var notice: []const u8 = if (portion) |p| try std.fmt.allocPrint(allocator, "Plan reading: {s} · read to the end-of-reading marker; m returns to the plan", .{p.label}) else "/ search · t chapters · H/L (Shift+←/→) chapters · m library · q quit";
    var redraw = true;
    var old: tui.Size = .{ .columns = 0, .rows = 0 };
    defer tui.setTextInput(false);
    var exit: Exit = .quit;
    var images: inline_image.Renderer = .{
        .allocator = gpa,
        .io = io,
        .dir = try std.fmt.allocPrint(allocator, "{s}/{s}.assets", .{ library_dir, file }),
        .caps = caps,
    };
    defer images.deinit();
    // Kitty keeps image data in the terminal; free it when leaving the book.
    defer if (images.caps.protocol == .kitty) tui.writeAll("\x1b_Ga=d,d=A,q=2\x1b\\") catch {};
    var laid_height: usize = 0;

    var zoom: usize = 2;
    loop: while (!tui.isInterrupted()) {
        const size = try tui.screenSize();
        if (size.columns != old.columns or size.rows != old.rows) {
            old = size;
            redraw = true;
        }
        const columns = size.columns;
        const height = size.rows;
        if (images.caps.protocol != .none and size.pixel_width >= columns and size.pixel_height >= height) {
            const cw: u16 = @intCast(size.pixel_width / columns);
            const ch: u16 = @intCast(size.pixel_height / height);
            if (images.setCell(cw, ch)) laid_width = 0; // re-reserve image rows
        }
        const panel_width: usize = if (!finder.open) 0 else if (columns >= 72) @min(64, @max(30, columns * 2 / 5)) else columns;
        const reader_visible = panel_width < columns;
        const text_width = @min(100, (columns - panel_width) -| (gutter_width + 3));
        const body_height = height -| 5;
        const usable = body_height >= 3 and (!reader_visible or text_width >= 20);
        // Zoom levels (+/-): ¼, ½, ¾ (default), full body, full + enlarge.
        const image_rows = @max(4, body_height * @min(zoom + 1, 4) / 4);
        images.setEnlarge(zoom == 4);
        if (usable and reader_visible and (text_width != laid_width or chapter != laid_chapter or (images.enabled() and image_rows != laid_height))) {
            const keep = anchor_block orelse if (lines.len > 0) lines[@min(top, lines.len - 1)].block else 0;
            _ = wrapping.reset(.retain_capacity);
            lines = try renderWith(wrapping.allocator(), doc.chapters[chapter], text_width, if (images.enabled()) &images else null, image_rows, if (portion != null and portion.?.chapter == chapter) portion.?.end else null);
            laid_width = text_width;
            laid_height = image_rows;
            laid_chapter = chapter;
            top = 0;
            for (lines, 0..) |line, i| if (line.block >= keep) {
                top = i;
                // Show the blank spacer / page rule above a block, if any.
                break;
            };
            anchor_block = null;
            redraw = true;
        }
        const max_top = lines.len -| body_height;
        top = @min(top, max_top);
        // Apply a burst of keys (held j, fast scrolling) before redrawing:
        // images make each frame costly, and stale frames only add lag.
        // (redraw stays pending, so an ignored final byte cannot lose a frame.)
        if (redraw and !tui.inputPending()) {
            var frame = std.Io.Writer.Allocating.init(allocator);
            defer frame.deinit();
            const w = &frame.writer;
            // Synchronized update (DEC 2026): the terminal shows the frame only
            // when complete, so clearing, text, and images never flash.
            if (images.caps.protocol == .kitty) {
                // Ghostty treats ED2 (clear screen) as "delete visible
                // placements and any image left unused", which threw away
                // the transmitted figure on the first scroll. Erase line by
                // line instead (EL keeps images); placements are reset
                // explicitly in beginFrame.
                try w.writeAll("\x1b[?2026h" ++ paper ++ ink);
                for (1..height + 1) |row| try w.print("\x1b[{d};1H\x1b[2K", .{row});
                try w.writeAll("\x1b[H");
            } else try w.writeAll("\x1b[?2026h\x1b[H\x1b[2J" ++ paper ++ ink);
            if (!usable) {
                try w.writeAll("Kata: enlarge the terminal. q quits.");
            } else {
                const title = try std.fmt.allocPrint(allocator, "{s} · {s}  ({d}/{d})", .{ doc.title, doc.chapters[chapter].title, chapter + 1, doc.chapters.len });
                try w.writeAll("\x1b[1;1H\x1b[1m" ++ gold ++ " K A T A  \x1b[0m" ++ paper ++ ink);
                try w.writeAll(tui.clipped(title, columns -| 12));
                try w.writeAll("\x1b[2;1H" ++ dim);
                try w.writeAll(tui.clipped(notice, columns));
                const highlight = finder.highlight();
                if (reader_visible) {
                    for (0..body_height) |row| {
                        try w.print("\x1b[{d};1H", .{row + 3});
                        const at = top + row;
                        if (at >= lines.len) continue;
                        const line = lines[at];
                        try w.writeAll(dim);
                        var gutter: [gutter_width]u8 = @splat(' ');
                        const g = line.gutter[0..@min(line.gutter.len, gutter_width - 1)];
                        @memcpy(gutter[gutter_width - 1 - g.len .. gutter_width - 1], g);
                        try w.writeAll(&gutter);
                        try w.writeAll(rule ++ "│ ");
                        const color: []const u8 = switch (line.style) {
                            .heading => paper ++ "\x1b[1m" ++ gold,
                            .code => paper ++ code_ink,
                            .quote, .note, .figure, .page => paper ++ dim,
                            .body => paper ++ ink,
                        };
                        try w.writeAll(color);
                        if (line.style == .code) try w.writeAll("  ");
                        const visible = tui.clipped(line.text, text_width);
                        const origin = @intFromPtr(line.full.ptr);
                        const begin = @intFromPtr(visible.ptr);
                        if (highlight != null and visible.len > 0 and begin >= origin and begin + visible.len <= origin + line.full.len) {
                            const from = begin - origin;
                            try tui.writeMarkedStyled(w, line.full, from, from + visible.len, highlight.?.query, 0, color);
                        } else try w.writeAll(visible);
                        try w.writeAll("\x1b[0m" ++ paper);
                    }
                }
                try images.beginFrame(w);
                if (reader_visible and images.enabled()) {
                    // Paint each visible image once, from its first visible row.
                    var row: usize = 0;
                    while (row < body_height) {
                        const at = top + row;
                        if (at >= lines.len) break;
                        const line = lines[at];
                        if (line.image.len == 0) {
                            row += 1;
                            continue;
                        }
                        var count: usize = 0;
                        while (row + count < body_height and top + row + count < lines.len and lines[top + row + count].image.ptr == line.image.ptr and lines[top + row + count].block == line.block) count += 1;
                        try w.print("\x1b[{d};{d}H", .{ row + 3, gutter_width + 3 });
                        if (!try images.draw(w, line.image, text_width, image_rows, line.image_row, count)) {
                            try w.writeAll(dim ++ "▣ image could not be decoded · i opens it");
                        }
                        row += count;
                    }
                }
                if (finder.open) try renderPanel(allocator, w, &finder, doc, columns - panel_width + 1, panel_width, 3, height - 2);
                try w.print("\x1b[{d};1H" ++ gold, .{height - 1});
                const help = if (finder.prompt)
                    "Type a word or phrase · matches highlight as you type · Enter searches the whole book · Esc cancels"
                else if (finder.open and finder.focus)
                    "j/k select · Enter open · n/N next/previous · Tab/Esc back to reader · / new search · x close search"
                else
                    "j/k or ↑/↓ scroll · Ctrl-d/u page · g/G ends · H/L chapter · t chapters · / search · n/N · i image · +/- size · x close";
                try w.writeAll(tui.clipped(help, columns));
                try w.print("\x1b[{d};1H" ++ dim, .{height});
                if (finder.prompt) {
                    const line = try std.fmt.allocPrint(allocator, "/{s}█   searching this book", .{finder.promptText()});
                    try w.writeAll(ink);
                    try w.writeAll(tui.clipped(line, columns));
                } else {
                    const where = if (lines.len > 0) lines[@min(top, lines.len - 1)].block else 0;
                    var page: []const u8 = "";
                    for (doc.chapters[chapter].blocks[0..@min(where + 1, doc.chapters[chapter].blocks.len)]) |block| {
                        if (block.page.len > 0) page = block.page;
                    }
                    const status = try std.fmt.allocPrint(allocator, "m library · q quit · line {d}/{d}{s}{s}", .{ @min(top + 1, lines.len), lines.len, if (page.len > 0) " · page " else "", page });
                    try w.writeAll(tui.clipped(status, columns));
                }
            }
            try w.writeAll("\x1b[?2026l");
            try tui.writeAll(frame.written());
            redraw = false;
        }
        tui.setTextInput(finder.prompt);
        const key = try tui.readKey() orelse continue;
        redraw = true;
        if (finder.prompt) {
            switch (key) {
                27 => finder.cancel(),
                10, 13 => {
                    finder.prompt = false;
                    if (finder.typing.empty()) continue;
                    _ = finder.arena.reset(.retain_capacity);
                    const results = try find(finder.arena.allocator(), doc, &finder.typing);
                    const typed = try allocator.dupe(u8, finder.promptText());
                    if (results.hits.len == 0) {
                        notice = try std.fmt.allocPrint(allocator, "No matches for “{s}” in this book.", .{typed});
                        finder.close();
                        continue;
                    }
                    finder.active = finder.typing;
                    @memcpy(finder.submitted[0..finder.text_len], finder.promptText());
                    finder.submitted_len = finder.text_len;
                    finder.hits = results.hits;
                    finder.occurrences = results.occurrences;
                    finder.selected = 0;
                    for (results.hits, 0..) |hit, i| if (hit.section >= chapter) {
                        finder.selected = i;
                        break;
                    };
                    finder.open = true;
                    finder.focus = true;
                    notice = try std.fmt.allocPrint(allocator, "{d} passages contain “{s}”. Enter opens a result; n/N steps through them.", .{ results.hits.len, typed });
                },
                127, 8 => finder.backspace(),
                21 => finder.clear(),
                else => finder.input(key),
            }
            continue;
        }
        var jump = false;
        if (finder.open and finder.focus) {
            const page = @max(1, body_height / 2);
            switch (key) {
                'j' => finder.selected = @min(finder.hits.len -| 1, finder.selected + 1),
                'k' => finder.selected -|= 1,
                4, 'f' => finder.selected = @min(finder.hits.len -| 1, finder.selected + page),
                21, 'b' => finder.selected -|= page,
                'g' => finder.selected = 0,
                'G' => finder.selected = finder.hits.len -| 1,
                10, 13 => jump = true,
                'n' => {
                    finder.selected = @min(finder.hits.len -| 1, finder.selected + 1);
                    jump = true;
                },
                'N' => {
                    finder.selected -|= 1;
                    jump = true;
                },
                27, 9, 'q', 'h' => finder.focus = false,
                'x' => finder.close(),
                '/' => finder.begin(0),
                else => {},
            }
            if (!jump) continue;
        } else if (finder.open and (key == 'n' or key == 'N')) {
            if (key == 'n') finder.selected = @min(finder.hits.len -| 1, finder.selected + 1) else finder.selected -|= 1;
            jump = true;
        }
        if (jump) {
            const hit = finder.current() orelse continue;
            chapter = hit.section;
            anchor_block = hit.number - 1;
            laid_chapter = std.math.maxInt(usize);
            continue;
        }
        switch (key) {
            'q', 3 => break :loop,
            'm' => {
                exit = .home;
                break :loop;
            },
            'j' => top = @min(max_top, top + 1),
            'k' => top -|= 1,
            4, 'f', ' ' => top = @min(max_top, top + @max(1, body_height / 2)),
            21, 'b' => top -|= @max(1, body_height / 2),
            'g' => top = 0,
            'G' => top = max_top,
            'L' => if (chapter + 1 < doc.chapters.len) {
                chapter += 1;
                anchor_block = 0;
            } else {
                notice = "Last chapter. H goes back; t lists chapters.";
            },
            'H' => if (chapter > 0) {
                chapter -= 1;
                anchor_block = 0;
            } else {
                notice = "First chapter. L goes forward; t lists chapters.";
            },
            't', 'o' => if (try chapterPicker(allocator, doc, chapter)) |picked| {
                chapter = picked;
                anchor_block = 0;
                laid_chapter = std.math.maxInt(usize);
            },
            '/' => finder.begin(0),
            'r' => if (finder.open) {
                finder.focus = true;
            },
            'i' => if (imageNear(doc.chapters[chapter].blocks, lines, top, body_height)) |name| {
                const path = try std.fmt.allocPrint(allocator, "{s}/{s}.assets/{s}", .{ library_dir, file, name });
                notice = if (std.Io.Dir.cwd().statFile(io, path, .{})) |_| blk: {
                    openImage(io, path) catch |err| break :blk try std.fmt.allocPrint(allocator, "Cannot open image viewer: {s}. File: {s}", .{ @errorName(err), path });
                    break :blk try std.fmt.allocPrint(allocator, "Opened {s} in your image viewer.", .{name});
                } else |_| "Image file missing; run `kata ingest --force` to restore assets.";
            } else {
                notice = "No image on screen or above it in this chapter.";
            },
            'x' => if (finder.open) {
                finder.close();
                notice = "Search closed.";
            },
            '+', '=', '-', '_' => {
                const grow = key == '+' or key == '=';
                if (grow and zoom < 4) zoom += 1 else if (!grow and zoom > 0) zoom -= 1;
                const names = [_][]const u8{ "¼ screen", "½ screen", "¾ screen (default)", "full screen", "full screen, small images enlarged" };
                notice = if (images.enabled()) try std.fmt.allocPrint(allocator, "Images: up to {s}. +/- change size.", .{names[zoom]}) else "Inline images are off in this terminal; i opens them.";
                anchor_block = if (lines.len > 0) lines[@min(top, lines.len - 1)].block else null;
            },
            else => {},
        }
    }
    const block = if (lines.len > 0) lines[@min(top, lines.len - 1)].block else 0;
    savePosition(allocator, io, base, file, .{ .chapter = chapter, .block = block }) catch {};
    return exit;
}

fn renderPanel(allocator: std.mem.Allocator, w: *std.Io.Writer, finder: *const search.State, doc: document.Document, x: usize, width: usize, top: usize, bottom: usize) !void {
    const inner = width -| 3;
    var row = top;
    const header = try std.fmt.allocPrint(allocator, "Search · “{s}”", .{finder.submittedText()});
    try w.print("\x1b[{d};{d}H" ++ rule ++ "┃ {s}", .{ row, x, if (finder.focus) "\x1b[1m" ++ gold else dim });
    try tui.cell(w, header, inner);
    try w.writeAll("\x1b[0m" ++ paper);
    row += 1;
    const summary = try std.fmt.allocPrint(allocator, "{d} passages · {d} matches · {d}/{d}", .{ finder.hits.len, finder.occurrences, if (finder.hits.len == 0) 0 else finder.selected + 1, finder.hits.len });
    try w.print("\x1b[{d};{d}H" ++ rule ++ "┃ " ++ dim, .{ row, x });
    try tui.cell(w, summary, inner);
    row += 1;
    const capacity = (bottom + 1) -| row;
    if (capacity == 0) return;
    const first = @min(finder.selected -| (capacity / 2), finder.hits.len -| capacity);
    for (0..capacity) |offset| {
        try w.print("\x1b[{d};{d}H" ++ rule ++ "┃ ", .{ row + offset, x });
        const index = first + offset;
        if (index >= finder.hits.len) {
            try tui.cell(w, "", inner);
            continue;
        }
        const hit = finder.hits[index];
        const chosen = index == finder.selected;
        const chapter_title = doc.chapters[hit.section].title;
        const place = if (hit.label.number > 0)
            try std.fmt.allocPrint(allocator, "{s}{s} {d} ", .{ if (chosen) "▶ " else "  ", chapter_title, hit.label.number })
        else
            try std.fmt.allocPrint(allocator, "{s}{s} ", .{ if (chosen) "▶ " else "  ", chapter_title });
        const short = tui.clipped(place, @min(inner, 28));
        try w.writeAll(if (chosen) gold else dim);
        try w.writeAll(short);
        var used = layout.displayWidth(short);
        if (short.len < place.len and used < inner) {
            try w.writeAll("… ");
            used += 2;
        }
        try w.writeAll(ink);
        var from: usize = 0;
        if (hit.match.start > 24) {
            from = hit.match.start - 24;
            while (from < hit.match.start and (hit.text[from] & 0xC0) == 0x80) from += 1;
            if (std.mem.indexOfScalarPos(u8, hit.text[0..hit.match.start], from, ' ')) |space| from = space + 1;
            if (used + 1 < inner) {
                try w.writeAll("…");
                used += 1;
            }
        }
        // Snippets are single-line: stop at an embedded newline.
        const rest = hit.text[from..];
        const one_line = rest[0 .. std.mem.indexOfScalar(u8, rest, '\n') orelse rest.len];
        const visible = tui.clipped(one_line, inner -| used);
        try tui.writeMarkedStyled(w, hit.text, from, from + visible.len, &finder.active, 0, paper ++ ink);
        used += layout.displayWidth(visible);
        for (used..inner) |_| try w.writeByte(' ');
    }
}

test "chapters render with verse gutters, page rules, and intact code" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const chapter: document.Chapter = .{ .title = "One", .blocks = &.{
        .{ .kind = .heading, .level = 2, .text = "Psalm I. 1" },
        .{ .kind = .verse, .verse = 1, .text = "Blessed is the man that hath not walked in the counsel of the ungodly" },
        .{ .kind = .verse, .verse = 2, .page = "26", .text = "But his will" },
        .{ .kind = .code, .text = "fn x() {\n    y();\n}" },
    } };
    const lines = try render(arena.allocator(), chapter, 30);
    var gutters: usize = 0;
    var page_rule = false;
    var code_indent = false;
    for (lines) |line| {
        if (line.gutter.len > 0) gutters += 1;
        if (line.style == .page and std.mem.indexOf(u8, line.text, "page 26") != null) page_rule = true;
        if (line.style == .code and std.mem.eql(u8, line.text, "    y();")) code_indent = true;
        try std.testing.expect(layout.displayWidth(line.text) <= 30);
    }
    try std.testing.expectEqual(@as(usize, 2), gutters);
    try std.testing.expect(page_rule and code_indent);
}

test "table rows render as aligned columns that wrap within the width" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const short: document.Chapter = .{ .title = "T", .blocks = &.{
        .{ .kind = .table_row, .text = "Day │ Matins │ Vespers" },
        .{ .kind = .table_row, .text = "Su │ II, III │" },
        .{ .kind = .table_row, .text = "M │ IV, V │ VI" },
    } };
    const grid = try render(arena.allocator(), short, 60);
    try std.testing.expectEqual(@as(usize, 3), grid.len);
    try std.testing.expectEqualStrings("  Day │ Matins  │ Vespers", grid[0].text);
    try std.testing.expectEqualStrings("  Su  │ II, III │", grid[1].text);
    try std.testing.expectEqualStrings("  M   │ IV, V   │ VI", grid[2].text);
    try std.testing.expectEqual(@as(usize, 2), grid[2].block);
    const long: document.Chapter = .{ .title = "T", .blocks = &.{
        .{ .kind = .table_row, .text = "Concept │ What you might model" },
        .{ .kind = .table_row, .text = "What is the goal? │ Define too slow. How fast should the information be received and why?" },
    } };
    const wrapped = try render(arena.allocator(), long, 40);
    var rules: usize = 0;
    for (wrapped) |line| {
        try std.testing.expect(layout.displayWidth(line.text) <= 40);
        if (line.style == .page) rules += 1;
    }
    try std.testing.expectEqual(@as(usize, 1), rules);
    try std.testing.expect(wrapped.len > 4);
    // Columns line up: every content line has its separator at one column.
    const bar = std.mem.indexOf(u8, wrapped[0].text, "│").?;
    for (wrapped) |line| if (line.style != .page) try std.testing.expectEqual(bar, std.mem.indexOf(u8, line.text, "│").?);
}

test "psalm headings give their arabic number" {
    try std.testing.expectEqual(@as(?u16, 3), psalmNumber("Psalm III. 3"));
    try std.testing.expectEqual(@as(?u16, 118), psalmNumber("Psalm CXVIII. 118"));
    try std.testing.expect(psalmNumber("Foreword") == null);
}

test "runs of short paragraphs stack without spacer lines" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const chapter: document.Chapter = .{ .title = "Title", .blocks = &.{
        .{ .kind = .paragraph, .text = "Second Edition" },
        .{ .kind = .paragraph, .text = "Designing Fine-Grained Systems" },
        .{ .kind = .paragraph, .text = "Sam Newman" },
        .{ .kind = .paragraph, .text = "A longer paragraph that is ordinary prose and keeps its spacing above it." },
        .{ .kind = .paragraph, .text = "Short one" },
        .{ .kind = .paragraph, .text = "Short two" },
    } };
    const lines = try render(arena.allocator(), chapter, 100);
    var texts: std.ArrayList([]const u8) = .empty;
    for (lines) |line| try texts.append(arena.allocator(), line.text);
    const expected = [_][]const u8{ "Second Edition", "Designing Fine-Grained Systems", "Sam Newman", "", "A longer paragraph that is ordinary prose and keeps its spacing above it.", "", "Short one", "", "Short two" };
    try std.testing.expectEqual(expected.len, texts.items.len);
    for (expected, texts.items) |want, got| try std.testing.expectEqualStrings(want, got);
}

test "document search returns chapter and block locations" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    const doc: document.Document = .{ .title = "T", .chapters = &.{
        .{ .title = "A", .blocks = &.{ .{ .kind = .paragraph, .text = "nothing" }, .{ .kind = .verse, .verse = 7, .text = "Mercy and mercy" } } },
        .{ .title = "B", .blocks = &.{.{ .kind = .paragraph, .text = "merciful" }} },
    } };
    const query = search.Query.init("merc");
    const results = try find(arena.allocator(), doc, &query);
    try std.testing.expectEqual(@as(usize, 2), results.hits.len);
    try std.testing.expectEqual(@as(usize, 3), results.occurrences);
    try std.testing.expectEqual(@as(u16, 2), results.hits[0].number);
    try std.testing.expectEqual(@as(u16, 7), results.hits[0].label.number);
    try std.testing.expectEqual(@as(usize, 1), results.hits[1].section);
}

test "every sample book chapter lays out at narrow and wide widths" {
    const io = std.testing.io;
    var dir = std.Io.Dir.cwd().openDir(io, "examplePubs", .{ .iterate = true }) catch return error.SkipZigTest;
    defer dir.close(io);
    var it = dir.iterate();
    var books: usize = 0;
    while (try it.next(io)) |entry| {
        if (!std.mem.endsWith(u8, entry.name, ".epub")) continue;
        var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
        defer arena.deinit();
        const a = arena.allocator();
        const bytes = try dir.readFileAlloc(io, entry.name, a, .limited(128 << 20));
        const result = try @import("convert.zig").convert(a, bytes, entry.name);
        for (result.doc.chapters) |chapter| for ([_]usize{ 20, 37, 64, 100 }) |width| {
            const lines = render(a, chapter, width) catch |err| {
                std.debug.print("{s} · {s} @ {d}: {s}\n", .{ entry.name, chapter.title, width, @errorName(err) });
                return err;
            };
            for (lines) |line| try std.testing.expect(layout.displayWidth(line.text) <= width + 2);
        };
        books += 1;
    }
    try std.testing.expect(books >= 2);
}
