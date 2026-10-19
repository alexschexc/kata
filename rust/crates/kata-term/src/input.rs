//! Incremental, allocation-free terminal input framing, ported from `src/input.zig`. Only plain
//! supported keys reach bindings; escape/control-string/UTF-8 payloads never become commands.
//! An incomplete paste stays quarantined until its closing marker (or exit).

use kata_core::keys::recognized;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Mode {
    #[default]
    Plain,
    Escape,
    Intermediate,
    Csi,
    Ss3,
    String,
    StringEscape,
    Unicode,
    Paste,
}

#[derive(Debug, Clone, Default)]
pub struct Decoder {
    mode: Mode,
    remaining: u8,
    csi_length: usize,
    paste_start: bool,
    paste_match: usize,
    /// Text entry (search prompt): printable ASCII and UTF-8 bytes are delivered too.
    /// Escape/control-string/paste framing is unchanged.
    pub text: bool,
    /// CSI parameter bytes, for recognising arrow-key sequences.
    params: [u8; 8],
    params_len: usize,
}

fn utf8_len(lead: u8) -> Option<u8> {
    match lead {
        0x00..=0x7f => Some(1),
        // Matches Zig's utf8ByteSequenceLength: overlong/out-of-range leads still frame a
        // sequence of that length so their continuation bytes stay inert.
        0xc0..=0xdf => Some(2),
        0xe0..=0xef => Some(3),
        0xf0..=0xf7 => Some(4),
        _ => None,
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    pub fn feed(&mut self, byte: u8) -> Option<u8> {
        if byte == 27 && matches!(self.mode, Mode::Csi | Mode::Ss3 | Mode::Intermediate) {
            self.mode = Mode::Escape;
            return None;
        }
        match self.mode {
            Mode::Plain => {
                if byte == 27 {
                    self.mode = Mode::Escape;
                } else if byte >= 0x80 {
                    // Also frame 8-bit CSI/SS3/control strings, not just UTF-8.
                    match byte {
                        0x9b => self.begin_csi(),
                        0x8f => self.mode = Mode::Ss3,
                        0x90 | 0x9d | 0x9e | 0x9f => self.mode = Mode::String,
                        _ => {
                            if let Some(length) = utf8_len(byte) {
                                self.remaining = length - 1;
                                self.mode = Mode::Unicode;
                                if self.text {
                                    return Some(byte);
                                }
                            }
                        }
                    }
                } else if recognized(byte) || (self.text && (0x20..0x7f).contains(&byte)) {
                    return Some(byte);
                }
            }
            Mode::Escape => match byte {
                b'[' => self.begin_csi(),
                b'O' => self.mode = Mode::Ss3,
                b']' | b'P' | b'^' | b'_' => self.mode = Mode::String,
                0x20..=0x2f => self.mode = Mode::Intermediate,
                _ => {
                    // Alt-key events, including Alt+Unicode, are inert.
                    self.mode = Mode::Plain;
                    if byte >= 0xc2 {
                        let length = utf8_len(byte)?;
                        self.remaining = length - 1;
                        self.mode = Mode::Unicode;
                    }
                }
            },
            Mode::Intermediate => {
                if (0x40..=0x7e).contains(&byte) {
                    self.mode = Mode::Plain;
                }
            }
            Mode::Ss3 => {
                if (0x40..=0x7e).contains(&byte) {
                    self.mode = Mode::Plain;
                    return self.arrow(byte, false);
                }
            }
            Mode::Csi => {
                // Only an exact CSI 200~ starts paste. The saturating count bounds memory and
                // arithmetic even for arbitrarily long sequences.
                const MARKER: &[u8] = b"200";
                if (0x40..=0x7e).contains(&byte) {
                    self.mode =
                        if byte == b'~' && self.paste_start && self.csi_length == MARKER.len() {
                            Mode::Paste
                        } else {
                            Mode::Plain
                        };
                    self.paste_match = 0;
                    if self.mode == Mode::Plain {
                        let params = &self.params[..self.params_len];
                        // Plain arrows (CSI A, CSI 1A) and Shift+arrows (CSI 1;2A).
                        if self.csi_length == params.len() && (params.is_empty() || params == b"1")
                        {
                            return self.arrow(byte, false);
                        }
                        if self.csi_length == params.len() && params == b"1;2" {
                            return self.arrow(byte, true);
                        }
                    }
                } else {
                    if self.params_len < self.params.len() {
                        self.params[self.params_len] = byte;
                        self.params_len += 1;
                    }
                    if self.csi_length >= MARKER.len()
                        || byte != MARKER[self.csi_length.min(MARKER.len() - 1)]
                    {
                        self.paste_start = false;
                    }
                    self.csi_length = self.csi_length.saturating_add(1);
                }
            }
            Mode::String => {
                if byte == 7 || byte == 0x9c {
                    self.mode = Mode::Plain;
                } else if byte == 27 {
                    self.mode = Mode::StringEscape;
                }
            }
            Mode::StringEscape => {
                self.mode = if byte == b'\\' {
                    Mode::Plain
                } else if byte == 27 {
                    Mode::StringEscape
                } else {
                    Mode::String
                };
            }
            Mode::Unicode => {
                // Consume the expected width even when malformed: an ASCII byte masquerading as
                // a continuation must not execute.
                self.remaining -= 1;
                if self.remaining == 0 {
                    self.mode = Mode::Plain;
                }
                if self.text && (byte & 0xc0) == 0x80 {
                    return Some(byte);
                }
            }
            Mode::Paste => {
                const END: &[u8] = b"\x1b[201~";
                if byte == END[self.paste_match] {
                    self.paste_match += 1;
                    if self.paste_match == END.len() {
                        self.mode = Mode::Plain;
                        self.paste_match = 0;
                    }
                } else {
                    self.paste_match = usize::from(byte == 27);
                    // Text entry accepts pasted characters; newlines and controls stay inert so
                    // a paste never submits.
                    if self.text && self.paste_match == 0 && byte >= 0x20 && byte != 0x7f {
                        return Some(byte);
                    }
                }
            }
        }
        None
    }

    fn begin_csi(&mut self) {
        self.mode = Mode::Csi;
        self.csi_length = 0;
        self.params_len = 0;
        self.paste_start = true;
    }

    /// Arrow keys map to their Vim keys (h/j/k/l); Shift+Left/Right map to H/L (previous/next
    /// chapter). Inert during text entry.
    fn arrow(&self, final_byte: u8, shift: bool) -> Option<u8> {
        if self.text {
            return None;
        }
        match final_byte {
            b'A' if !shift => Some(b'k'),
            b'B' if !shift => Some(b'j'),
            b'C' => Some(if shift { b'L' } else { b'l' }),
            b'D' => Some(if shift { b'H' } else { b'h' }),
            _ => None,
        }
    }

    /// Called only after an idle interbyte deadline. Lone ESC cancels; incomplete sequences are
    /// discarded, never emitted as an ESC event.
    pub fn timeout(&mut self) -> Option<u8> {
        let lone_escape = self.mode == Mode::Escape;
        // Reset framing only; text entry is a caller mode, not decoder state.
        if self.mode != Mode::Paste {
            *self = Decoder {
                text: self.text,
                ..Decoder::default()
            };
        }
        lone_escape.then_some(27)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed_all(decoder: &mut Decoder, bytes: &[u8]) -> Vec<u8> {
        bytes.iter().filter_map(|&b| decoder.feed(b)).collect()
    }

    #[test]
    fn text_mode_delivers_printable_and_utf8_bytes_but_keeps_sequences_inert() {
        let mut decoder = Decoder {
            text: true,
            ..Decoder::default()
        };
        assert_eq!(
            feed_all(&mut decoder, b"a Z\x1b[A\xce\xbb\x1b]0;x\x07!"),
            b"a Z\xce\xbb!"
        );
        decoder.text = false;
        assert_eq!(decoder.feed(b'a'), None);
        assert_eq!(decoder.feed(0xce), None);
        assert_eq!(decoder.feed(0xbb), None);
    }

    #[test]
    fn text_mode_accepts_pasted_characters_but_never_pasted_controls() {
        let mut decoder = Decoder {
            text: true,
            ..Decoder::default()
        };
        assert_eq!(
            feed_all(&mut decoder, b"\x1b[200~/home/a b\r\n\x1b[201~q"),
            b"/home/a bq"
        );
    }

    #[test]
    fn idle_timeouts_keep_text_entry_mode() {
        let mut decoder = Decoder {
            text: true,
            ..Decoder::default()
        };
        decoder.timeout();
        assert_eq!(decoder.feed(b'e'), Some(b'e'));
        decoder.feed(27);
        assert_eq!(decoder.timeout(), Some(27));
        assert!(decoder.text);
    }

    #[test]
    fn arrow_keys_map_to_vim_keys_and_shift_arrows_to_chapter_keys() {
        let mut decoder = Decoder::new();
        let input = b"\x1b[A\x1b[B\x1b[C\x1b[D\x1bOA\x1b[1B\x1b[1;2D\x1b[1;2C\x1b[1;2A\x1b[1;5C\x1b[5~\x9b\x41";
        assert_eq!(feed_all(&mut decoder, input), b"kjlhkjHLk");
        decoder.text = true;
        for &b in b"\x1b[A\x1b[1;2D" {
            assert_eq!(decoder.feed(b), None);
        }
    }

    #[test]
    fn timeouts_distinguish_lone_escape_from_malformed_sequences_and_preserve_paste_quarantine() {
        let mut decoder = Decoder::new();
        decoder.feed(27);
        assert_eq!(decoder.timeout(), Some(27));
        feed_all(&mut decoder, b"\x1b[123;");
        assert_eq!(decoder.timeout(), None);
        assert_eq!(decoder.feed(b'k'), Some(b'k'));
        feed_all(&mut decoder, b"\x1b[200~");
        assert_eq!(decoder.timeout(), None);
        assert!(feed_all(&mut decoder, b"qcy\r\n\x1b[201~").is_empty());
        assert_eq!(decoder.feed(b'y'), Some(b'y'));
    }

    #[test]
    fn unknown_control_and_unicode_bytes_are_inert_including_malformed_continuation_commands() {
        let mut decoder = Decoder::new();
        let noise = b"@!?\x00\x01\x02\x05\x06\x7e\xff\xfe\x80\xc3\xa9\xf0\x9f\x98\x80\xc3q\xf0cyq\x1bq\x1b\xc3\xa9";
        assert!(feed_all(&mut decoder, noise).is_empty());
        assert_eq!(decoder.feed(b'c'), Some(b'c'));
    }

    #[test]
    fn invalid_lead_bytes_still_frame_their_continuations() {
        // Zig frames 0xC0/0xC1 (2 bytes) and 0xF5..0xF7 (4 bytes); the following ASCII bytes are
        // consumed as the sequence body and must not dispatch.
        for lead in [0xc0u8, 0xc1, 0xf5, 0xf7] {
            let mut decoder = Decoder::new();
            let body: &[u8] = if lead >= 0xf0 { b"qcy" } else { b"q" };
            assert_eq!(decoder.feed(lead), None);
            for &b in body {
                assert_eq!(decoder.feed(b), None, "lead {lead:#x}");
            }
            assert_eq!(decoder.feed(b'j'), Some(b'j'));
        }
        // Leads at or above 0xF8 frame nothing.
        let mut decoder = Decoder::new();
        assert_eq!(decoder.feed(0xf8), None);
        assert_eq!(decoder.feed(b'j'), Some(b'j'));
    }

    #[test]
    fn overlong_sequences_need_no_buffer_and_retain_framing_until_their_final() {
        let mut decoder = Decoder::new();
        decoder.feed(27);
        decoder.feed(b'[');
        for _ in 0..100_000 {
            assert_eq!(decoder.feed(b'1'), None);
        }
        assert_eq!(decoder.feed(b'q'), None);
        assert_eq!(decoder.feed(b'j'), Some(b'j'));
    }

    #[test]
    fn nested_escapes_resynchronize_without_leaking_a_command_final() {
        let mut decoder = Decoder::new();
        assert!(feed_all(&mut decoder, b"\x1b[12;\x1b[q\x1bO\x1bOq").is_empty());
        assert_eq!(decoder.feed(b'j'), Some(b'j'));
    }

    #[test]
    fn terminal_sequences_never_dispatch_their_reserved_payload_bytes() {
        let mut decoder = Decoder::new();
        let input = b"\x1b[1;5A\x1bOq\x1b[1;5q\x1b[27;5;121~\x1b[200~qcy123\r\n[]\x1b[201~\x1b]0;q\x07\x1bPq\x1b\\";
        assert!(feed_all(&mut decoder, input).is_empty());
        assert_eq!(decoder.feed(b'j'), Some(b'j'));
    }
}
