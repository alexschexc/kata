//! EPUB container reading (ingest stage 2): in-memory zip, container.xml,
//! OPF metadata/manifest/spine, and a DRM check. XHTML parsing is stage 3.
const std = @import("std");
const flate = std.compress.flate;

pub const Entry = struct { name: []const u8, method: u16, compressed: []const u8, size: usize };

/// Zip central directory over an EPUB held in memory (EPUBs are small).
pub const Archive = struct {
    bytes: []const u8,
    entries: []Entry,

    pub fn init(allocator: std.mem.Allocator, bytes: []const u8) !Archive {
        if (bytes.len < 22) return error.NotZip;
        var end: usize = bytes.len - 22;
        while (true) : (end -= 1) {
            if (std.mem.eql(u8, bytes[end..][0..4], "PK\x05\x06")) break;
            if (end == 0 or bytes.len - end > 22 + 65535) return error.NotZip;
        }
        var count: usize = std.mem.readInt(u16, bytes[end + 10 ..][0..2], .little);
        var at: usize = std.mem.readInt(u32, bytes[end + 16 ..][0..4], .little);
        // Zip64: saturated fields point to the zip64 end record via its locator.
        if (count == 0xFFFF or at == 0xFFFFFFFF) {
            if (end < 20 or !std.mem.eql(u8, bytes[end - 20 ..][0..4], "PK\x06\x07")) return error.CorruptZip;
            const record = std.math.cast(usize, std.mem.readInt(u64, bytes[end - 12 ..][0..8], .little)) orelse return error.CorruptZip;
            if (record + 56 > bytes.len or !std.mem.eql(u8, bytes[record..][0..4], "PK\x06\x06")) return error.CorruptZip;
            count = std.math.cast(usize, std.mem.readInt(u64, bytes[record + 32 ..][0..8], .little)) orelse return error.CorruptZip;
            at = std.math.cast(usize, std.mem.readInt(u64, bytes[record + 48 ..][0..8], .little)) orelse return error.CorruptZip;
        }
        if (count > bytes.len / 46) return error.CorruptZip;
        const entries = try allocator.alloc(Entry, count);
        errdefer allocator.free(entries);
        for (entries) |*entry| {
            if (at + 46 > bytes.len or !std.mem.eql(u8, bytes[at..][0..4], "PK\x01\x02")) return error.CorruptZip;
            const method = std.mem.readInt(u16, bytes[at + 10 ..][0..2], .little);
            var csize: u64 = std.mem.readInt(u32, bytes[at + 20 ..][0..4], .little);
            var usize_: u64 = std.mem.readInt(u32, bytes[at + 24 ..][0..4], .little);
            const name_len = std.mem.readInt(u16, bytes[at + 28 ..][0..2], .little);
            const extra_len = std.mem.readInt(u16, bytes[at + 30 ..][0..2], .little);
            const comment_len = std.mem.readInt(u16, bytes[at + 32 ..][0..2], .little);
            var local: u64 = std.mem.readInt(u32, bytes[at + 42 ..][0..4], .little);
            if (at + 46 + name_len + extra_len > bytes.len) return error.CorruptZip;
            const name = bytes[at + 46 ..][0..name_len];
            if (usize_ == 0xFFFFFFFF or csize == 0xFFFFFFFF or local == 0xFFFFFFFF) {
                // Zip64 extra field (0x0001): only the saturated values, in order.
                var extra = bytes[at + 46 + name_len ..][0..extra_len];
                const found = while (extra.len >= 4) {
                    const id = std.mem.readInt(u16, extra[0..2], .little);
                    const len = std.mem.readInt(u16, extra[2..4], .little);
                    if (4 + @as(usize, len) > extra.len) break null;
                    if (id == 1) break extra[4..][0..len];
                    extra = extra[4 + len ..];
                } else null;
                var values = found orelse return error.CorruptZip;
                inline for (.{ &usize_, &csize, &local }) |field| if (field.* == 0xFFFFFFFF) {
                    if (values.len < 8) return error.CorruptZip;
                    field.* = std.mem.readInt(u64, values[0..8], .little);
                    values = values[8..];
                };
            }
            if (local > bytes.len -| 30 or csize > bytes.len) return error.CorruptZip;
            const local_at: usize = @intCast(local);
            const data = local_at + 30 + std.mem.readInt(u16, bytes[local_at + 26 ..][0..2], .little) + std.mem.readInt(u16, bytes[local_at + 28 ..][0..2], .little);
            if (data + csize > bytes.len) return error.CorruptZip;
            entry.* = .{ .name = name, .method = method, .compressed = bytes[data..][0..@intCast(csize)], .size = std.math.cast(usize, usize_) orelse return error.CorruptZip };
            at += 46 + name_len + extra_len + comment_len;
        }
        return .{ .bytes = bytes, .entries = entries };
    }

    pub fn find(self: Archive, name: []const u8) ?Entry {
        for (self.entries) |entry| if (std.mem.eql(u8, entry.name, name)) return entry;
        return null;
    }

    /// Decompressed member contents (stored or deflate). Caller owns.
    pub fn read(self: Archive, allocator: std.mem.Allocator, name: []const u8) ![]u8 {
        const entry = self.find(name) orelse return error.MissingMember;
        if (entry.size > 64 << 20) return error.MemberTooLarge;
        switch (entry.method) {
            0 => return allocator.dupe(u8, entry.compressed),
            8 => {
                var input: std.Io.Reader = .fixed(entry.compressed);
                var output: std.Io.Writer.Allocating = .init(allocator);
                errdefer output.deinit();
                var decompress: flate.Decompress = .init(&input, .raw, &.{});
                _ = try decompress.reader.streamRemaining(&output.writer);
                if (output.written().len != entry.size) return error.CorruptZip;
                return output.toOwnedSlice();
            },
            else => return error.UnsupportedCompression,
        }
    }
};

