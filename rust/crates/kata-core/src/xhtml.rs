//! Tolerant XHTML/XML tokenizer for EPUB content documents, ported from `src/xhtml.zig`. No
//! DTDs, no validation: comments, processing instructions and doctypes are skipped; CDATA is
//! returned as literal text. Works on bytes so malformed UTF-8 never panics.

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tag<'a> {
    /// Element name as written (may carry a namespace prefix).
    pub name: &'a [u8],
    /// Whole start tag without the angle brackets, for attribute lookup.
    pub raw: &'a [u8],
    pub self_closing: bool,
}

impl<'a> Tag<'a> {
    /// Element name without any namespace prefix, e.g. "svg" for "svg:svg".
    pub fn local(&self) -> &'a [u8] {
        local_name(self.name)
    }

    pub fn attr(&self, name: &str) -> Option<&'a [u8]> {
        attribute(self.raw, name.as_bytes())
    }

    /// True when the whitespace-separated attribute value contains `token`.
    pub fn has_token(&self, name: &str, token: &str) -> bool {
        self.attr(name).is_some_and(|value| {
            value
                .split(|c| matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
                .any(|word| word == token.as_bytes())
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token<'a> {
    /// Raw character data; entities still encoded.
    Text(&'a [u8]),
    /// CDATA section contents; literal.
    Cdata(&'a [u8]),
    Start(Tag<'a>),
    /// End tag name as written.
    End(&'a [u8]),
}

pub fn is_space(c: u8) -> bool {
    matches!(c, b' ' | b'\t' | b'\n' | b'\r' | 0x0b | 0x0c)
}

pub fn local_name(name: &[u8]) -> &[u8] {
    match name.iter().rposition(|&c| c == b':') {
        Some(colon) => &name[colon + 1..],
        None => name,
    }
}

fn find(haystack: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    if from > haystack.len() {
        return None;
    }
    haystack[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|i| i + from)
}

fn find_byte(haystack: &[u8], from: usize, byte: u8) -> Option<usize> {
    haystack
        .get(from..)?
        .iter()
        .position(|&c| c == byte)
        .map(|i| i + from)
}

/// Value of attribute `name` in a start tag's raw text (exact name, so "type" does not match
/// "epub:type" or "data-type").
pub fn attribute<'a>(raw: &'a [u8], name: &[u8]) -> Option<&'a [u8]> {
    let mut from = 0;
    while let Some(at) = find(raw, from, name) {
        from = at + name.len();
        if at == 0 || !is_space(raw[at - 1]) {
            continue;
        }
        let mut i = at + name.len();
        while i < raw.len() && is_space(raw[i]) {
            i += 1;
        }
        if i >= raw.len() || raw[i] != b'=' {
            continue;
        }
        i += 1;
        while i < raw.len() && is_space(raw[i]) {
            i += 1;
        }
        if i >= raw.len() || (raw[i] != b'"' && raw[i] != b'\'') {
            continue;
        }
        let close = find_byte(raw, i + 1, raw[i])?;
        return Some(&raw[i + 1..close]);
    }
    None
}

pub struct Tokenizer<'a> {
    src: &'a [u8],
    at: usize,
}

impl<'a> Tokenizer<'a> {
    pub fn new(src: &'a [u8]) -> Self {
        Tokenizer { src, at: 0 }
    }
}

fn trim(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
        .map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

impl<'a> Iterator for Tokenizer<'a> {
    type Item = Token<'a>;

    fn next(&mut self) -> Option<Token<'a>> {
        let s = self.src;
        while self.at < s.len() {
            if s[self.at] != b'<' {
                let end = find_byte(s, self.at, b'<').unwrap_or(s.len());
                let text = &s[self.at..end];
                self.at = end;
                return Some(Token::Text(text));
            }
            let rest = &s[self.at..];
            if rest.starts_with(b"<!--") {
                let end = find(s, self.at + 4, b"-->").unwrap_or(s.len());
                self.at = s.len().min(end + 3);
                continue;
            }
            if rest.starts_with(b"<![CDATA[") {
                let start = self.at + 9;
                let end = find(s, start, b"]]>").unwrap_or(s.len());
                self.at = s.len().min(end + 3);
                return Some(Token::Cdata(&s[start.min(end)..end]));
            }
            if rest.starts_with(b"<!") || rest.starts_with(b"<?") {
                let end = find_byte(s, self.at, b'>').unwrap_or(s.len());
                self.at = s.len().min(end + 1);
                continue;
            }
            // Find the closing '>' outside quoted attribute values.
            let mut i = self.at + 1;
            let mut quote = 0u8;
            while i < s.len() {
                let c = s[i];
                if quote != 0 {
                    if c == quote {
                        quote = 0;
                    }
                } else if c == b'"' || c == b'\'' {
                    quote = c;
                } else if c == b'>' {
                    break;
                }
                i += 1;
            }
            let inner = &s[self.at + 1..s.len().min(i)];
            self.at = s.len().min(i + 1);
            if inner.is_empty() {
                return Some(Token::Text(b"<"));
            }
            if inner[0] == b'/' {
                return Some(Token::End(trim(&inner[1..])));
            }
            let name_end = inner
                .iter()
                .position(|&c| is_space(c) || c == b'/')
                .unwrap_or(inner.len());
            if name_end == 0 {
                return Some(Token::Text(b"<"));
            }
            let trimmed = &inner[..inner
                .iter()
                .rposition(|&c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
                .map_or(0, |e| e + 1)];
            return Some(Token::Start(Tag {
                name: &inner[..name_end],
                raw: inner,
                self_closing: trimmed.ends_with(b"/"),
            }));
        }
        None
    }
}

const NAMED: [(&str, u32); 15] = [
    ("amp", '&' as u32),
    ("lt", '<' as u32),
    ("gt", '>' as u32),
    ("quot", '"' as u32),
    ("apos", '\'' as u32),
    ("nbsp", 0xA0),
    ("mdash", 0x2014),
    ("ndash", 0x2013),
    ("hellip", 0x2026),
    ("lsquo", 0x2018),
    ("rsquo", 0x2019),
    ("ldquo", 0x201C),
    ("rdquo", 0x201D),
    ("copy", 0xA9),
    ("shy", 0xAD),
];

/// Decoded code point and byte length of an entity at `text[at]` ('&'), or None.
pub fn entity_at(text: &[u8], at: usize) -> Option<(u32, usize)> {
    let semi = find_byte(text, at, b';')?;
    if semi - at > 12 || semi == at + 1 {
        return None;
    }
    let body = &text[at + 1..semi];
    let len = semi + 1 - at;
    if body[0] == b'#' {
        let digits = std::str::from_utf8(&body[1..]).ok()?;
        let value = match digits.strip_prefix(['x', 'X']) {
            Some(hex) => parse_unsigned(hex, 16)?,
            None => parse_unsigned(digits, 10)?,
        };
        // Zig parses into u21 and rejects invalid scalar values and NUL.
        if value == 0 || value > 0x10_FFFF || (0xD800..=0xDFFF).contains(&value) {
            return None;
        }
        return Some((value, len));
    }
    NAMED
        .iter()
        .find(|(name, _)| name.as_bytes() == body)
        .map(|&(_, cp)| (cp, len))
}

/// Like Zig's `parseInt` for an unsigned type: optional `+`, digits and `_` separators rejected.
fn parse_unsigned(digits: &str, radix: u32) -> Option<u32> {
    let digits = digits.strip_prefix('+').unwrap_or(digits);
    if digits.is_empty() || digits.starts_with('_') || digits.ends_with('_') {
        return None;
    }
    let mut value: u32 = 0;
    for c in digits.chars() {
        if c == '_' {
            continue;
        }
        value = value.checked_mul(radix)?.checked_add(c.to_digit(radix)?)?;
        if value > 0x1F_FFFF {
            return None; // does not fit u21
        }
    }
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(t: Token<'_>) -> &[u8] {
        match t {
            Token::Text(s) | Token::Cdata(s) | Token::End(s) => s,
            Token::Start(tag) => tag.name,
        }
    }

    #[test]
    fn tokenizer_handles_comments_cdata_quotes_prefixes_and_self_closing_tags() {
        let mut t = Tokenizer::new(
            br#"<?xml version="1.0"?><!DOCTYPE html><!-- c --><p class="a>b">x &amp; y<br/><![CDATA[<raw>]]></p><svg:svg/>"#,
        );
        assert_eq!(text(t.next().unwrap()), b"p");
        assert_eq!(text(t.next().unwrap()), b"x &amp; y");
        assert!(matches!(t.next().unwrap(), Token::Start(tag) if tag.self_closing));
        assert_eq!(t.next().unwrap(), Token::Cdata(b"<raw>"));
        assert_eq!(t.next().unwrap(), Token::End(b"p"));
        match t.next().unwrap() {
            Token::Start(tag) => assert_eq!(tag.local(), b"svg"),
            other => panic!("{other:?}"),
        }
        assert!(t.next().is_none());
    }

    #[test]
    fn attributes_match_exact_names_and_class_tokens() {
        let tag = Tag {
            name: b"p",
            raw: br#"p class="verse opening" epub:type='x' data-type="y""#,
            self_closing: false,
        };
        assert!(tag.has_token("class", "verse"));
        assert!(!tag.has_token("class", "vers"));
        assert_eq!(tag.attr("epub:type"), Some(&b"x"[..]));
        assert_eq!(tag.attr("data-type"), Some(&b"y"[..]));
        assert_eq!(tag.attr("type"), None);
    }

    #[test]
    fn entities_decode_numeric_and_common_named_forms() {
        assert_eq!(entity_at(b"&#x2019;", 0).unwrap().0, 0x2019);
        assert_eq!(entity_at(b"&#233;", 0).unwrap().0, 233);
        assert_eq!(entity_at(b"&amp;", 0).unwrap().0, '&' as u32);
        assert_eq!(entity_at(b"& x;", 0), None);
        assert_eq!(entity_at(b"&bogus;", 0), None);
        assert_eq!(entity_at(b"&#0;", 0), None);
        assert_eq!(entity_at(b"&#xD800;", 0), None);
        assert_eq!(entity_at(b"&#99999999999;", 0), None);
    }

    #[test]
    fn truncated_markup_never_panics() {
        for s in [
            &b"<"[..],
            b"<p",
            b"<p class=\"x",
            b"<!--",
            b"<![CDATA[x",
            b"</",
            b"<>",
            b"< p>",
            b"<?x",
        ] {
            let _ = Tokenizer::new(s).count();
        }
    }
}
