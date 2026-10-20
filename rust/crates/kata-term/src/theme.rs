//! Application palettes. The default is the Omarchy "Ethereal" theme (ported from
//! `src/theme.zig`); Aetheria and Sedate are alternatives. Palettes are compiled in so every
//! platform gets the same look without reading any system file, and the active one can change
//! at runtime (`set`).
//!
//! Roles: `PAPER` page background, `INK` body text, `DIM` secondary text, `ACCENT`
//! pointer/titles/focus, `RULE` separators and gutters, `CODE` code blocks. `ACCENT_BG` +
//! `ON_ACCENT` form the search-match highlight. Each role is a value that prints as its escape
//! sequence for the active palette (`{PAPER}` in format strings, `.as_str()` for push_str).

use std::fmt;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicUsize, Ordering};

type Rgb = [u8; 3];

struct Spec {
    name: &'static str,
    description: &'static str,
    paper: Rgb,
    ink: Rgb,
    dim: Rgb,
    accent: Rgb,
    rule: Rgb,
    code: Rgb,
}

const SPECS: [Spec; 3] = [
    // Ethereal: background #060B1E, foreground #ffcead, muted #6d7db6, accent #7d82d9, green
    // #92a593. Rules are the midpoint of selection #252e56 and muted so they stay visible.
    Spec {
        name: "Ethereal",
        description: "deep navy with warm peach text (default)",
        paper: [6, 11, 30],
        ink: [255, 206, 173],
        dim: [109, 125, 182],
        accent: [125, 130, 217],
        rule: [73, 86, 134],
        code: [146, 165, 147],
    },
    // Aetheria (Omarchy): background #0e091d, foreground #14B9B5, accent #BE3F50, green #7cd699.
    Spec {
        name: "Aetheria",
        description: "midnight violet with teal text and crimson accents",
        paper: [14, 9, 29],
        ink: [20, 185, 181],
        dim: [82, 118, 140],
        accent: [221, 72, 96],
        rule: [58, 44, 82],
        code: [124, 214, 153],
    },
    // Sedate: low-contrast charcoal with soft greys; nothing saturated enough to tire the eye.
    Spec {
        name: "Sedate",
        description: "quiet charcoal and soft grey, low contrast",
        paper: [28, 29, 33],
        ink: [197, 200, 198],
        dim: [129, 133, 140],
        accent: [138, 164, 184],
        rule: [58, 61, 66],
        code: [154, 168, 143],
    },
];

struct Sequences {
    paper: String,
    ink: String,
    dim: String,
    accent: String,
    rule: String,
    code: String,
    accent_bg: String,
    on_accent: String,
}

fn fg([r, g, b]: Rgb) -> String {
    format!("\x1b[38;2;{r};{g};{b}m")
}

fn bg([r, g, b]: Rgb) -> String {
    format!("\x1b[48;2;{r};{g};{b}m")
}

fn sequences() -> &'static [Sequences] {
    static ALL: OnceLock<Vec<Sequences>> = OnceLock::new();
    ALL.get_or_init(|| {
        SPECS
            .iter()
            .map(|s| Sequences {
                paper: bg(s.paper),
                ink: fg(s.ink),
                dim: fg(s.dim),
                accent: fg(s.accent),
                rule: fg(s.rule),
                code: fg(s.code),
                accent_bg: bg(s.accent),
                on_accent: fg(s.paper),
            })
            .collect()
    })
}

static ACTIVE: AtomicUsize = AtomicUsize::new(0);

fn active() -> &'static Sequences {
    &sequences()[ACTIVE.load(Ordering::Relaxed).min(SPECS.len() - 1)]
}

/// Palette names, in menu order.
pub fn names() -> Vec<String> {
    SPECS.iter().map(|s| s.name.to_string()).collect()
}

/// One-line descriptions for the theme menu, in the same order as `names`.
pub fn descriptions() -> Vec<String> {
    SPECS
        .iter()
        .map(|s| format!("{} · {}", s.name, s.description))
        .collect()
}

pub fn current() -> usize {
    ACTIVE.load(Ordering::Relaxed)
}

pub fn set(index: usize) {
    ACTIVE.store(index.min(SPECS.len() - 1), Ordering::Relaxed);
}

/// Index of the palette called `name` (case-insensitive).
pub fn find(name: &str) -> Option<usize> {
    SPECS
        .iter()
        .position(|s| s.name.eq_ignore_ascii_case(name.trim()))
}

/// Menu pointer: accent arrow when selected, ink-coloured padding otherwise.
pub fn pointer(selected: bool) -> String {
    if selected {
        format!("{ACCENT}▶ ")
    } else {
        format!("{INK}  ")
    }
}

/// Search-match start/end sequences for the active palette.
pub fn mark_on() -> String {
    format!("{ACCENT_BG}{ON_ACCENT}")
}

pub fn mark_off() -> String {
    format!("{PAPER}{INK}")
}

/// A palette colour that resolves against whichever palette is active when it is used.
#[derive(Clone, Copy)]
pub struct Role(fn(&Sequences) -> &str);

impl Role {
    pub fn as_str(self) -> &'static str {
        (self.0)(active())
    }
}

impl fmt::Display for Role {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

pub const PAPER: Role = Role(|s| &s.paper);
pub const INK: Role = Role(|s| &s.ink);
pub const DIM: Role = Role(|s| &s.dim);
pub const ACCENT: Role = Role(|s| &s.accent);
pub const RULE: Role = Role(|s| &s.rule);
pub const CODE: Role = Role(|s| &s.code);
/// Search-match highlight: accent background, page-coloured text.
pub const ACCENT_BG: Role = Role(|s| &s.accent_bg);
pub const ON_ACCENT: Role = Role(|s| &s.on_accent);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color;
    use std::sync::Mutex;

    /// The active theme is global; tests that change it run one at a time.
    static THEME_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn palette_escapes_are_well_formed_and_downsample_like_zig() {
        let _guard = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        set(0);
        assert_eq!(PAPER.as_str(), "\x1b[48;2;6;11;30m");
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

    #[test]
    fn themes_switch_by_name_and_every_role_changes_with_the_palette() {
        let _guard = THEME_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        assert_eq!(names(), ["Ethereal", "Aetheria", "Sedate"]);
        assert_eq!(find(" aetheria "), Some(1));
        assert_eq!(find("nope"), None);
        set(1);
        assert_eq!(PAPER.as_str(), "\x1b[48;2;14;9;29m");
        assert_eq!(format!("{INK}"), "\x1b[38;2;20;185;181m");
        set(2);
        assert_eq!(current(), 2);
        assert_ne!(ACCENT.as_str(), ACCENT_BG.as_str());
        set(99);
        assert_eq!(current(), 2); // clamped to the last palette
        set(0);
    }
}
