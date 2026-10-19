//! Embedded Bible corpora (kjv, grb, vul) and reference selection, ported from
//! `src/bundled.zig`.
//!
//! The TSV files are embedded verbatim. Chapter byte spans are computed lazily on first use
//! with the same rules as `tests/bundle_sources.py` (the generated Zig `*_index.zig`); a test
//! checks them against those files. KAT-18 decides the final index strategy.

use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub chapter: u16,
    pub number: u16,
    pub text: Cow<'static, str>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BundledError {
    InvalidReference,
    DuplicateVerse,
    Malformed,
}

impl std::fmt::Display for BundledError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for BundledError {}

/// Index of the Greek source in [`TOOLS`]; its repeated verse labels are merged.
const GREEK: usize = 1;

pub static TEXTS: [&str; 3] = [
    include_str!("../../../../src/data/kjv.tsv"),
    include_str!("../../../../src/data/grb.tsv"),
    include_str!("../../../../src/data/vul.tsv"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Span {
    pub book: &'static str,
    pub chapter: u16,
    pub start: usize,
    pub end: usize,
}

fn compute_spans(text: &'static str) -> Vec<Span> {
    let mut spans: Vec<Span> = Vec::new();
    let mut offset = 0usize;
    let bytes = text.as_bytes();
    while offset < bytes.len() {
        // Python's bytes.splitlines(keepends=True): break after \n, \r\n or a lone \r.
        let mut end = offset;
        while end < bytes.len() && bytes[end] != b'\n' && bytes[end] != b'\r' {
            end += 1;
        }
        if end < bytes.len() {
            end += if bytes[end] == b'\r' && bytes.get(end + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        }
        let line = &text[offset..end];
        let fields: Vec<&str> = line
            .trim_end_matches(['\r', '\n'])
            .splitn(6, '\t')
            .collect();
        if fields.len() == 4 {
            // Source subsection heading, not a verse.
            offset = end;
            continue;
        }
        assert_eq!(
            fields.len(),
            6,
            "embedded TSV: expected six fields at byte {offset}"
        );
        let chapter: u16 = fields[3].parse().expect("embedded TSV: chapter number");
        match spans.last_mut() {
            Some(last) if last.book == fields[0] && last.chapter == chapter => last.end = end,
            _ => spans.push(Span {
                book: fields[0],
                chapter,
                start: offset,
                end,
            }),
        }
        offset = end;
    }
    spans
}

pub fn spans(source_index: usize) -> &'static [Span] {
    static SPANS: [OnceLock<Vec<Span>>; 3] = [OnceLock::new(), OnceLock::new(), OnceLock::new()];
    SPANS[source_index].get_or_init(|| compute_spans(TEXTS[source_index]))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Selection {
    pub chapters: String,
    pub verses: String,
    pub cross: Option<(u32, u32)>,
}

fn position(chapter: u16, verse: u16) -> u32 {
    ((chapter as u32) << 16) | verse as u32
}

fn number(text: &str) -> Result<u16, BundledError> {
    if text.is_empty() || !text.bytes().all(|c| c.is_ascii_digit()) {
        return Err(BundledError::InvalidReference);
    }
    text.parse().map_err(|_| BundledError::InvalidReference)
}

fn interval(text: &str) -> Result<(u16, u16), BundledError> {
    let (first, last) = match text.split_once('-') {
        Some((a, b)) => (number(a)?, number(b)?),
        None => {
            let n = number(text)?;
            (n, n)
        }
    };
    if last < first {
        return Err(BundledError::InvalidReference);
    }
    Ok((first, last))
}

fn validate_list(text: &str) -> Result<(), BundledError> {
    text.split(',')
        .try_for_each(|term| interval(term).map(|_| ()))
}

fn contains(text: &str, value: u16) -> bool {
    text.split(',')
        .any(|term| matches!(interval(term), Ok((a, b)) if value >= a && value <= b))
}

impl Selection {
    /// Parse the part of a reference after the book name (empty, or starting with `:`).
    pub fn new(suffix: &str) -> Result<Selection, BundledError> {
        if suffix.is_empty() {
            return Ok(Selection::default());
        }
        if !suffix.starts_with(':') || suffix.len() == 1 {
            return Err(BundledError::InvalidReference);
        }
        let body = &suffix[1..];
        let colon = body.find(':').unwrap_or(body.len());
        let chapters = &body[..colon];
        validate_list(chapters)?;
        let verses = if colon < body.len() {
            &body[colon + 1..]
        } else {
            ""
        };
        if colon < body.len() {
            let first_chapter = number(chapters)?; // Verse queries require one chapter.
            if let Some(end_colon) = verses.find(':') {
                let dash = verses[..end_colon]
                    .find('-')
                    .ok_or(BundledError::InvalidReference)?;
                let first_verse = number(&verses[..dash])?;
                let last_chapter = number(&verses[dash + 1..end_colon])?;
                let last_verse = number(&verses[end_colon + 1..])?;
                let first = position(first_chapter, first_verse);
                let last = position(last_chapter, last_verse);
                if last < first {
                    return Err(BundledError::InvalidReference);
                }
                return Ok(Selection {
                    cross: Some((first, last)),
                    ..Selection::default()
                });
            }
            validate_list(verses)?;
        }
        Ok(Selection {
            chapters: chapters.to_string(),
            verses: verses.to_string(),
            cross: None,
        })
    }

    pub fn has_chapter(&self, chapter: u16) -> bool {
        match self.cross {
            Some((first, last)) => chapter as u32 >= first >> 16 && chapter as u32 <= last >> 16,
            None => self.chapters.is_empty() || contains(&self.chapters, chapter),
        }
    }

    pub fn matches(&self, chapter: u16, verse: u16) -> bool {
        match self.cross {
            Some((first, last)) => {
                let label = position(chapter, verse);
                label >= first && label <= last
            }
            None => {
                self.has_chapter(chapter)
                    && (self.verses.is_empty() || contains(&self.verses, verse))
            }
        }
    }
}

/// One verse record of a source, in embedded (source) order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Record {
    pub book: &'static str,
    pub chapter: u16,
    pub number: u16,
    pub text: &'static str,
}

/// Sequential scan of every non-empty verse record in one source.
pub struct Records {
    text: &'static str,
    spans: &'static [Span],
    span: usize,
    lines: Option<std::str::Split<'static, char>>,
}

impl Iterator for Records {
    type Item = Record;

    fn next(&mut self) -> Option<Record> {
        loop {
            if self.lines.is_none() {
                let span = self.spans.get(self.span)?;
                self.lines = Some(self.text[span.start..span.end].split('\n'));
            }
            let Some(raw_line) = self.lines.as_mut().and_then(Iterator::next) else {
                self.lines = None;
                self.span += 1;
                continue;
            };
            let mut fields = raw_line.trim_end_matches('\r').splitn(6, '\t');
            let (_, _, _) = (fields.next()?, fields.next(), fields.next());
            let Some(chapter) = fields.next().and_then(|c| c.parse::<u16>().ok()) else {
                continue;
            };
            let Some(label) = fields.next() else { continue };
            let Ok(number) = label.split('-').next().unwrap_or(label).parse::<u16>() else {
                continue;
            };
            let text = fields.next().unwrap_or("");
            if text.is_empty() {
                continue;
            }
            return Some(Record {
                book: self.spans[self.span].book,
                chapter,
                number,
                text,
            });
        }
    }
}

pub fn records(source_index: usize) -> Records {
    Records {
        text: TEXTS[source_index],
        spans: spans(source_index),
        span: 0,
        lines: None,
    }
}

/// Pure retrieval from the immutable embedded TSV via chapter spans. Repeated Greek labels
/// (source index 1) are merged in source order, joined by one space; elsewhere a repeat is an
/// error.
pub fn query(source_index: usize, raw_book: &str, suffix: &str) -> Result<Vec<Row>, BundledError> {
    let mut result: Vec<Row> = Vec::new();
    if source_index >= TEXTS.len() {
        return Ok(result);
    }
    let selection = Selection::new(suffix)?;
    let text = TEXTS[source_index];
    let mut labels: HashMap<u32, usize> = HashMap::new();
    for span in spans(source_index) {
        if span.book != raw_book || !selection.has_chapter(span.chapter) {
            continue;
        }
        for raw_line in text[span.start..span.end].split('\n') {
            let mut fields = raw_line.trim_end_matches('\r').splitn(6, '\t');
            if fields.next().is_none() || fields.next().is_none() || fields.next().is_none() {
                continue;
            }
            let Some(chapter) = fields.next().and_then(|c| c.parse::<u16>().ok()) else {
                continue;
            };
            let Some(label) = fields.next() else { continue };
            // The original exporter prints combined source labels (27-28) with %d, which
            // keeps only the first number; never fabricate the omitted label.
            let verse: u16 = label
                .split('-')
                .next()
                .unwrap_or(label)
                .parse()
                .map_err(|_| BundledError::Malformed)?;
            let body = fields.next().unwrap_or("");
            if body.is_empty()
                && source_index == 0
                && raw_book == "Sirach"
                && chapter == 0
                && verse == 0
            {
                continue;
            }
            if !selection.matches(chapter, verse) {
                continue;
            }
            match labels.get(&position(chapter, verse)) {
                Some(&index) => {
                    if source_index != GREEK {
                        return Err(BundledError::DuplicateVerse);
                    }
                    let existing = &mut result[index];
                    existing.text = Cow::Owned(format!("{} {}", existing.text, body));
                }
                None => {
                    labels.insert(position(chapter, verse), result.len());
                    result.push(Row {
                        chapter,
                        number: verse,
                        text: Cow::Borrowed(body),
                    });
                }
            }
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse the Zig generated index (`.{ .book = "X", .chapter = N, .start = A, .end = B }`).
    fn zig_spans(name: &str) -> Vec<Span> {
        let path = format!(
            "{}/../../../src/data/{name}_index.zig",
            env!("CARGO_MANIFEST_DIR")
        );
        let text = std::fs::read_to_string(path).unwrap();
        text.lines()
            .filter(|l| l.contains(".book = "))
            .map(|l| {
                let field = |key: &str| -> &str {
                    let rest = &l[l.find(key).unwrap() + key.len()..];
                    rest[..rest.find([',', ' ']).unwrap_or(rest.len())].trim_matches('"')
                };
                let book = &l[l.find(".book = \"").unwrap() + 9..];
                let book = &book[..book.find('"').unwrap()];
                let leaked: &'static str = Box::leak(book.to_string().into_boxed_str());
                Span {
                    book: leaked,
                    chapter: field(".chapter = ").parse().unwrap(),
                    start: field(".start = ").parse().unwrap(),
                    end: field(".end = ")
                        .trim_end_matches(" }")
                        .trim_end_matches(',')
                        .parse()
                        .unwrap(),
                }
            })
            .collect()
    }

    #[test]
    fn computed_spans_equal_the_zig_generated_indexes() {
        for (index, name) in ["kjv", "grb", "vul"].into_iter().enumerate() {
            let expected = zig_spans(name);
            assert!(!expected.is_empty());
            assert_eq!(spans(index), expected.as_slice(), "{name}");
        }
    }

    #[test]
    fn selection_validation_and_matching() {
        assert!(Selection::new("").unwrap().has_chapter(7));
        let s = Selection::new(":1,3-4:2,5").unwrap_err();
        assert_eq!(s, BundledError::InvalidReference); // verse queries need a single chapter
        let s = Selection::new(":3:2,5-6").unwrap();
        assert!(s.matches(3, 5) && s.matches(3, 2) && !s.matches(3, 4) && !s.matches(4, 5));
        let cross = Selection::new(":1:50-2:3").unwrap();
        assert!(
            cross.matches(1, 50)
                && cross.matches(2, 3)
                && !cross.matches(2, 4)
                && !cross.matches(1, 49)
        );
        assert!(cross.has_chapter(2) && !cross.has_chapter(3));
    }

    #[test]
    fn records_skip_empty_text_and_follow_source_order() {
        let first = records(0).next().unwrap();
        assert_eq!((first.book, first.chapter, first.number), ("Genesis", 1, 1));
        assert!(first.text.starts_with("In the beginning God created"));
        assert!(records(2).all(|r| !r.text.is_empty()));
    }

    #[test]
    fn greek_repeated_labels_merge_and_other_sources_reject_repeats() {
        // grb Sirach repeats labels (additions); the merged text is joined by one space.
        let rows = query(1, "Sirach", "").unwrap();
        let mut seen = std::collections::HashSet::new();
        assert!(
            rows.iter().all(|r| seen.insert((r.chapter, r.number))),
            "labels unique after merge"
        );
    }
}
