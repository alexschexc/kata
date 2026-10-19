//! Whole-source word search for one translation, with case- and accent-insensitive word-prefix
//! matching ("love" finds loved, loveth; "λογος" finds Λόγος, λόγου). Ported from
//! `src/search.zig`. Results carry the Bible location the reader jumps to; KJV Psalms report
//! Greek/Latin psalm positions with KJV labels.

use crate::bundled;
use crate::fold::{fold, is_word_char};
use crate::library::{self, Location};
use crate::source::{self, Label, Verse};
use std::collections::HashMap;

pub const MAX_QUERY: usize = 64;

/// Decode one char at byte offset `at`; invalid UTF-8 yields U+FFFD with length 1.
fn decode_at(text: &[u8], at: usize) -> (u32, usize) {
    let lead = text[at];
    let len = match lead {
        0x00..=0x7f => 1,
        0xc2..=0xdf => 2,
        0xe0..=0xef => 3,
        0xf0..=0xf4 => 4,
        _ => return (0xfffd, 1),
    };
    if at + len > text.len() {
        return (0xfffd, 1);
    }
    match std::str::from_utf8(&text[at..at + len]) {
        Ok(s) => (s.chars().next().map_or(0xfffd, |c| c as u32), len),
        Err(_) => (0xfffd, 1),
    }
}

/// A folded query. Runs of spaces collapse to one; leading/trailing spaces are dropped.
/// Invalid UTF-8 is ignored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Query {
    cps: [u32; MAX_QUERY],
    len: usize,
}

impl Default for Query {
    fn default() -> Self {
        Query {
            cps: [0; MAX_QUERY],
            len: 0,
        }
    }
}

impl Query {
    pub fn new(text: &str) -> Query {
        Query::from_bytes(text.as_bytes())
    }

