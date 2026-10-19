//! Application palette, ported from `src/theme.zig`. Defaults are the Omarchy "Ethereal" theme,
//! compiled in so every platform gets the same look without reading any system file.
//!
//! Roles: `PAPER` page background, `INK` body text, `DIM` secondary text, `ACCENT`
//! pointer/titles/focus, `RULE` separators and gutters, `CODE` code blocks. `ACCENT_BG` +
//! `ON_ACCENT` form the search-match highlight.

/// Ethereal: background #060B1E.
pub const PAPER: &str = "\x1b[48;2;6;11;30m";
/// Ethereal: foreground #ffcead.
pub const INK: &str = "\x1b[38;2;255;206;173m";
/// Ethereal: muted #6d7db6.
pub const DIM: &str = "\x1b[38;2;109;125;182m";
/// Ethereal: accent #7d82d9.
pub const ACCENT: &str = "\x1b[38;2;125;130;217m";
/// Midpoint of Ethereal selection #252e56 and muted #6d7db6, so rules stay visible on the dark
/// background.
pub const RULE: &str = "\x1b[38;2;73;86;134m";
/// Ethereal: green #92a593.
pub const CODE: &str = "\x1b[38;2;146;165;147m";
/// Search-match highlight: accent background, page-coloured text.
pub const ACCENT_BG: &str = "\x1b[48;2;125;130;217m";
pub const ON_ACCENT: &str = "\x1b[38;2;6;11;30m";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color;

    #[test]
    fn palette_escapes_are_well_formed_and_downsample_like_zig() {
        assert_eq!(PAPER, "\x1b[48;2;6;11;30m");
        let styled = format!("a\x1b[1m{PAPER}{INK}┃x\x1b[38;2;999;1;1m");
        assert_eq!(
            color::downsample(styled.as_bytes()),
            "a\x1b[1m\x1b[48;5;233m\x1b[38;5;223m┃x\x1b[38;2;999;1;1m".as_bytes()
        );
        assert_eq!(
            format!("{ACCENT_BG}{ON_ACCENT}"),
            "\x1b[48;2;125;130;217m\x1b[38;2;6;11;30m"
        );
    }
}
