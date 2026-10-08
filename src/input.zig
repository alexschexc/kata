const std = @import("std");

/// Incremental, allocation-free terminal framing. Only plain supported keys
/// reach bindings; escape/control-string/UTF-8 payloads never become commands.
/// An incomplete paste stays quarantined until its closing marker (or exit).
pub const Decoder = struct {
    const Mode = enum { plain, escape, intermediate, csi, ss3, string, string_escape, unicode, paste };
    mode: Mode = .plain,
    remaining: u3 = 0,
    csi_length: usize = 0,
    paste_start: bool = false,
    paste_match: usize = 0,

    pub fn feed(self: *Decoder, byte: u8) ?u8 {
        if (byte == 27 and (self.mode == .csi or self.mode == .ss3 or self.mode == .intermediate)) {
            self.mode = .escape;
            return null;
        }
        switch (self.mode) {
            .plain => {
                if (byte == 27) self.mode = .escape else if (byte >= 0x80) {
                    // Also frame 8-bit CSI/SS3/control strings, not just UTF-8.
                    switch (byte) {
                        0x9b => self.beginCsi(),
                        0x8f => self.mode = .ss3,
                        0x90, 0x9d, 0x9e, 0x9f => self.mode = .string,
                        else => if (std.unicode.utf8ByteSequenceLength(byte)) |length| {
                            self.remaining = @intCast(length - 1);
                            self.mode = .unicode;
                        } else |_| {},
                    }
                } else if (recognized(byte)) return byte;
            },
            .escape => switch (byte) {
                '[' => self.beginCsi(),
                'O' => self.mode = .ss3,
                ']', 'P', '^', '_' => self.mode = .string,
                0x20...0x2f => self.mode = .intermediate,
                else => {
                    // Alt-key events, including Alt+Unicode, are inert.
                    self.mode = .plain;
                    if (byte >= 0xc2) {
                        const length = std.unicode.utf8ByteSequenceLength(byte) catch return null;
                        self.remaining = @intCast(length - 1);
                        self.mode = .unicode;
                    }
                },
            },
            .intermediate, .ss3 => {
                if (byte >= 0x40 and byte <= 0x7e) self.mode = .plain;
            },
            .csi => {
                // Only an exact CSI 200~ starts paste. Saturating count bounds
                // memory and arithmetic even for arbitrarily long sequences.
                const marker = "200";
                if (byte >= 0x40 and byte <= 0x7e) {
                    self.mode = if (byte == '~' and self.paste_start and self.csi_length == marker.len) .paste else .plain;
                    self.paste_match = 0;
                } else {
                    if (self.csi_length >= marker.len or byte != marker[@min(self.csi_length, marker.len - 1)]) self.paste_start = false;
                    self.csi_length +|= 1;
                }
            },
            .string => {
                if (byte == 7 or byte == 0x9c) self.mode = .plain else if (byte == 27) self.mode = .string_escape;
            },
            .string_escape => {
                self.mode = if (byte == '\\') .plain else if (byte == 27) .string_escape else .string;
            },
            .unicode => {
                // Consume the expected width even when malformed: an ASCII
                // byte masquerading as a continuation must not execute.
                self.remaining -= 1;
                if (self.remaining == 0) self.mode = .plain;
            },
            .paste => {
                const end = "\x1b[201~";
                if (byte == end[self.paste_match]) {
                    self.paste_match += 1;
                    if (self.paste_match == end.len) {
                        self.mode = .plain;
                        self.paste_match = 0;
                    }
                } else self.paste_match = if (byte == 27) 1 else 0;
            },
        }
        return null;
    }

    fn beginCsi(self: *Decoder) void {
        self.mode = .csi;
        self.csi_length = 0;
        self.paste_start = true;
    }

    /// Called only after an idle interbyte deadline. Lone ESC cancels;
    /// incomplete sequences are discarded, never emitted as an ESC event.
    pub fn timeout(self: *Decoder) ?u8 {
        const lone_escape = self.mode == .escape;
        if (self.mode != .paste) self.* = .{};
        return if (lone_escape) 27 else null;
    }
};

pub fn recognized(byte: u8) bool {
    return switch (byte) {
        3, 4, 8, 9, 10, 13, 21, 27, 127, '0'...'9', 'j', 'k', 'h', 'l', 'n', 'y', 'c', 'q', 'm', 'o', '[', ']', 'f', 'b', 'g', 'G', 's', 'p', 'd' => true,
        else => false,
    };
}

test "timeouts distinguish lone Escape from malformed sequences and preserve paste quarantine" {
    var decoder: Decoder = .{};
    _ = decoder.feed(27);
    try std.testing.expectEqual(@as(?u8, 27), decoder.timeout());
    for ("\x1b[123;") |byte| _ = decoder.feed(byte);
    try std.testing.expect(decoder.timeout() == null);
    try std.testing.expectEqual(@as(?u8, 'k'), decoder.feed('k'));
    for ("\x1b[200~") |byte| _ = decoder.feed(byte);
    try std.testing.expect(decoder.timeout() == null);
    for ("qcy\r\n\x1b[201~") |byte| try std.testing.expect(decoder.feed(byte) == null);
    try std.testing.expectEqual(@as(?u8, 'y'), decoder.feed('y'));
}

test "unknown control and Unicode bytes are inert including malformed continuation commands" {
    var decoder: Decoder = .{};
    for ("@!?\x00\x01\x02\x05\x06\x7e\xff\xfe\x80\xc3\xa9\xf0\x9f\x98\x80\xc3q\xf0cyq\x1bq\x1b\xc3\xa9") |byte| {
        try std.testing.expect(decoder.feed(byte) == null);
    }
    try std.testing.expectEqual(@as(?u8, 'c'), decoder.feed('c'));
}

test "overlong sequences need no buffer and retain framing until their final" {
    var decoder: Decoder = .{};
    _ = decoder.feed(27);
    _ = decoder.feed('[');
    for (0..100_000) |_| try std.testing.expect(decoder.feed('1') == null);
    try std.testing.expect(decoder.feed('q') == null);
    try std.testing.expectEqual(@as(?u8, 'j'), decoder.feed('j'));
}

test "nested escapes resynchronize without leaking a command final" {
    var decoder: Decoder = .{};
    for ("\x1b[12;\x1b[q\x1bO\x1bOq") |byte| try std.testing.expect(decoder.feed(byte) == null);
    try std.testing.expectEqual(@as(?u8, 'j'), decoder.feed('j'));
}

test "terminal sequences never dispatch their reserved payload bytes" {
    var decoder: Decoder = .{};
    for ("\x1b[A\x1bOq\x1b[1;5q\x1b[27;5;121~\x1b[200~qcy123\r\n[]\x1b[201~\x1b]0;q\x07\x1bPq\x1b\\") |byte| {
        try std.testing.expect(decoder.feed(byte) == null);
    }
    try std.testing.expectEqual(@as(?u8, 'j'), decoder.feed('j'));
}
