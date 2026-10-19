//! Book names, reference parsing and verse retrieval from the bundled sources, ported from
//! `src/source.zig`. There is no external tool: `fetch` reads the embedded corpora.

use crate::book_catalog::{CATALOG as BOOK_CATALOG, Entry};
use crate::bundled::{self, BundledError, Selection};

pub use crate::book_catalog::Entry as CatalogEntry;

pub const TOOLS: [&str; 3] = ["kjv", "grb", "vul"];
pub static CATALOG: &[Entry] = &BOOK_CATALOG;

/// A source's own chapter:verse label when it differs from the aligned row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Label {
    pub chapter: u16,
    pub number: u16,
}

/// `chapter`/`number` are the alignment position; `label` (when set) is the numbering printed
/// by the source itself and is what the reader displays.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verse {
    pub book: String,
    pub chapter: u16,
    pub number: u16,
    pub text: String,
    pub label: Option<Label>,
}

#[derive(Debug, Clone, Copy)]
pub struct Book {
    pub name: &'static str,
    pub alias: &'static str,
}

pub static BOOKS: [Book; 27] = [
    Book {
        name: "Matthew",
        alias: "Mat",
    },
    Book {
        name: "Mark",
        alias: "Mark",
    },
    Book {
        name: "Luke",
        alias: "Luke",
    },
    Book {
        name: "John",
        alias: "John",
    },
    Book {
        name: "Acts",
        alias: "Acts",
    },
    Book {
        name: "Romans",
        alias: "Rom",
    },
    Book {
        name: "1 Corinthians",
        alias: "1Cor",
    },
    Book {
        name: "2 Corinthians",
        alias: "2Cor",
    },
    Book {
        name: "Galatians",
        alias: "Gal",
    },
    Book {
        name: "Ephesians",
        alias: "Eph",
    },
    Book {
        name: "Philippians",
        alias: "Phi",
    },
    Book {
        name: "Colossians",
        alias: "Col",
    },
    Book {
        name: "1 Thessalonians",
        alias: "1Th",
    },
    Book {
        name: "2 Thessalonians",
        alias: "2Th",
    },
    Book {
        name: "1 Timothy",
        alias: "1Tim",
    },
    Book {
        name: "2 Timothy",
        alias: "2Tim",
    },
    Book {
        name: "Titus",
        alias: "Titus",
    },
    Book {
        name: "Philemon",
        alias: "Phmn",
    },
    Book {
        name: "Hebrews",
        alias: "Heb",
    },
    Book {
        name: "James",
        alias: "Jas",
    },
    Book {
        name: "1 Peter",
        alias: "1Pet",
    },
    Book {
        name: "2 Peter",
        alias: "2Pet",
    },
    Book {
        name: "1 John",
        alias: "1Jn",
    },
    Book {
        name: "2 John",
        alias: "2Jn",
    },
    Book {
        name: "3 John",
        alias: "3Jn",
    },
    Book {
        name: "Jude",
        alias: "Jude",
    },
    Book {
        name: "Revelation",
        alias: "Rev",
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceError {
    InvalidReference,
    MalformedOutput,
    UnexpectedBook,
    DuplicateVerse,
    NoVerses,
    UnsupportedSource,
    UnsupportedLocation,
    Bundled(BundledError),
}

impl std::fmt::Display for SourceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for SourceError {}

impl From<BundledError> for SourceError {
    fn from(error: BundledError) -> Self {
        match error {
            BundledError::InvalidReference => SourceError::InvalidReference,
            other => SourceError::Bundled(other),
        }
    }
}

/// Canonical book name for a name, alias or source spelling (ASCII case-insensitive).
pub fn canonical(name: &str) -> Option<&'static str> {
    // Preserve the original NT aliases (notably Phi) ahead of the union.
    for book in &BOOKS {
        if name.eq_ignore_ascii_case(book.name) || name.eq_ignore_ascii_case(book.alias) {
            return Some(book.name);
        }
    }
    for entry in CATALOG {
        if name.eq_ignore_ascii_case(entry.name) {
            return Some(entry.name);
        }
        for (query, alias) in entry.query_names.iter().zip(entry.aliases.iter()) {
            if query.is_some_and(|n| name.eq_ignore_ascii_case(n))
                || alias.is_some_and(|n| name.eq_ignore_ascii_case(n))
            {
                return Some(entry.name);
            }
        }
    }
    None
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub book: &'static str,
    pub query: String,
}

impl Reference {
    pub fn new(raw: &str) -> Result<Reference, SourceError> {
        let colon = raw.find(':').unwrap_or(raw.len());
        let book = canonical(&raw[..colon]).ok_or(SourceError::InvalidReference)?;
        let suffix = &raw[colon..];
        Selection::new(suffix)?;
        Ok(Reference {
            book,
            query: format!("{book}{suffix}"),
        })
    }

    /// The part of the query after the book name.
    fn suffix(&self) -> &str {
        &self.query[self.query.find(':').unwrap_or(self.query.len())..]
    }
}

pub fn catalog_entry(name: &str) -> Option<&'static Entry> {
    let normalized = canonical(name)?;
    CATALOG.iter().find(|entry| entry.name == normalized)
}

