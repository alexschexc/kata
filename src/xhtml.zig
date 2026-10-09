//! Tolerant XHTML/XML tokenizer for EPUB content documents. No DTDs, no
//! validation: comments, processing instructions, and doctypes are skipped;
//! CDATA is returned as literal text. Entities are decoded by `appendText`.
const std = @import("std");

pub const Tag = struct {
    /// Element name as written (may carry a namespace prefix).
    name: []const u8,
    /// Whole start tag without the angle brackets, for attribute lookup.
    raw: []const u8,
    self_closing: bool,

    /// Element name without any namespace prefix, e.g. "svg" for "svg:svg".
    pub fn local(self: Tag) []const u8 {
        return localName(self.name);
    }

    pub fn attr(self: Tag, name: []const u8) ?[]const u8 {
        return attribute(self.raw, name);
    }

    /// True when the whitespace-separated attribute value contains `token`.
    pub fn hasToken(self: Tag, name: []const u8, token: []const u8) bool {
        const value = self.attr(name) orelse return false;
        var words = std.mem.tokenizeAny(u8, value, " \t\r\n");
        while (words.next()) |word| if (std.mem.eql(u8, word, token)) return true;
        return false;
    }
};

pub const Token = union(enum) {
    /// Raw character data; entities still encoded.
    text: []const u8,
    /// CDATA section contents; literal.
    cdata: []const u8,
    start: Tag,
    /// End tag name as written.
    end: []const u8,
};

pub fn localName(name: []const u8) []const u8 {
    return if (std.mem.lastIndexOfScalar(u8, name, ':')) |colon| name[colon + 1 ..] else name;
}

/// Value of attribute `name` in a start tag's raw text (exact name, so
/// "type" does not match "epub:type" or "data-type").
pub fn attribute(raw: []const u8, name: []const u8) ?[]const u8 {
    var from: usize = 0;
    while (std.mem.indexOfPos(u8, raw, from, name)) |at| {
        from = at + name.len;
        if (at == 0 or !std.ascii.isWhitespace(raw[at - 1])) continue;
        var i = at + name.len;
        while (i < raw.len and std.ascii.isWhitespace(raw[i])) i += 1;
        if (i >= raw.len or raw[i] != '=') continue;
        i += 1;
        while (i < raw.len and std.ascii.isWhitespace(raw[i])) i += 1;
        if (i >= raw.len or (raw[i] != '"' and raw[i] != '\'')) continue;
        const close = std.mem.indexOfScalarPos(u8, raw, i + 1, raw[i]) orelse return null;
        return raw[i + 1 .. close];
    }
    return null;
}

pub const Tokenizer = struct {
    src: []const u8,
    at: usize = 0,

    pub fn next(self: *Tokenizer) ?Token {
        const s = self.src;
        while (self.at < s.len) {
            if (s[self.at] != '<') {
                const end = std.mem.indexOfScalarPos(u8, s, self.at, '<') orelse s.len;
                defer self.at = end;
                return .{ .text = s[self.at..end] };
            }
            const rest = s[self.at..];
            if (std.mem.startsWith(u8, rest, "<!--")) {
                const end = std.mem.indexOfPos(u8, s, self.at + 4, "-->") orelse s.len;
                self.at = @min(s.len, end + 3);
                continue;
            }
            if (std.mem.startsWith(u8, rest, "<![CDATA[")) {
                const start = self.at + 9;
                const end = std.mem.indexOfPos(u8, s, start, "]]>") orelse s.len;
                self.at = @min(s.len, end + 3);
                return .{ .cdata = s[start..end] };
            }
            if (std.mem.startsWith(u8, rest, "<!") or std.mem.startsWith(u8, rest, "<?")) {
                const end = std.mem.indexOfScalarPos(u8, s, self.at, '>') orelse s.len;
                self.at = @min(s.len, end + 1);
                continue;
            }
            // Find the closing '>' outside quoted attribute values.
            var i = self.at + 1;
            var quote: u8 = 0;
            while (i < s.len) : (i += 1) {
                const c = s[i];
                if (quote != 0) {
                    if (c == quote) quote = 0;
                } else if (c == '"' or c == '\'') quote = c else if (c == '>') break;
            }
            const inner = s[self.at + 1 .. @min(i, s.len)];
            self.at = @min(s.len, i + 1);
            if (inner.len == 0) return .{ .text = "<" };
            if (inner[0] == '/') {
                return .{ .end = std.mem.trim(u8, inner[1..], " \t\r\n") };
            }
            var name_end: usize = 0;
            while (name_end < inner.len and !std.ascii.isWhitespace(inner[name_end]) and inner[name_end] != '/') name_end += 1;
            if (name_end == 0) return .{ .text = "<" };
            const trimmed = std.mem.trimEnd(u8, inner, " \t\r\n");
            return .{ .start = .{ .name = inner[0..name_end], .raw = inner, .self_closing = std.mem.endsWith(u8, trimmed, "/") } };
        }
        return null;
    }
};

