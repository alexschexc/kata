//! Terminal graphics capability detection, ported from `src/graphics.zig`. Asks the terminal
//! itself instead of trusting TERM: a Kitty graphics query plus Primary Device Attributes (DA1),
//! which every terminal answers, so the DA1 reply also ends the wait. Cell pixel size comes from
//! XTWINOPS 16.

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Protocol {
    #[default]
    None,
    Sixel,
    Kitty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Capabilities {
    pub protocol: Protocol,
    /// Character cell size in pixels (0 when the terminal did not say).
    pub cell_width: u16,
    pub cell_height: u16,
    /// Text area size in pixels (XTWINOPS 14), used when 16 is unsupported.
    pub area_width: u16,
    pub area_height: u16,
    /// The terminal answered at all (DA1 seen before the timeout).
    pub answered: bool,
}

fn to_u16(value: usize) -> u16 {
    u16::try_from(value).unwrap_or(u16::MAX)
}

impl Capabilities {
    /// Cell size from the window-size ioctl. This is the unit sixel pixels are drawn in; foot at
    /// fractional scaling reports a different (scaled) cell size via CSI 16t, which made images
    /// overflow their rows.
    pub fn with_window_pixels(
        self,
        columns: usize,
        rows: usize,
        pixel_width: usize,
        pixel_height: usize,
    ) -> Capabilities {
        let mut caps = self;
        if columns > 0 && rows > 0 && pixel_width >= columns && pixel_height >= rows {
            caps.cell_width = to_u16(pixel_width / columns);
            caps.cell_height = to_u16(pixel_height / rows);
        }
        caps
    }

    /// Derives the cell size from the text area when only that was reported.
    pub fn with_cells_from(self, columns: usize, rows: usize) -> Capabilities {
        let mut caps = self;
        if (caps.cell_width == 0 || caps.cell_height == 0)
            && caps.area_width > 0
            && caps.area_height > 0
            && columns > 0
            && rows > 0
        {
            caps.cell_width = to_u16(usize::from(caps.area_width) / columns);
            caps.cell_height = to_u16(usize::from(caps.area_height) / rows);
        }
        caps
    }

    pub fn describe(&self) -> &'static str {
        match self.protocol {
            Protocol::Kitty => "kitty graphics",
            Protocol::Sixel => "sixel graphics",
            Protocol::None if self.answered => "no inline image support",
            Protocol::None => "terminal did not answer",
        }
    }
}

/// Kitty query (1×1 RGB, query action), cell size, then DA1 last.
pub const QUERY: &str = "\x1b_Gi=31,s=1,v=1,a=q,t=d,f=24;AAAA\x1b\\\x1b[16t\x1b[14t\x1b[c";

fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

/// Zig `parseInt(u16, …) catch 0`: optional `+`, digits, `_` separators; overflow is 0.
fn parse_u16(text: &[u8]) -> u16 {
    let digits = text.strip_prefix(b"+").unwrap_or(text);
    if digits.is_empty() || digits[0] == b'_' || digits[digits.len() - 1] == b'_' {
        return 0;
    }
    let mut value: u32 = 0;
    for &c in digits {
        if c == b'_' {
            continue;
        }
        if !c.is_ascii_digit() {
            return 0;
        }
        value = value * 10 + u32::from(c - b'0');
        if value > u32::from(u16::MAX) {
            return 0;
        }
    }
    value as u16
}

