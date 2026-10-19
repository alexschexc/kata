//! Colour depth detection and the 24-bit to 256-colour downsample, ported from `src/tui.zig`
//! (`detectColorDepth`) and `src/theme.zig` (`rgbTo256`, `downsample`).
//!
//! macOS Terminal.app does not understand 24-bit colour: unconverted theme escapes render as a
//! solid red screen (the v0.3 bug). `KATA_COLORS=256|truecolor` overrides detection.

/// True when output must be rewritten to the 256-colour palette. `env` looks up a variable.
/// Windows consoles always take truecolor, so detection is skipped there.
pub fn detect_256(env: impl Fn(&str) -> Option<String>, windows: bool) -> bool {
    if windows {
        return false;
    }
    match env("KATA_COLORS").as_deref() {
        Some("256") => return true,
        Some("truecolor") => return false,
        _ => {}
    }
    if matches!(env("COLORTERM").as_deref(), Some("truecolor" | "24bit")) {
        return false;
    }
    env("TERM_PROGRAM").as_deref() == Some("Apple_Terminal")
}

/// Nearest xterm-256 palette index (6x6x6 cube or 24-step gray ramp).
pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let rgb = [r as i32, g as i32, b as i32];
    let mut idx = [0usize; 3];
    let mut cube_err = 0;
    for (k, &v) in rgb.iter().enumerate() {
        let mut best = 0;
        for (i, &l) in LEVELS.iter().enumerate() {
            if (l - v).abs() < (LEVELS[best] - v).abs() {
                best = i;
            }
        }
        idx[k] = best;
        let d = LEVELS[best] - v;
        cube_err += d * d;
    }
    let avg = (rgb[0] + rgb[1] + rgb[2]) / 3;
    let step = ((avg - 3) / 10).clamp(0, 23);
    let gray = 8 + step * 10;
    let gray_err: i32 = rgb.iter().map(|&v| (gray - v) * (gray - v)).sum();
    if gray_err < cube_err {
        return (232 + step) as u8;
    }
    (16 + 36 * idx[0] + 6 * idx[1] + idx[2]) as u8
}

/// Rewrites `ESC[38;2;R;G;Bm` / `ESC[48;2;R;G;Bm` in `bytes` to `ESC[38;5;Nm` / `ESC[48;5;Nm`.
/// Everything else, including malformed or out-of-range sequences, passes through unchanged.
pub fn downsample(bytes: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(bytes.len());
    let (mut i, mut start) = (0, 0);
    while i < bytes.len() {
        if bytes[i] == 0x1b
            && i + 7 <= bytes.len()
            && bytes[i + 1] == b'['
            && (bytes[i + 2] == b'3' || bytes[i + 2] == b'4')
            && bytes[i + 3] == b'8'
            && bytes[i + 4] == b';'
            && bytes[i + 5] == b'2'
            && bytes[i + 6] == b';'
        {
            let mut vals = [0u16; 3];
            let mut j = i + 7;
            let mut ok = true;
            for (n, slot) in vals.iter_mut().enumerate() {
                let (mut v, mut digits) = (0u16, 0);
                while j < bytes.len() && bytes[j].is_ascii_digit() {
                    v = v
                        .saturating_mul(10)
                        .saturating_add((bytes[j] - b'0') as u16);
                    digits += 1;
                    j += 1;
                }
                let want = if n == 2 { b'm' } else { b';' };
                if digits == 0 || v > 255 || j >= bytes.len() || bytes[j] != want {
                    ok = false;
                    break;
                }
                *slot = v;
                j += 1;
            }
            if ok {
                out.extend_from_slice(&bytes[start..i]);
                let index = rgb_to_256(vals[0] as u8, vals[1] as u8, vals[2] as u8);
                out.extend_from_slice(
                    format!("\x1b[{}8;5;{index}m", bytes[i + 2] as char).as_bytes(),
                );
                i = j;
                start = j;
                continue;
            }
        }
        i += 1;
    }
    out.extend_from_slice(&bytes[start..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
        let map: HashMap<String, String> = pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        move |name| map.get(name).cloned()
    }

    #[test]
    fn downsample_maps_truecolor_to_256_and_leaves_other_bytes_alone() {
        // paper = 48;2;6;11;30, ink = 38;2;255;206;173 (src/theme.zig): the Zig test expects 233 and 223.
        let input = "a\x1b[1m\x1b[48;2;6;11;30m\x1b[38;2;255;206;173m┃x\x1b[38;2;999;1;1m";
        let expected = "a\x1b[1m\x1b[48;5;233m\x1b[38;5;223m┃x\x1b[38;2;999;1;1m";
        assert_eq!(downsample(input.as_bytes()), expected.as_bytes());
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
    }

    #[test]
    fn truncated_and_malformed_sequences_pass_through() {
        for s in [
            "\x1b[38;2;1;2",
            "\x1b[38;2;1;2;",
            "\x1b[38;2;;1;2m",
            "\x1b[38;2;1;2;3x",
            "\x1b[38;5;7m",
            "plain",
        ] {
            assert_eq!(downsample(s.as_bytes()), s.as_bytes(), "{s:?}");
        }
        assert_eq!(downsample(b""), b"");
    }

    #[test]
    fn no_truecolor_sequence_survives_downsampling() {
        let theme = "\x1b[48;2;125;130;217m\x1b[38;2;6;11;30m\x1b[38;2;146;165;147m";
        let out = downsample(theme.as_bytes());
        assert!(!String::from_utf8_lossy(&out).contains(";2;"));
    }

    #[test]
    fn apple_terminal_without_truecolor_gets_256_colours() {
        assert!(detect_256(
            env(&[("TERM_PROGRAM", "Apple_Terminal")]),
            false
        ));
        assert!(!detect_256(
            env(&[
                ("TERM_PROGRAM", "Apple_Terminal"),
                ("COLORTERM", "truecolor")
            ]),
            false
        ));
        assert!(!detect_256(
            env(&[("TERM_PROGRAM", "Apple_Terminal"), ("COLORTERM", "24bit")]),
            false
        ));
        assert!(!detect_256(env(&[("TERM_PROGRAM", "ghostty")]), false));
        assert!(!detect_256(env(&[]), false));
    }

    #[test]
    fn kata_colors_overrides_detection() {
        assert!(detect_256(env(&[("KATA_COLORS", "256")]), false));
        assert!(!detect_256(
            env(&[
                ("KATA_COLORS", "truecolor"),
                ("TERM_PROGRAM", "Apple_Terminal")
            ]),
            false
        ));
        // An unrecognised value falls back to detection.
        assert!(detect_256(
            env(&[("KATA_COLORS", "weird"), ("TERM_PROGRAM", "Apple_Terminal")]),
            false
        ));
        assert!(!detect_256(
            env(&[("KATA_COLORS", "256"), ("TERM_PROGRAM", "Apple_Terminal")]),
            true
        ));
    }
}