/// Availability is based on observed chapter sets, not merely their maxima.
pub fn available(reference: &Reference, tool_index: usize) -> bool {
    if tool_index >= TOOLS.len() {
        return false;
    }
    let Some(entry) = catalog_entry(reference.book) else {
        return false;
    };
    if entry.chapters[tool_index] == 0 {
        return false;
    }
    let Ok(selection) = Selection::new(reference.suffix()) else {
        return false;
    };
    entry.present_chapters[tool_index]
        .iter()
        .any(|&chapter| selection.has_chapter(chapter))
}

pub fn streams(reference: &Reference) -> Result<[Vec<Verse>; 3], SourceError> {
    let mut result: [Vec<Verse>; 3] = Default::default();
    for (index, tool) in TOOLS.iter().enumerate() {
        if available(reference, index) {
            result[index] = fetch(tool, reference)?;
        }
    }
    Ok(result)
}

pub fn fetch(tool: &str, reference: &Reference) -> Result<Vec<Verse>, SourceError> {
    let index = TOOLS
        .iter()
        .position(|name| *name == tool)
        .ok_or(SourceError::UnsupportedSource)?;
    if !available(reference, index) {
        return Err(SourceError::UnsupportedLocation);
    }
    let entry = catalog_entry(reference.book).ok_or(SourceError::InvalidReference)?;
    let raw_book = entry.query_names[index].ok_or(SourceError::UnsupportedLocation)?;
    if index == 0 && reference.book == "Psalms" {
        return fetch_kjv_psalms(raw_book, reference);
    }
    let rows = bundled::query(index, raw_book, reference.suffix())?;
    Ok(rows
        .into_iter()
        .map(|row| Verse {
            book: reference.book.to_string(),
            chapter: row.chapter,
            number: row.number,
            text: row.text.into_owned(),
            label: None,
        })
        .collect())
}

/// The KJV numbers Psalms by the Hebrew (Masoretic) text; grb and vul use the Greek/Latin
/// (LXX/Vulgate) numbering, which Kata treats as authoritative. Maps a KJV psalm verse to its
/// LXX psalm and an ordering position within it. Merged psalms place the second KJV psalm after
/// the first; split psalms restart at 1. Verse numbers inside a psalm are not cross-mapped (the
/// Greek and Latin count superscriptions as verses), so the KJV keeps its own labels.
pub fn kjv_psalm_to_lxx(chapter: u16, verse: u16) -> Option<Label> {
    const PSALM_9_VERSES: u16 = 20;
    const PSALM_114_VERSES: u16 = 8;
    let at = |chapter, number| Some(Label { chapter, number });
    match chapter {
        1..=9 | 148..=150 => at(chapter, verse),
        10 => at(9, verse + PSALM_9_VERSES),
        11..=113 | 117..=146 => at(chapter - 1, verse),
        114 => at(113, verse),
        115 => at(113, verse + PSALM_114_VERSES),
        116 if verse <= 9 => at(114, verse),
        116 => at(115, verse - 9),
        147 if verse <= 11 => at(146, verse),
        147 => at(147, verse - 11),
        _ => None,
    }
}