/// Parses everything the terminal sent back. Returns None until the DA1 reply (`ESC [ ? … c`) has
/// arrived, i.e. while more replies may follow.
pub fn parse(reply: &[u8]) -> Option<Capabilities> {
    let mut caps = Capabilities::default();
    let (mut kitty, mut da1) = (false, false);
    let mut at = 0;
    while let Some(esc) = reply
        .get(at..)
        .and_then(|rest| rest.iter().position(|&c| c == 0x1b))
        .map(|i| i + at)
    {
        at = esc + 1;
        if esc + 2 > reply.len() {
            break;
        }
        match reply[esc + 1] {
            b'_' => {
                // APC: kitty graphics response "Gi=31;OK"
                let Some(end) = find(reply, esc, b"\x1b\\") else {
                    break;
                };
                let body = &reply[esc + 2..end];
                if body.starts_with(b"G")
                    && body.windows(4).any(|w| w == b"i=31")
                    && body.ends_with(b";OK")
                {
                    kitty = true;
                }
                at = end + 2;
            }
            b'[' => {
                let mut i = esc + 2;
                while i < reply.len() && !(0x40..=0x7e).contains(&reply[i]) {
                    i += 1;
                }
                if i >= reply.len() {
                    break;
                }
                let params = &reply[esc + 2..i];
                let last = reply[i];
                at = i + 1;
                if last == b'c' && params.first() == Some(&b'?') {
                    da1 = true;
                    // First field is the device class; attribute 4 means sixel.
                    if params[1..]
                        .split(|&c| c == b';')
                        .skip(1)
                        .any(|field| field == b"4")
                        && caps.protocol == Protocol::None
                    {
                        caps.protocol = Protocol::Sixel;
                    }
                } else if last == b't' {
                    let mut fields = params.split(|&c| c == b';');
                    let kind = fields.next().unwrap_or(b"");
                    let h = parse_u16(fields.next().unwrap_or(b""));
                    let w = parse_u16(fields.next().unwrap_or(b""));
                    if kind == b"6" {
                        caps.cell_height = h;
                        caps.cell_width = w;
                    } else if kind == b"4" {
                        caps.area_height = h;
                        caps.area_width = w;
                    }
                }
            }
            _ => {}
        }
    }
    if !da1 {
        return None;
    }
    caps.answered = true;
    if kitty {
        caps.protocol = Protocol::Kitty; // preferred: sends pixel data directly
    }
    Some(caps)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn foot_style_reply_sixel_via_da1_attribute_4_and_cell_size() {
        let caps = parse(b"\x1b[6;20;10t\x1b[?62;4;22c").unwrap();
        assert_eq!(
            (caps.protocol, caps.cell_width, caps.cell_height),
            (Protocol::Sixel, 10, 20)
        );
    }

    #[test]
    fn cell_size_falls_back_to_text_area_divided_by_the_grid() {
        let caps = parse(b"\x1b[4;900;1500t\x1b[?62;4c")
            .unwrap()
            .with_cells_from(150, 45);
        assert_eq!((caps.cell_width, caps.cell_height), (10, 20));
    }

    #[test]
    fn window_pixel_size_overrides_the_scaled_csi_16t_cell_size() {
        // Real foot at scale 1.6: CSI 16t says 13x35, the ioctl says 1830x1064 for 183x38.
        let caps = parse(b"\x1b[6;35;13t\x1b[?62;4;22;28;52c")
            .unwrap()
            .with_window_pixels(183, 38, 1830, 1064);
        assert_eq!((caps.cell_width, caps.cell_height), (10, 28));
        let unknown = parse(b"\x1b[6;35;13t\x1b[?62;4c")
            .unwrap()
            .with_window_pixels(183, 38, 0, 0);
        assert_eq!(unknown.cell_width, 13);
    }

    #[test]
    fn kitty_reply_wins_over_sixel_and_no_da1_means_keep_waiting() {
        assert_eq!(parse(b"\x1b_Gi=31;OK\x1b\\"), None);
        assert_eq!(
            parse(b"\x1b_Gi=31;OK\x1b\\\x1b[?62;4c").unwrap().protocol,
            Protocol::Kitty
        );
        assert_eq!(
            parse(b"\x1b_Gi=31;ENOENT:x\x1b\\\x1b[?1;2c")
                .unwrap()
                .protocol,
            Protocol::None
        );
        assert_eq!(parse(b"\x1b[?62;22c").unwrap().protocol, Protocol::None);
    }

    #[test]
    fn truncated_and_hostile_replies_never_panic() {
        for reply in [
            &b"\x1b"[..],
            b"\x1b[",
            b"\x1b_G",
            b"\x1b[?",
            b"\x1b[6;99999999;t",
            b"\x1b[6;;t\x1b[c",
            b"\x1b[?4;c\x1b[?c",
        ] {
            let _ = parse(reply);
        }
    }
}
