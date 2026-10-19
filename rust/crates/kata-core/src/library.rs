//! Titles (collections of books) and chapter locations, ported from `src/library.zig`.

use crate::source;
use std::sync::OnceLock;

#[derive(Debug, Clone)]
pub struct Section {
    pub name: &'static str,
    pub chapters: u16,
}

#[derive(Debug, Clone)]
pub struct Title {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub sections: Vec<Section>,
    pub tools: &'static [&'static str],
    pub supports_plans: bool,
}

const NT_COUNTS: [u16; 27] = [
    28, 16, 24, 21, 28, 16, 16, 13, 6, 6, 4, 4, 5, 3, 6, 4, 3, 1, 13, 5, 5, 3, 5, 1, 1, 1, 22,
];

/// Only implemented source adapters belong in this registry.
pub fn titles() -> &'static [Title] {
    static TITLES: OnceLock<Vec<Title>> = OnceLock::new();
    TITLES.get_or_init(|| {
        let nt = source::BOOKS
            .iter()
            .zip(NT_COUNTS)
            .map(|(book, chapters)| Section {
                name: book.name,
                chapters,
            })
            .collect();
        let bible = source::CATALOG
            .iter()
            .map(|entry| Section {
                name: entry.name,
                chapters: *entry.chapters.iter().max().unwrap(),
            })
            .collect();
        vec![
            Title {
                id: "new-testament",
                name: "New Testament",
                description: "Read the New Testament with KJV, Greek, and Latin sources.",
                sections: nt,
                tools: &source::TOOLS,
                supports_plans: true,
            },
            Title {
                id: "bible",
                name: "Bible",
                description: "All books available in the installed KJV, Greek, and Latin sources.",
                sections: bible,
                tools: &source::TOOLS,
                supports_plans: true,
            },
        ]
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Location {
    pub title: usize,
    pub section: usize,
    pub chapter: u16,
    pub verse: Option<u16>,
}

impl Default for Location {
    fn default() -> Self {
        Location {
            title: 0,
            section: 0,
            chapter: 1,
            verse: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidLocation;

impl std::fmt::Display for InvalidLocation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InvalidLocation")
    }
}

impl std::error::Error for InvalidLocation {}

pub fn valid(at: Location) -> bool {
    let Some(title) = titles().get(at.title) else {
        return false;
    };
    let Some(section) = title.sections.get(at.section) else {
        return false;
    };
    if at.chapter > section.chapters || at.verse == Some(0) {
        return false;
    }
    if at.title == 0 {
        at.chapter != 0
    } else {
        source::CATALOG[at.section]
            .present_chapters
            .iter()
            .any(|chapters| chapters.contains(&at.chapter))
    }
}

/// A full-chapter query; the verse is only a reader anchor.
pub fn reference(at: Location) -> Result<String, InvalidLocation> {
    if !valid(at) {
        return Err(InvalidLocation);
    }
    Ok(format!(
        "{}:{}",
        titles()[at.title].sections[at.section].name,
        at.chapter
    ))
}

/// Moves one chapter without wrapping or preserving a verse anchor.
pub fn adjacent(at: Location, forward: bool) -> Option<Location> {
    if !valid(at) {
        return None;
    }
    let sections = &titles()[at.title].sections;
    let mut next = Location { verse: None, ..at };
    let minimum: u16 = if at.title == 0 { 1 } else { 0 };
    loop {
        if forward {
            if next.chapter < sections[next.section].chapters {
                next.chapter += 1;
            } else {
                if next.section + 1 == sections.len() {
                    return None;
                }
                next.section += 1;
                next.chapter = minimum;
            }
        } else if next.chapter > minimum {
            next.chapter -= 1;
        } else {
            if next.section == 0 {
                return None;
            }
            next.section -= 1;
            next.chapter = sections[next.section].chapters;
        }
        if valid(next) {
            return Some(next);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn invalid_locations() -> Vec<Location> {
        let base = Location::default();
        vec![
            Location { chapter: 0, ..base },
            Location {
                chapter: 29,
                ..base
            },
            Location {
                title: titles().len(),
                ..base
            },
            Location {
                title: usize::MAX,
                ..base
            },
            Location {
                section: titles()[0].sections.len(),
                ..base
            },
            Location {
                section: usize::MAX,
                ..base
            },
            Location {
                chapter: u16::MAX,
                ..base
            },
            Location {
                verse: Some(0),
                ..base
            },
        ]
    }

    #[test]
    fn adjacent_rejects_invalid_locations_in_both_directions() {
        for at in invalid_locations() {
            assert_eq!(adjacent(at, true), None, "{at:?}");
            assert_eq!(adjacent(at, false), None, "{at:?}");
        }
    }

    #[test]
    fn adjacent_walks_chapters_in_both_directions_clearing_anchors() {
        let title = &titles()[0];
        for (section_index, section) in title.sections.iter().enumerate() {
            for chapter in 1..=section.chapters {
                let at = Location {
                    title: 0,
                    section: section_index,
                    chapter,
                    verse: Some(1),
                };
                let forward = if chapter < section.chapters {
                    Some(Location {
                        title: 0,
                        section: section_index,
                        chapter: chapter + 1,
                        verse: None,
                    })
                } else if section_index + 1 < title.sections.len() {
                    Some(Location {
                        title: 0,
                        section: section_index + 1,
                        chapter: 1,
                        verse: None,
                    })
                } else {
                    None
                };
                assert_eq!(adjacent(at, true), forward);
                let backward = if chapter > 1 {
                    Some(Location {
                        title: 0,
                        section: section_index,
                        chapter: chapter - 1,
                        verse: None,
                    })
                } else if section_index > 0 {
                    let chapters = title.sections[section_index - 1].chapters;
                    Some(Location {
                        title: 0,
                        section: section_index - 1,
                        chapter: chapters,
                        verse: None,
                    })
                } else {
                    None
                };
                assert_eq!(adjacent(at, false), backward);
            }
        }
    }

    #[test]
    fn reference_rejects_invalid_locations() {
        for at in invalid_locations() {
            assert_eq!(reference(at), Err(InvalidLocation), "{at:?}");
        }
    }

    #[test]
    fn reference_returns_a_full_canonical_chapter_even_with_a_verse_anchor() {
        assert_eq!(reference(Location::default()).unwrap(), "Matthew:1");
        let john = Location {
            section: 3,
            chapter: 20,
            verse: Some(7),
            ..Location::default()
        };
        assert_eq!(reference(john).unwrap(), "John:20");
        let numbered = Location {
            section: 6,
            chapter: 16,
            ..Location::default()
        };
        assert_eq!(reference(numbered).unwrap(), "1 Corinthians:16");
    }

    #[test]
    fn bible_zero_chapter_is_observed_and_all_locations_roundtrip() {
        let bible = &titles()[1];
        assert_eq!(bible.sections[0].name, "Genesis");
        assert_eq!(bible.sections.len(), source::CATALOG.len());
        let sirach = bible
            .sections
            .iter()
            .position(|s| s.name == "Sirach")
            .unwrap();
        let zero = Location {
            title: 1,
            section: sirach,
            chapter: 0,
            verse: None,
        };
        assert_eq!(reference(zero).unwrap(), "Sirach:0");
        let one = Location {
            title: 1,
            section: sirach,
            chapter: 1,
            verse: None,
        };
        assert_eq!(adjacent(one, false), Some(zero));
        for (index, section) in bible.sections.iter().enumerate() {
            for chapter in 0..=section.chapters {
                let at = Location {
                    title: 1,
                    section: index,
                    chapter,
                    verse: None,
                };
                if !valid(at) {
                    continue;
                }
                let query = reference(at).unwrap();
                let reference = source::Reference::new(&query).unwrap();
                assert!(
                    (0..3).any(|tool| source::available(&reference, tool)),
                    "{query}"
                );
                if let Some(next) = adjacent(at, true) {
                    assert_eq!(adjacent(next, false), Some(at));
                }
                if let Some(previous) = adjacent(at, false) {
                    assert_eq!(adjacent(previous, true), Some(at));
                }
            }
        }
    }

    #[test]
    fn registry_preserves_nt_bookmarks_and_adds_the_discovered_bible() {
        assert_eq!(titles().len(), 2);
        let title = &titles()[0];
        assert_eq!((title.id, title.name), ("new-testament", "New Testament"));
        assert!(!title.description.is_empty() && title.supports_plans);
        assert_eq!(title.sections.len(), source::BOOKS.len());
        for ((section, book), count) in title
            .sections
            .iter()
            .zip(source::BOOKS.iter())
            .zip(NT_COUNTS)
        {
            assert_eq!((section.name, section.chapters), (book.name, count));
        }
        assert_eq!(title.tools, &source::TOOLS);
    }
}