/// Value of attribute `name` inside one start tag's text, if present.
pub fn attribute(tag: []const u8, name: []const u8) ?[]const u8 {
    var from: usize = 0;
    while (std.mem.indexOfPos(u8, tag, from, name)) |at| {
        from = at + name.len;
        const before_ok = at > 0 and std.ascii.isWhitespace(tag[at - 1]);
        var i = at + name.len;
        while (i < tag.len and std.ascii.isWhitespace(tag[i])) i += 1;
        if (!before_ok or i >= tag.len or tag[i] != '=') continue;
        i += 1;
        while (i < tag.len and std.ascii.isWhitespace(tag[i])) i += 1;
        if (i >= tag.len or (tag[i] != '"' and tag[i] != '\'')) continue;
        const close = std.mem.indexOfScalarPos(u8, tag, i + 1, tag[i]) orelse return null;
        return tag[i + 1 .. close];
    }
    return null;
}

/// Iterates start tags `<prefix…>` (any namespace prefix) in XML text.
pub const Tags = struct {
    text: []const u8,
    local: []const u8,
    at: usize = 0,

    pub fn next(self: *Tags) ?[]const u8 {
        while (std.mem.indexOfScalarPos(u8, self.text, self.at, '<')) |open| {
            const close = std.mem.indexOfScalarPos(u8, self.text, open, '>') orelse return null;
            self.at = close + 1;
            var name_end = open + 1;
            while (name_end < close and !std.ascii.isWhitespace(self.text[name_end]) and self.text[name_end] != '/') name_end += 1;
            const name = self.text[open + 1 .. name_end];
            const local = if (std.mem.lastIndexOfScalar(u8, name, ':')) |c| name[c + 1 ..] else name;
            if (std.mem.eql(u8, local, self.local)) return self.text[open..close];
        }
        return null;
    }
};

/// Text content of the first `<…local>` element (namespace-agnostic).
fn elementText(xml: []const u8, local: []const u8) ?[]const u8 {
    var tags: Tags = .{ .text = xml, .local = local };
    _ = tags.next() orelse return null;
    const start = tags.at;
    const end = std.mem.indexOfScalarPos(u8, xml, start, '<') orelse return null;
    return std.mem.trim(u8, xml[start..end], " \t\r\n");
}