/// The reference's chapter/verse selection is in LXX numbering. Read the whole KJV Psalter,
/// relocate each verse, and keep those the selection matches.
fn fetch_kjv_psalms(raw_book: &str, reference: &Reference) -> Result<Vec<Verse>, SourceError> {
    let selection = Selection::new(reference.suffix())?;
    let rows = bundled::query(0, raw_book, "")?;
    let mut verses = Vec::new();
    for row in rows {
        let at = kjv_psalm_to_lxx(row.chapter, row.number).ok_or(SourceError::MalformedOutput)?;
        if !selection.matches(at.chapter, at.number) {
            continue;
        }
        verses.push(Verse {
            book: reference.book.to_string(),
            chapter: at.chapter,
            number: at.number,
            text: row.text.into_owned(),
            label: Some(Label {
                chapter: row.chapter,
                number: row.number,
            }),
        });
    }
    Ok(verses)
}

/// Strict parser for tool-style output (`Book` header lines, then `chapter:verse<TAB>text`).
pub fn parse(output: &str, expected_book: &str) -> Result<Vec<Verse>, SourceError> {
    parse_impl(output, expected_book, false, false)
}

/// Installed tools prefix-match books, so select the exact canonical header. Greek repeated
/// labels are real data: join their text with one space, in source order, without inventing
/// labels for additions or misnumbered rows.
pub fn parse_source(
    output: &str,
    expected_book: &str,
    greek: bool,
) -> Result<Vec<Verse>, SourceError> {
    parse_impl(output, expected_book, true, greek)
}