const named = [_]struct { name: []const u8, cp: u21 }{
    .{ .name = "amp", .cp = '&' },      .{ .name = "lt", .cp = '<' },       .{ .name = "gt", .cp = '>' },
    .{ .name = "quot", .cp = '"' },     .{ .name = "apos", .cp = '\'' },    .{ .name = "nbsp", .cp = 0xA0 },
    .{ .name = "mdash", .cp = 0x2014 }, .{ .name = "ndash", .cp = 0x2013 }, .{ .name = "hellip", .cp = 0x2026 },
    .{ .name = "lsquo", .cp = 0x2018 }, .{ .name = "rsquo", .cp = 0x2019 }, .{ .name = "ldquo", .cp = 0x201C },
    .{ .name = "rdquo", .cp = 0x201D }, .{ .name = "copy", .cp = 0xA9 },    .{ .name = "shy", .cp = 0xAD },
};

/// Decoded code point and byte length of an entity at text[at] ('&'), or null.
pub fn entityAt(text: []const u8, at: usize) ?struct { cp: u21, len: usize } {
    const semi = std.mem.indexOfScalarPos(u8, text, at, ';') orelse return null;
    if (semi - at > 12 or semi == at + 1) return null;
    const body = text[at + 1 .. semi];
    if (body[0] == '#') {
        const value = if (body.len > 1 and (body[1] == 'x' or body[1] == 'X'))
            std.fmt.parseInt(u21, body[2..], 16) catch return null
        else
            std.fmt.parseInt(u21, body[1..], 10) catch return null;
        if (!std.unicode.utf8ValidCodepoint(value) or value == 0) return null;
        return .{ .cp = value, .len = semi + 1 - at };
    }
    for (named) |entry| if (std.mem.eql(u8, entry.name, body)) return .{ .cp = entry.cp, .len = semi + 1 - at };
    return null;
}

test "tokenizer handles comments, CDATA, quotes, prefixes, and self-closing tags" {
    var t: Tokenizer = .{ .src = "<?xml version=\"1.0\"?><!DOCTYPE html><!-- c --><p class=\"a>b\">x &amp; y<br/><![CDATA[<raw>]]></p><svg:svg/>" };
    try std.testing.expectEqualStrings("p", t.next().?.start.name);
    try std.testing.expectEqualStrings("x &amp; y", t.next().?.text);
    try std.testing.expect(t.next().?.start.self_closing);
    try std.testing.expectEqualStrings("<raw>", t.next().?.cdata);
    try std.testing.expectEqualStrings("p", t.next().?.end);
    const svg = t.next().?.start;
    try std.testing.expectEqualStrings("svg", svg.local());
    try std.testing.expect(t.next() == null);
}

test "attributes match exact names and class tokens" {
    const tag: Tag = .{ .name = "p", .raw = "p class=\"verse opening\" epub:type='x' data-type=\"y\"", .self_closing = false };
    try std.testing.expect(tag.hasToken("class", "verse"));
    try std.testing.expect(!tag.hasToken("class", "vers"));
    try std.testing.expectEqualStrings("x", tag.attr("epub:type").?);
    try std.testing.expectEqualStrings("y", tag.attr("data-type").?);
    try std.testing.expect(tag.attr("type") == null);
}

test "entities decode numeric and common named forms" {
    try std.testing.expectEqual(@as(u21, 0x2019), entityAt("&#x2019;", 0).?.cp);
    try std.testing.expectEqual(@as(u21, 233), entityAt("&#233;", 0).?.cp);
    try std.testing.expectEqual(@as(u21, '&'), entityAt("&amp;", 0).?.cp);
    try std.testing.expect(entityAt("& x;", 0) == null);
    try std.testing.expect(entityAt("&bogus;", 0) == null);
}
