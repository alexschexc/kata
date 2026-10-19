//! Terminal cell width of a code point, backed by the `unicode-width` crate (Unicode 16 tables).
//!
//! Kata-specific rules layered on the crate: control characters take one cell (the Zig
//! implementation never returned "unprintable"), and the result is capped at two cells.
//! Against the removed Zig table the crate agrees on Greek, combining marks, format characters,
//! CJK and emoji, and on every character in the embedded kjv/grb/vul corpora. It differs on
//! rare scripts (e.g. Indic vowel signs, Khmer, symbols such as U+2630) where the crate follows
//! the newer Unicode East Asian Width data; none occur in the corpora.

use unicode_width::UnicodeWidthChar;

/// Number of terminal cells `cp` occupies: 0, 1 or 2.
pub fn width(cp: u32) -> usize {
    match char::from_u32(cp) {
        // Surrogates and out-of-range values are not characters; treat them as one cell.
        None => 1,
        Some(c) => c.width().unwrap_or(1).min(2),
    }
}

/// Total cells of a string.
pub fn str_width(text: &str) -> usize {
    text.chars().map(|c| width(c as u32)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    // Ported from the Zig test "portable width covers Greek combining format CJK and emoji".
    #[test]
    fn covers_greek_combining_format_cjk_and_emoji() {
        assert_eq!(width(0x1f18), 1);
        assert_eq!(width(0x301), 0);
        assert_eq!(width(0x5b0), 0);
        assert_eq!(width(0x200d), 0);
        assert_eq!(width(0xe0100), 0);
        assert_eq!(width(0x754c), 2);
        assert_eq!(width(0x1f600), 2);
        assert_eq!(width('a' as u32), 1);
    }

    #[test]
    fn controls_and_non_characters_take_one_cell() {
        assert_eq!(width(0), 1);
        assert_eq!(width(0x1b), 1);
        assert_eq!(width(0x7f), 1);
        assert_eq!(width(0xd800), 1);
        assert_eq!(width(0x11_0000), 1);
    }

    #[test]
    fn string_width_sums_cells() {
        assert_eq!(str_width("Λόγος"), 5);
        assert_eq!(str_width("e\u{301}"), 1);
        assert_eq!(str_width("界a"), 3);
    }
}