fn parse_impl(
    output: &str,
    expected_book: &str,
    select_book: bool,
    greek: bool,
) -> Result<Vec<Verse>, SourceError> {
    if output.contains("Unknown reference:") {
        return Err(SourceError::InvalidReference);
    }
    let mut result: Vec<Verse> = Vec::new();
    let (mut have_header, mut selected, mut saw_expected) = (false, false, false);
    for raw_line in output.split('\n') {
        // vul contains CRLF verse records. Strip only the line-ending CR; embedded controls
        // are still rejected before terminal rendering.
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line);
        if line.bytes().any(|ch| (ch < 32 && ch != b'\t') || ch == 127) {
            return Err(SourceError::MalformedOutput);
        }
        if line.is_empty() {
            continue;
        }
        if let Some(tab) = line.find('\t') {
            if !have_header {
                return Err(SourceError::MalformedOutput);
            }
            let colon = line[..tab].find(':').ok_or(SourceError::MalformedOutput)?;
            let chapter: u16 = line[..colon]
                .parse()
                .map_err(|_| SourceError::MalformedOutput)?;
            let number: u16 = line[colon + 1..tab]
                .parse()
                .map_err(|_| SourceError::MalformedOutput)?;
            // The KJV Sirach exporter appends a lone empty 0:0 sentinel. It is not a verse or a
            // populated chapter; no other empty row is accepted.
            if select_book
                && selected
                && expected_book == "Sirach"
                && chapter == 0
                && number == 0
                && tab + 1 == line.len()
            {
                continue;
            }
            if (!select_book && (chapter == 0 || number == 0)) || tab + 1 == line.len() {
                return Err(SourceError::MalformedOutput);
            }
            if !selected {
                continue;
            }
            let text = &line[tab + 1..];
            match result
                .iter_mut()
                .find(|v| v.chapter == chapter && v.number == number)
            {
                Some(existing) => {
                    if !greek {
                        return Err(SourceError::DuplicateVerse);
                    }
                    existing.text = format!("{} {}", existing.text, text);
                }
                None => result.push(Verse {
                    book: expected_book.to_string(),
                    chapter,
                    number,
                    text: text.to_string(),
                    label: None,
                }),
            }
        } else {
            let name = canonical(line).ok_or(SourceError::MalformedOutput)?;
            selected = name == expected_book;
            if !selected && !select_book {
                return Err(SourceError::UnexpectedBook);
            }
            if selected {
                saw_expected = true;
            }
            have_header = true;
        }
    }
    if !saw_expected && have_header {
        return Err(SourceError::UnexpectedBook);
    }
    if result.is_empty() {
        return Err(SourceError::NoVerses);
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn label(chapter: u16, number: u16) -> Label {
        Label { chapter, number }
    }
    fn reference(raw: &str) -> Reference {
        Reference::new(raw).unwrap()
    }

    #[test]
    fn kjv_psalms_follow_greek_and_latin_psalm_numbering_with_their_own_verse_labels() {
        // (lxx reference, first label, last label, count)
        let cases = [
            ("Psalms:8", label(8, 1), label(8, 9), 9),
            ("Psalms:9", label(9, 1), label(10, 18), 38),
            ("Psalms:22", label(23, 1), label(23, 6), 6),
            ("Psalms:112", label(113, 1), label(113, 9), 9),
            ("Psalms:113", label(114, 1), label(115, 18), 26),
            ("Psalms:114", label(116, 1), label(116, 9), 9),
            ("Psalms:115", label(116, 10), label(116, 19), 10),
            ("Psalms:116", label(117, 1), label(117, 2), 2),
            ("Psalms:145", label(146, 1), label(146, 10), 10),
            ("Psalms:146", label(147, 1), label(147, 11), 11),
            ("Psalms:147", label(147, 12), label(147, 20), 9),
            ("Psalms:148", label(148, 1), label(148, 14), 14),
        ];
        for (lxx, first, last, count) in cases {
            let rows = streams(&reference(lxx)).unwrap();
            let kjv = &rows[0];
            assert_eq!(kjv.len(), count, "{lxx}");
            assert_eq!(kjv[0].label, Some(first), "{lxx}");
            assert_eq!(kjv[kjv.len() - 1].label, Some(last), "{lxx}");
            assert!(kjv.iter().all(|v| v.chapter == kjv[0].chapter), "{lxx}");
            // Greek and Latin are unchanged and keep their own numbering.
            assert!(
                rows[1..].iter().flatten().all(|v| v.label.is_none()),
                "{lxx}"
            );
        }
        let shepherd = streams(&reference("Psalms:22:1")).unwrap();
        assert_eq!(
            shepherd[0][0].text,
            "The LORD is my shepherd; I shall not want."
        );
        assert!(shepherd[2][0].text.starts_with("Psalmus David."));
        let extra = streams(&reference("Psalms:151")).unwrap();
        assert!(extra[0].is_empty() && !extra[1].is_empty());
        // Every KJV psalm verse appears exactly once under a distinct LXX position.
        let whole = streams(&reference("Psalms")).unwrap();
        assert_eq!(whole[0].len(), 2461);
        let mut positions: Vec<(u16, u16)> =
            whole[0].iter().map(|v| (v.chapter, v.number)).collect();
        positions.sort_unstable();
        positions.dedup();
        assert_eq!(positions.len(), 2461);
    }

    #[test]
    fn native_absent_verse_does_not_block_translations_with_that_label() {
        let rows = streams(&reference("Susanna:1:1")).unwrap();
        assert_eq!((rows[0].len(), rows[1].len(), rows[2].len()), (1, 0, 0));
        let absent = streams(&reference("John:65535")).unwrap();
        assert!(absent.iter().all(Vec::is_empty));
    }

    #[test]
    fn native_chapter_lists_include_later_available_chapters() {
        let rows = streams(&reference("Jeremiah:2,3,5-6")).unwrap();
        assert!(!rows[1].is_empty());
        assert!(rows[1].iter().all(|v| matches!(v.chapter, 3 | 5 | 6)));
    }

    #[test]
    fn native_cross_chapter_ranges_do_not_suppress_a_source_missing_first_chapter() {
        let rows = streams(&reference("Jeremiah:2:1-3:3")).unwrap();
        assert!(!rows[1].is_empty());
        assert!(rows[1].iter().all(|v| v.chapter == 3 && v.number <= 3));
    }

    #[test]
    fn native_cross_chapter_verse_ranges_retain_endpoint_labels() {
        for verses in streams(&reference("John:1:50-2:3")).unwrap() {
            assert_eq!(verses.len(), 5);
            assert_eq!(verses[0].number, 50);
            assert_eq!((verses[4].chapter, verses[4].number), (2, 3));
        }
    }

    #[test]
    fn native_verse_lists_and_ranges_select_labels_once_in_source_order() {
        for verses in streams(&reference("John:1:5,1-3,3")).unwrap() {
            assert_eq!(
                verses.iter().map(|v| v.number).collect::<Vec<_>>(),
                [1, 2, 3, 5]
            );
        }
    }

    #[test]
    fn native_chapter_ranges_include_available_chapters_across_source_gaps() {
        let rows = streams(&reference("Jeremiah:2-3")).unwrap();
        assert!(!rows[1].is_empty());
        assert!(rows[1].iter().all(|v| v.chapter == 3));
        assert_eq!(rows[0][0].chapter, 2);
    }

    #[test]
    fn bundled_fetch_works_without_external_executables() {
        let verses = fetch("kjv", &reference("John:1:1")).unwrap();
        assert_eq!(verses.len(), 1);
        assert_eq!(
            verses[0].text,
            "In the beginning was the Word, and the Word was with God, and the Word was God."
        );
    }

    #[test]
    fn parse_utf8_verses_using_their_labels() {
        let verses = parse("John\n1:1\tἘν ἀρχῇ\n1:3\tthird\n", "John").unwrap();
        assert_eq!(verses.len(), 2);
        assert_eq!(verses[1].number, 3);
        assert_eq!(verses[0].text, "Ἐν ἀρχῇ");
    }

    #[test]
    fn vulgate_crlf_rows_retain_text_without_terminal_controls() {
        let verses = parse("John\n1:1\t[In principio erat Verbum.\r\n", "John").unwrap();
        assert_eq!(verses[0].text, "[In principio erat Verbum.");
    }

    #[test]
    fn exit_zero_error_output_is_not_a_passage() {
        assert_eq!(
            parse("Unknown reference: invalid\n", "John"),
            Err(SourceError::InvalidReference)
        );
    }

    #[test]
    fn unexpected_book_and_duplicate_labels_are_rejected() {
        assert_eq!(
            parse("John\n1:1\ttext\n", "Matthew"),
            Err(SourceError::UnexpectedBook)
        );
        assert_eq!(
            parse("John\n1:1\tfirst\n1:1\tsecond\n", "John"),
            Err(SourceError::DuplicateVerse)
        );
    }

    #[test]
    fn control_sequences_and_malformed_rows_are_rejected() {
        assert_eq!(
            parse("John\n1:1\t\x1b[2J\n", "John"),
            Err(SourceError::MalformedOutput)
        );
        assert_eq!(
            parse("John\n1:1 text\n", "John"),
            Err(SourceError::MalformedOutput)
        );
    }

    #[test]
    fn source_sirach_sentinel_and_explicit_zero_chapter_preserve_real_prologue() {
        assert_eq!(
            parse_source("Sirach\n1:1\tfirst\n0:0\t\n", "Sirach", false)
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            parse_source("Sirach\n1:1\t\n", "Sirach", false),
            Err(SourceError::MalformedOutput)
        );
        let prologue = streams(&reference("Sirach:0")).unwrap();
        assert!(!prologue[1].is_empty());
        assert!(prologue.iter().flatten().all(|v| v.chapter == 0));
    }

    #[test]
    fn source_parser_preserves_repeated_greek_labels_and_zero_labels() {
        let verses = parse_source(
            "Sirach\n0:0\tprologue\n1:1\tfirst\n1:1\ta addition\n",
            "Sirach",
            true,
        )
        .unwrap();
        assert_eq!(verses.len(), 2);
        assert_eq!(verses[0].chapter, 0);
        assert_eq!(verses[1].text, "first a addition");
        let selected = parse_source(
            "Sussana\n1:6\told Greek\nSussana (Theodotion)\n1:1\tvariant\n",
            "Susanna",
            true,
        )
        .unwrap();
        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].text, "old Greek");
        assert_eq!(
            parse_source("Sirach\nnot a source heading\n", "Sirach", true),
            Err(SourceError::MalformedOutput)
        );
    }

    #[test]
    fn streams_uses_source_availability_and_source_specific_names() {
        let rows = streams(&reference("Judges (Vaticanus):1")).unwrap();
        assert!(rows[0].is_empty() && !rows[1].is_empty() && rows[2].is_empty());
        let gap = reference("Jeremiah:2");
        let gap_rows = streams(&gap).unwrap();
        assert!(!gap_rows[0].is_empty() && gap_rows[1].is_empty() && !gap_rows[2].is_empty());
        assert_eq!(fetch("grb", &gap), Err(SourceError::UnsupportedLocation));
        for verses in streams(&reference("2 Kings:25")).unwrap() {
            assert!(!verses.is_empty());
        }
    }

    #[test]
    fn old_testament_and_variant_aliases_canonicalize() {
        let reference = Reference::new("Gen:50").unwrap();
        assert_eq!(
            (reference.book, reference.query.as_str()),
            ("Genesis", "Genesis:50")
        );
        assert_eq!(canonical("2 King"), Some("2 Kings"));
        assert_eq!(canonical("SusT"), Some("Susanna (Theodotion)"));
        assert_eq!(canonical("Sussana"), Some("Susanna"));
    }

    #[test]
    fn every_discovered_book_fetches_with_exact_canonical_labels_and_observed_chapters() {
        let mut counts = [0usize; 3];
        for entry in CATALOG {
            assert_eq!(canonical(entry.name), Some(entry.name));
            for (index, tool) in TOOLS.iter().enumerate() {
                let Some(query_name) = entry.query_names[index] else {
                    continue;
                };
                assert_eq!(canonical(query_name), Some(entry.name));
                assert_eq!(canonical(entry.aliases[index].unwrap()), Some(entry.name));
                let verses = fetch(tool, &Reference::new(entry.name).unwrap()).unwrap();
                assert!(!verses.is_empty(), "{} {tool}", entry.name);
                for verse in &verses {
                    assert_eq!(verse.book, entry.name);
                    assert!(
                        entry.present_chapters[index].contains(&verse.chapter),
                        "{} {tool} {}",
                        entry.name,
                        verse.chapter
                    );
                }
                for chapter in entry.present_chapters[index] {
                    assert!(
                        verses.iter().any(|v| v.chapter == *chapter),
                        "{} {tool} chapter {chapter}",
                        entry.name
                    );
                }
                counts[index] += 1;
            }
        }
        assert_eq!(counts, [79, 87, 68]);
        assert_eq!(CATALOG.len(), 89);
        assert_eq!(BOOKS.len(), 27);
    }

    #[test]
    fn reference_rejects_malformed_numeric_suffixes_before_availability() {
        for raw in [
            "John::1",
            "John:1-",
            "John:1,,2",
            "John:-1",
            "John:65536",
            "John:2-1",
            "John:1:3-1",
            "John:1:1-2:3:4",
            "John:1:2:3",
            "John:1-2:3",
            "John:2:1-1:50",
            "John:1:1-2:65536",
        ] {
            assert_eq!(
                Reference::new(raw),
                Err(SourceError::InvalidReference),
                "{raw}"
            );
        }
    }

    #[test]
    fn aliases_canonicalize_and_cannot_inject_shell_arguments() {
        let reference = Reference::new("Phi:1:1-3").unwrap();
        assert_eq!(
            (reference.book, reference.query.as_str()),
            ("Philippians", "Philippians:1:1-3")
        );
        assert_eq!(
            Reference::new("John:1;echo x"),
            Err(SourceError::InvalidReference)
        );
    }
}