/// Metadata text with entities decoded and whitespace collapsed.
fn decodeText(allocator: std.mem.Allocator, raw: []const u8) ![]const u8 {
    const xhtml = @import("xhtml.zig");
    var out: std.ArrayList(u8) = .empty;
    var i: usize = 0;
    while (i < raw.len) {
        if (raw[i] == '&') if (xhtml.entityAt(raw, i)) |entity| {
            var buf: [4]u8 = undefined;
            const n = std.unicode.utf8Encode(if (entity.cp == 0xA0) ' ' else entity.cp, &buf) catch 0;
            try out.appendSlice(allocator, buf[0..n]);
            i += entity.len;
            continue;
        };
        const c = raw[i];
        if (std.ascii.isWhitespace(c)) {
            if (out.items.len > 0 and out.items[out.items.len - 1] != ' ') try out.append(allocator, ' ');
        } else try out.append(allocator, c);
        i += 1;
    }
    return std.mem.trim(u8, out.items, " ");
}

pub const Item = struct { id: []const u8, href: []const u8, media_type: []const u8, properties: []const u8 };

pub const Package = struct {
    title: []const u8,
    author: []const u8,
    /// Spine, in reading order, as zip member paths.
    spine: []const []const u8,
    /// EPUB 3 nav document or EPUB 2 NCX (zip member path), if any.
    toc: ?[]const u8,
};

fn join(allocator: std.mem.Allocator, base_dir: []const u8, href: []const u8) ![]const u8 {
    const clean = if (std.mem.indexOfScalar(u8, href, '#')) |h| href[0..h] else href;
    if (base_dir.len == 0) return allocator.dupe(u8, clean);
    return std.fmt.allocPrint(allocator, "{s}/{s}", .{ base_dir, clean });
}

/// Refuses DRM: encryption.xml listing anything other than font obfuscation.
pub fn checkDrm(archive: Archive) !void {
    if (archive.find("META-INF/rights.xml") != null) return error.EncryptedEpub;
    if (archive.find("META-INF/encryption.xml") == null) return;
    var scratch: [64 << 10]u8 = undefined;
    var fixed = std.heap.FixedBufferAllocator.init(&scratch);
    const xml = archive.read(fixed.allocator(), "META-INF/encryption.xml") catch return error.EncryptedEpub;
    return checkEncryption(xml);
}

/// Font obfuscation (IDPF, Adobe) only scrambles embedded fonts; anything else
/// listed in encryption.xml means content is encrypted (DRM): refuse it.
fn checkEncryption(xml: []const u8) !void {
    const allowed = [_][]const u8{ "http://www.idpf.org/2008/embedding", "http://ns.adobe.com/pdf/enc#RC" };
    var methods: Tags = .{ .text = xml, .local = "EncryptionMethod" };
    while (methods.next()) |tag| {
        const algorithm = attribute(tag, "Algorithm") orelse return error.EncryptedEpub;
        for (allowed) |ok| {
            if (std.mem.eql(u8, algorithm, ok)) break;
        } else return error.EncryptedEpub;
    }
}

test "font obfuscation is allowed but encrypted content is refused" {
    try checkEncryption("<encryption><EncryptedData><enc:EncryptionMethod Algorithm=\"http://www.idpf.org/2008/embedding\"/></EncryptedData></encryption>");
    try std.testing.expectError(error.EncryptedEpub, checkEncryption("<EncryptionMethod Algorithm=\"http://www.w3.org/2001/04/xmlenc#aes128-cbc\"/>"));
    try std.testing.expectError(error.EncryptedEpub, checkEncryption("<EncryptionMethod/>"));
}