    pub fn from_bytes(text: &[u8]) -> Query {
        let mut query = Query::default();
        let mut at = 0;
        let mut pending_space = false;
        while at < text.len() && query.len < MAX_QUERY {
            let (cp, len) = decode_at(text, at);
            at += len;
            if cp == 0xfffd {
                continue;
            }
            if cp == ' ' as u32 || cp == '\t' as u32 {
                pending_space = query.len > 0;
                continue;
            }
            if pending_space && query.len + 1 < MAX_QUERY {
                query.cps[query.len] = ' ' as u32;
                query.len += 1;
            }
            pending_space = false;
            query.cps[query.len] = fold(cp);
            query.len += 1;
        }
        query
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn chars(&self) -> &[u32] {
        &self.cps[..self.len]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Range {
    pub start: usize,
    pub end: usize,
}

fn match_at(text: &[u8], start: usize, query: &Query) -> Option<usize> {
    let mut at = start;
    for &want in query.chars() {
        if at >= text.len() {
            return None;
        }
        let (cp, len) = decode_at(text, at);
        if fold(cp) != want {
            return None;
        }
        at += len;
    }
    Some(at)
}

/// Next match at or after byte `from`. A query beginning with a letter only matches at the
/// start of a word; the range extends to the end of that word.
pub fn next(text: &str, query: &Query, from: usize) -> Option<Range> {
    let text = text.as_bytes();
    if query.is_empty() {
        return None;
    }
    let word_start = is_word_char(query.cps[0]);
    let mut previous_word = false;
    if from > 0 && from <= text.len() {
        let mut back = from - 1;
        while back > 0 && (text[back] & 0xc0) == 0x80 {
            back -= 1;
        }
        previous_word = is_word_char(fold(decode_at(text, back).0));
    }
    let mut at = from;
    while at < text.len() {
        let (cp, len) = decode_at(text, at);
        if (!word_start || !previous_word)
            && let Some(end) = match_at(text, at, query)
        {
            let mut stop = end;
            if word_start && is_word_char(query.cps[query.len - 1]) {
                while stop < text.len() {
                    let (following, following_len) = decode_at(text, stop);
                    if !is_word_char(fold(following)) {
                        break;
                    }
                    stop += following_len;
                }
            }
            return Some(Range {
                start: at,
                end: stop,
            });
        }
        previous_word = is_word_char(fold(cp));
        at += len;
    }
    None
}

pub fn count(text: &str, query: &Query) -> usize {
    let mut total = 0;
    let mut at = 0;
    while let Some(range) = next(text, query, at) {
        total += 1;
        at = range.end;
    }
    total
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hit {
    /// Index into `source::CATALOG` (and the Bible title's sections).
    pub section: usize,
    /// Aligned reader position (Greek/Latin psalm numbering for KJV Psalms).
    pub chapter: u16,
    pub number: u16,
    /// The source's own chapter:verse label.
    pub label: Label,
    pub text: String,
    pub match_range: Range,
    pub occurrences: u32,
}

impl Hit {
    pub fn book(&self) -> &'static str {
        source::CATALOG[self.section].name
    }

    pub fn location(&self) -> Location {
        Location {
            title: bible_title(),
            section: self.section,
            chapter: self.chapter,
            verse: Some(self.number),
        }
    }
}

fn bible_title() -> usize {
    library::titles()
        .iter()
        .position(|t| t.id == "bible")
        .expect("bible title registered")
}

fn section_for(pane: usize, raw_book: &str) -> Option<usize> {
    source::CATALOG
        .iter()
        .position(|entry| entry.query_names[pane] == Some(raw_book))
}

#[derive(Debug, Default)]
pub struct Results {
    pub hits: Vec<Hit>,
    pub occurrences: usize,
}

/// Every verse of source `pane` containing the query, in source order. Repeated Greek labels
/// are reported once, as the reader shows them.
pub fn find(pane: usize, query: &Query) -> Results {
    let mut results = Results::default();
    if query.is_empty() || pane >= source::TOOLS.len() {
        return results;
    }
    let mut seen: HashMap<u64, usize> = HashMap::new();
    let mut cached_book = "";
    let mut cached_section: Option<usize> = None;
    for record in bundled::records(pane) {
        let Some(first) = next(record.text, query, 0) else {
            continue;
        };
        let matches = count(record.text, query) as u32;
        if record.book != cached_book {
            cached_book = record.book;
            cached_section = section_for(pane, record.book);
        }
        let Some(section) = cached_section else {
            continue;
        };
        let (mut chapter, mut number) = (record.chapter, record.number);
        if pane == 0 && source::CATALOG[section].name == "Psalms" {
            let Some(aligned) = source::kjv_psalm_to_lxx(chapter, number) else {
                continue;
            };
            chapter = aligned.chapter;
            number = aligned.number;
        }
        results.occurrences += matches as usize;
        let key = ((section as u64) << 32) | ((chapter as u64) << 16) | number as u64;
        if let Some(&index) = seen.get(&key) {
            results.hits[index].occurrences += matches;
            continue;
        }
        seen.insert(key, results.hits.len());
        results.hits.push(Hit {
            section,
            chapter,
            number,
            label: Label {
                chapter: record.chapter,
                number: record.number,
            },
            text: record.text.to_string(),
            match_range: first,
            occurrences: matches,
        });
    }
    results
}

/// Search over an edition's verses (all in catalog books).
pub fn find_in(verses: &[Verse], query: &Query) -> Results {
    let mut results = Results::default();
    for verse in verses {
        let Some(first) = next(&verse.text, query, 0) else {
            continue;
        };
        let Some(section) = source::CATALOG
            .iter()
            .position(|entry| entry.name == verse.book)
        else {
            continue;
        };
        let matches = count(&verse.text, query) as u32;
        results.occurrences += matches as usize;
        results.hits.push(Hit {
            section,
            chapter: verse.chapter,
            number: verse.number,
            label: verse.label.unwrap_or(Label {
                chapter: verse.chapter,
                number: verse.number,
            }),
            text: verse.text.clone(),
            match_range: first,
            occurrences: matches,
        });
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_start_at_word_boundaries_and_extend_to_the_word_end() {
        let query = Query::new("love");
        let text = "I love; he loveth. Glove beloved LOVE";
        let mut found = Vec::new();
        let mut at = 0;
        while let Some(range) = next(text, &query, at) {
            found.push(&text[range.start..range.end]);
            at = range.end;
        }
        assert_eq!(found, ["love", "loveth", "LOVE"]);
    }

    #[test]
    fn greek_and_latin_match_without_accents_and_phrases_collapse_spaces() {
        let greek = Query::new("λογος");
        let verse = "Ἐν ἀρχῇ ἦν ὁ Λόγος, καὶ ὁ λόγος ἦν πρὸς τὸν θεόν";
        assert_eq!(count(verse, &greek), 2);
        let phrase = Query::new("  my   SHEPHERD ");
        assert_eq!(
            count("The LORD is my shepherd; I shall not want.", &phrase),
            1
        );
        let empty = Query::new("   ");
        assert!(empty.is_empty());
        assert_eq!(next("anything", &empty, 0), None);
    }

    #[test]
    fn invalid_utf8_in_a_query_is_ignored_and_long_queries_are_capped() {
        assert_eq!(Query::from_bytes(b"lo\xffve"), Query::new("love"));
        assert_eq!(Query::new(&"a".repeat(200)).len, MAX_QUERY);
    }

    #[test]
    fn every_source_book_maps_to_a_bible_section() {
        for pane in 0..source::TOOLS.len() {
            let mut last = "";
            for record in bundled::records(pane) {
                if record.book == last {
                    continue;
                }
                last = record.book;
                assert!(
                    section_for(pane, record.book).is_some(),
                    "{} in pane {pane}",
                    record.book
                );
            }
        }
    }

    #[test]
    fn whole_source_search_finds_verses_with_jumpable_locations() {
        let kjv = find(0, &Query::new("shepherd"));
        assert!(kjv.hits.len() > 50);
        let mut psalm = false;
        for hit in &kjv.hits {
            assert!(
                library::valid(hit.location()),
                "{} {}:{}",
                hit.book(),
                hit.chapter,
                hit.number
            );
            if hit.book() == "Psalms" && hit.label.chapter == 23 && hit.label.number == 1 {
                assert_eq!(hit.chapter, 22);
                psalm = true;
            }
        }
        assert!(psalm);
        let grb = find(1, &Query::new("λογος"));
        let john = grb
            .hits
            .iter()
            .find(|h| h.book() == "John" && h.chapter == 1 && h.number == 1)
            .unwrap();
        assert_eq!(john.occurrences, 3);
        let vul = find(2, &Query::new("verbum"));
        assert!(!vul.hits.is_empty() && vul.occurrences >= vul.hits.len());
        assert!(find(0, &Query::new("zzzzqqq")).hits.is_empty());
    }
}
