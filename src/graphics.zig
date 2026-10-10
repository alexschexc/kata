//! Terminal graphics capability detection (inline images, stage 3).
//! Asks the terminal itself instead of trusting TERM: a Kitty graphics query
//! plus Primary Device Attributes (DA1), which every terminal answers, so the
//! DA1 reply also ends the wait. Cell pixel size comes from XTWINOPS 16.
const std = @import("std");

pub const Protocol = enum { none, sixel, kitty };

pub const Capabilities = struct {
    protocol: Protocol = .none,
    /// Character cell size in pixels (0 when the terminal did not say).
    cell_width: u16 = 0,
    cell_height: u16 = 0,
    /// Text area size in pixels (XTWINOPS 14), used when 16 is unsupported.
    area_width: u16 = 0,
    area_height: u16 = 0,
    /// The terminal answered at all (DA1 seen before the timeout).
    answered: bool = false,

    /// Cell size from the window-size ioctl. This is the unit sixel pixels
    /// are drawn in; foot at fractional scaling reports a different (scaled)
    /// cell size via CSI 16t, which made images overflow their rows.
    pub fn withWindowPixels(self: Capabilities, columns: usize, rows: usize, pixel_width: usize, pixel_height: usize) Capabilities {
        var caps = self;
        if (columns > 0 and rows > 0 and pixel_width >= columns and pixel_height >= rows) {
            caps.cell_width = @intCast(pixel_width / columns);
            caps.cell_height = @intCast(pixel_height / rows);
        }
        return caps;
    }

    /// Derives the cell size from the text area when only that was reported.
    pub fn withCellsFrom(self: Capabilities, columns: usize, rows: usize) Capabilities {
        var caps = self;
        if ((caps.cell_width == 0 or caps.cell_height == 0) and caps.area_width > 0 and caps.area_height > 0 and columns > 0 and rows > 0) {
            caps.cell_width = @intCast(caps.area_width / columns);
            caps.cell_height = @intCast(caps.area_height / rows);
        }
        return caps;
    }

    pub fn describe(self: Capabilities) []const u8 {
        return switch (self.protocol) {
            .kitty => "kitty graphics",
            .sixel => "sixel graphics",
            .none => if (self.answered) "no inline image support" else "terminal did not answer",
        };
    }
};

/// Kitty query (1×1 RGB, query action), cell size, then DA1 last.
pub const query = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\" ++ "\x1b[16t" ++ "\x1b[14t" ++ "\x1b[c";

/// Parses everything the terminal sent back. Returns null until the DA1
/// reply (`ESC [ ? … c`) has arrived, i.e. while more replies may follow.
pub fn parse(reply: []const u8) ?Capabilities {
    var caps: Capabilities = .{};
    var kitty = false;
    var da1 = false;
    var at: usize = 0;
    while (std.mem.indexOfScalarPos(u8, reply, at, 0x1b)) |esc| {
        at = esc + 1;
        if (esc + 2 > reply.len) break;
        switch (reply[esc + 1]) {
            '_' => { // APC: kitty graphics response "Gi=31;OK"
                const end = std.mem.indexOfPos(u8, reply, esc, "\x1b\\") orelse break;
                const body = reply[esc + 2 .. end];
                if (std.mem.startsWith(u8, body, "G") and std.mem.indexOf(u8, body, "i=31") != null and std.mem.endsWith(u8, body, ";OK")) kitty = true;
                at = end + 2;
            },
            '[' => {
                var i = esc + 2;
                while (i < reply.len and (reply[i] < 0x40 or reply[i] > 0x7e)) i += 1;
                if (i >= reply.len) break;
                const params = reply[esc + 2 .. i];
                const final = reply[i];
                at = i + 1;
                if (final == 'c' and params.len > 0 and params[0] == '?') {
                    da1 = true;
                    var fields = std.mem.splitScalar(u8, params[1..], ';');
                    _ = fields.next(); // device class
                    while (fields.next()) |field| if (std.mem.eql(u8, field, "4")) {
                        if (caps.protocol == .none) caps.protocol = .sixel;
                    };
                } else if (final == 't') {
                    var fields = std.mem.splitScalar(u8, params, ';');
                    const kind = fields.next() orelse "";
                    const h = std.fmt.parseInt(u16, fields.next() orelse "", 10) catch 0;
                    const w = std.fmt.parseInt(u16, fields.next() orelse "", 10) catch 0;
                    if (std.mem.eql(u8, kind, "6")) {
                        caps.cell_height = h;
                        caps.cell_width = w;
                    } else if (std.mem.eql(u8, kind, "4")) {
                        caps.area_height = h;
                        caps.area_width = w;
                    }
                }
            },
            else => {},
        }
    }
    if (!da1) return null;
    caps.answered = true;
    if (kitty) caps.protocol = .kitty; // preferred: sends PNG bytes directly
    return caps;
}

test "foot-style reply: sixel via DA1 attribute 4 and cell size" {
    const caps = parse("\x1b[6;20;10t\x1b[?62;4;22c").?;
    try std.testing.expectEqual(Protocol.sixel, caps.protocol);
    try std.testing.expectEqual(@as(u16, 10), caps.cell_width);
    try std.testing.expectEqual(@as(u16, 20), caps.cell_height);
}

test "cell size falls back to text area divided by the grid" {
    const caps = parse("\x1b[4;900;1500t\x1b[?62;4c").?.withCellsFrom(150, 45);
    try std.testing.expectEqual(@as(u16, 10), caps.cell_width);
    try std.testing.expectEqual(@as(u16, 20), caps.cell_height);
}

test "window pixel size overrides the scaled CSI 16t cell size" {
    // Real foot at scale 1.6: CSI 16t says 13x35, the ioctl says 1830x1064 for 183x38.
    const caps = parse("\x1b[6;35;13t\x1b[?62;4;22;28;52c").?.withWindowPixels(183, 38, 1830, 1064);
    try std.testing.expectEqual(@as(u16, 10), caps.cell_width);
    try std.testing.expectEqual(@as(u16, 28), caps.cell_height);
    const unknown = parse("\x1b[6;35;13t\x1b[?62;4c").?.withWindowPixels(183, 38, 0, 0);
    try std.testing.expectEqual(@as(u16, 13), unknown.cell_width);
}

test "kitty reply wins over sixel; no DA1 means keep waiting" {
    try std.testing.expect(parse("\x1b_Gi=31;OK\x1b\\") == null);
    try std.testing.expectEqual(Protocol.kitty, parse("\x1b_Gi=31;OK\x1b\\\x1b[?62;4c").?.protocol);
    try std.testing.expectEqual(Protocol.none, parse("\x1b_Gi=31;ENOENT:x\x1b\\\x1b[?1;2c").?.protocol);
    try std.testing.expectEqual(Protocol.none, parse("\x1b[?62;22c").?.protocol);
}