/// container.xml → OPF → metadata, manifest, spine, TOC. Strings borrow
/// from allocator-owned decompressed text.
pub fn package(allocator: std.mem.Allocator, archive: Archive) !Package {
    try checkDrm(archive);
    const container = try archive.read(allocator, "META-INF/container.xml");
    var rootfiles: Tags = .{ .text = container, .local = "rootfile" };
    const rootfile = rootfiles.next() orelse return error.NoRootfile;
    const opf_path = attribute(rootfile, "full-path") orelse return error.NoRootfile;
    const opf = try archive.read(allocator, opf_path);
    const base_dir = std.fs.path.dirnamePosix(opf_path) orelse "";

    var items: std.ArrayList(Item) = .empty;
    var item_tags: Tags = .{ .text = opf, .local = "item" };
    while (item_tags.next()) |tag| {
        try items.append(allocator, .{
            .id = attribute(tag, "id") orelse continue,
            .href = attribute(tag, "href") orelse continue,
            .media_type = attribute(tag, "media-type") orelse "",
            .properties = attribute(tag, "properties") orelse "",
        });
    }
    var toc: ?[]const u8 = null;
    for (items.items) |item| if (std.mem.indexOf(u8, item.properties, "nav") != null) {
        toc = try join(allocator, base_dir, item.href);
    };
    var spine_tags: Tags = .{ .text = opf, .local = "spine" };
    if (toc == null) if (spine_tags.next()) |spine_tag| if (attribute(spine_tag, "toc")) |ncx_id| {
        for (items.items) |item| if (std.mem.eql(u8, item.id, ncx_id)) {
            toc = try join(allocator, base_dir, item.href);
        };
    };
    var spine: std.ArrayList([]const u8) = .empty;
    var refs: Tags = .{ .text = opf, .local = "itemref" };
    while (refs.next()) |tag| {
        const idref = attribute(tag, "idref") orelse continue;
        for (items.items) |item| if (std.mem.eql(u8, item.id, idref)) {
            const path = try join(allocator, base_dir, item.href);
            if (archive.find(path) == null) return error.MissingSpineMember;
            try spine.append(allocator, path);
            break;
        };
    }
    if (spine.items.len == 0) return error.EmptySpine;
    return .{
        .title = try decodeText(allocator, elementText(opf, "title") orelse ""),
        .author = try decodeText(allocator, elementText(opf, "creator") orelse ""),
        .spine = spine.items,
        .toc = toc,
    };
}

test "metadata entities decode" {
    var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
    defer arena.deinit();
    try std.testing.expectEqualStrings("A — B & C", try decodeText(arena.allocator(), "A &#8212;\n  B &amp; C"));
}

test "attribute and tag scanning are namespace and quote tolerant" {
    try std.testing.expectEqualStrings("a b", attribute("<item id='x' href=\"a b\"", "href").?);
    try std.testing.expect(attribute("<item data-href=\"no\"", "href") == null);
    var tags: Tags = .{ .text = "<opf:item id=\"1\"/><itemref idref=\"2\"/><item id=\"3\">", .local = "item" };
    try std.testing.expectEqualStrings("1", attribute(tags.next().?, "id").?);
    try std.testing.expectEqualStrings("3", attribute(tags.next().?, "id").?);
    try std.testing.expect(tags.next() == null);
}

test "both sample EPUBs open with metadata, spine, and TOC" {
    const io = std.testing.io;
    const samples = [_]struct { path: []const u8, title: []const u8, spine: usize }{
        .{ .path = "examplePubs/The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub", .title = "Psalter", .spine = 33 },
        .{ .path = "examplePubs/buildingmicroservices2ndedition.epub", .title = "Microservices", .spine = 29 },
    };
    for (samples) |sample| {
        var arena = std.heap.ArenaAllocator.init(std.testing.allocator);
        defer arena.deinit();
        const a = arena.allocator();
        const bytes = std.Io.Dir.cwd().readFileAlloc(io, sample.path, a, .limited(64 << 20)) catch |err| switch (err) {
            error.FileNotFound => return error.SkipZigTest, // samples are local, not in git
            else => return err,
        };
        const archive = try Archive.init(a, bytes);
        const pkg = try package(a, archive);
        try std.testing.expect(std.mem.indexOf(u8, pkg.title, sample.title) != null);
        try std.testing.expectEqual(sample.spine, pkg.spine.len);
        try std.testing.expect(pkg.toc != null);
        for (pkg.spine) |member| {
            const text = try archive.read(a, member);
            try std.testing.expect(std.mem.indexOf(u8, text, "<body") != null);
        }
    }
}

test "zip64 central directory and extra fields are read" {
    // One stored member "a.txt" = "hello", written zip64-style: saturated
    // sizes/offset in the central entry, real values in the 0x0001 extra,
    // and a saturated end record pointing at the zip64 end record.
    var buf: std.Io.Writer.Allocating = .init(std.testing.allocator);
    defer buf.deinit();
    const w = &buf.writer;
    const le = std.builtin.Endian.little;
    try w.writeAll("PK\x03\x04");
    try w.writeInt(u16, 45, le); // version
    try w.writeInt(u16, 0, le);
    try w.writeInt(u16, 0, le); // stored
    try w.writeInt(u32, 0, le); // time/date
    try w.writeInt(u32, 0x3610a686, le); // crc (unchecked)
    try w.writeInt(u32, 5, le);
    try w.writeInt(u32, 5, le);
    try w.writeInt(u16, 5, le);
    try w.writeInt(u16, 0, le);
    try w.writeAll("a.txthello");
    const central = buf.written().len;
    try w.writeAll("PK\x01\x02");
    try w.writeInt(u16, 45, le);
    try w.writeInt(u16, 45, le);
    try w.writeInt(u16, 0, le);
    try w.writeInt(u16, 0, le);
    try w.writeInt(u32, 0, le);
    try w.writeInt(u32, 0x3610a686, le);
    try w.writeInt(u32, 0xFFFFFFFF, le);
    try w.writeInt(u32, 0xFFFFFFFF, le);
    try w.writeInt(u16, 5, le);
    try w.writeInt(u16, 4 + 24, le); // extra
    try w.writeInt(u16, 0, le);
    try w.writeInt(u16, 0, le);
    try w.writeInt(u16, 0, le);
    try w.writeInt(u32, 0, le);
    try w.writeInt(u32, 0xFFFFFFFF, le);
    try w.writeAll("a.txt");
    try w.writeInt(u16, 1, le);
    try w.writeInt(u16, 24, le);
    try w.writeInt(u64, 5, le); // uncompressed
    try w.writeInt(u64, 5, le); // compressed
    try w.writeInt(u64, 0, le); // local header offset
    const record = buf.written().len;
    const central_len = record - central;
    try w.writeAll("PK\x06\x06");
    try w.writeInt(u64, 44, le);
    try w.writeInt(u16, 45, le);
    try w.writeInt(u16, 45, le);
    try w.writeInt(u32, 0, le);
    try w.writeInt(u32, 0, le);
    try w.writeInt(u64, 1, le);
    try w.writeInt(u64, 1, le);
    try w.writeInt(u64, central_len, le);
    try w.writeInt(u64, central, le);
    try w.writeAll("PK\x06\x07");
    try w.writeInt(u32, 0, le);
    try w.writeInt(u64, record, le);
    try w.writeInt(u32, 1, le);
    try w.writeAll("PK\x05\x06");
    try w.writeInt(u16, 0, le);
    try w.writeInt(u16, 0, le);
    try w.writeInt(u16, 0xFFFF, le);
    try w.writeInt(u16, 0xFFFF, le);
    try w.writeInt(u32, 0xFFFFFFFF, le);
    try w.writeInt(u32, 0xFFFFFFFF, le);
    try w.writeInt(u16, 0, le);
    const archive = try Archive.init(std.testing.allocator, buf.written());
    defer std.testing.allocator.free(archive.entries);
    try std.testing.expectEqual(@as(usize, 1), archive.entries.len);
    const data = try archive.read(std.testing.allocator, "a.txt");
    defer std.testing.allocator.free(data);
    try std.testing.expectEqualStrings("hello", data);
}
