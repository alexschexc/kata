//! Free-reading support, ported from `src/reading.zig`: per-chapter state paths, verse anchoring,
//! reference parsing and the last-place bookmark.

use crate::layout::Row;
use crate::library::{self, Location};
use crate::source::Reference;
use crate::state::{Position, State};
use crate::wyhash;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::Path;

#[derive(Debug, PartialEq, Eq)]
pub enum ReadingError {
    InvalidLocation,
    InvalidVerse,
    InvalidReference,
    InvalidFreeBookmark,
    Io(String),
}

impl std::fmt::Display for ReadingError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ReadingError::Io(message) => write!(f, "{message}"),
            other => write!(f, "{other:?}"),
        }
    }
}

impl std::error::Error for ReadingError {}

impl From<io::Error> for ReadingError {
    fn from(e: io::Error) -> Self {
        ReadingError::Io(e.to_string())
    }
}

#[derive(Serialize, Deserialize)]
struct Bookmark {
    version: u8,
    title_id: String,
    section: String,
    chapter: u16,
}

/// State file for one free-reading chapter, isolated from plan state and other chapters.
pub fn path(
    base: &str,
    at: Location,
    override_query: Option<&str>,
) -> Result<String, ReadingError> {
    let full = library::reference(at).map_err(|_| ReadingError::InvalidLocation)?;
    let reference = Reference::new(override_query.unwrap_or(&full))
        .map_err(|_| ReadingError::InvalidReference)?;
    let id = library::titles()[at.title].id;
    let hash = wyhash::hash(wyhash::hash(0, id.as_bytes()), reference.query.as_bytes());
    Ok(format!("{base}.library/{id}/{hash:x}.json"))
}

/// Points every pane at the row of the requested starting verse (no-op without a verse).
pub fn anchor(rows: &[Row], state: &mut State, at: Location) -> Result<(), ReadingError> {
    let Some(verse) = at.verse else { return Ok(()) };
    if !library::valid(at) {
        return Err(ReadingError::InvalidLocation);
    }
    let name = library::titles()[at.title].sections[at.section].name;
    for (index, row) in rows.iter().enumerate() {
        if row.book == name && row.chapter == at.chapter && row.number == verse {
            state.positions = [Position {
                row: index,
                line: 0,
            }; 3];
            return Ok(());
        }
    }
    Err(ReadingError::InvalidVerse)
}

/// The free-reading location named by a reference such as "John:3".
pub fn from_reference(raw: &str) -> Result<Location, ReadingError> {
    let reference = Reference::new(raw).map_err(|_| ReadingError::InvalidReference)?;
    for (title_index, title) in library::titles().iter().enumerate() {
        for (section_index, section) in title.sections.iter().enumerate() {
            if section.name != reference.book {
                continue;
            }
            let rest = &reference.query[reference.book.len()..];
            let chapter = match rest.split([':', '-', ',']).find(|part| !part.is_empty()) {
                Some(number) => number
                    .parse::<u16>()
                    .map_err(|_| ReadingError::InvalidReference)?,
                None => 1,
            };
            let at = Location {
                title: title_index,
                section: section_index,
                chapter,
                verse: None,
            };
            library::reference(at).map_err(|_| ReadingError::InvalidLocation)?;
            return Ok(at);
        }
    }
    Err(ReadingError::InvalidReference)
}

/// The saved last free-reading place, or the default when none was saved.
pub fn load_bookmark(base: &str) -> Result<Location, ReadingError> {
    let bytes = match fs::read(format!("{base}.free-selection.json")) {
        Ok(bytes) if bytes.len() <= 65536 => bytes,
        Ok(_) => return Err(ReadingError::InvalidFreeBookmark),
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Location::default()),
        Err(e) => return Err(e.into()),
    };
    let bookmark: Bookmark =
        serde_json::from_slice(&bytes).map_err(|_| ReadingError::InvalidFreeBookmark)?;
    if bookmark.version != 1 {
        return Err(ReadingError::InvalidFreeBookmark);
    }
    for (title_index, title) in library::titles().iter().enumerate() {
        if title.id != bookmark.title_id {
            continue;
        }
        for (section_index, section) in title.sections.iter().enumerate() {
            if section.name != bookmark.section {
                continue;
            }
            let at = Location {
                title: title_index,
                section: section_index,
                chapter: bookmark.chapter,
                verse: None,
            };
            library::reference(at).map_err(|_| ReadingError::InvalidFreeBookmark)?;
            return Ok(at);
        }
    }
    Err(ReadingError::InvalidFreeBookmark)
}

pub fn save_bookmark(base: &str, at: Location) -> Result<(), ReadingError> {
    library::reference(at).map_err(|_| ReadingError::InvalidLocation)?;
    let title = &library::titles()[at.title];
    let bookmark = Bookmark {
        version: 1,
        title_id: title.id.to_string(),
        section: title.sections[at.section].name.to_string(),
        chapter: at.chapter,
    };
    let bytes = serde_json::to_vec(&bookmark).map_err(|e| ReadingError::Io(e.to_string()))?;
    let filename = format!("{base}.free-selection.json");
    if let Some(parent) = Path::new(&filename)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = format!("{filename}.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, &filename)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nt(section: usize, chapter: u16, verse: Option<u16>) -> Location {
        Location {
            title: 0,
            section,
            chapter,
            verse,
        }
    }

    #[test]
    fn free_chapter_state_is_isolated_from_base_and_other_chapter_state() {
        let first = path("/tmp/progress.json", nt(3, 20, None), None).unwrap();
        let next = path("/tmp/progress.json", nt(3, 21, None), None).unwrap();
        assert!(first.starts_with("/tmp/progress.json.library/new-testament/"));
        assert_ne!(first, next);
    }

    #[test]
    fn starting_verse_anchors_all_panes_without_changing_plan_counters() {
        let rows = [Row::new("John", 20, 1), Row::new("John", 20, 5)];
        let mut state = State {
            next_day: 87,
            completed_on: 20261007,
            ..State::default()
        };
        anchor(&rows, &mut state, nt(3, 20, Some(5))).unwrap();
        assert_eq!(state.positions[0].row, 1);
        assert_eq!(state.positions[0], state.positions[2]);
        assert_eq!((state.next_day, state.completed_on), (87, 20261007));
        let before = state;
        assert_eq!(
            anchor(&rows, &mut state, nt(3, 20, Some(8))),
            Err(ReadingError::InvalidVerse)
        );
        assert_eq!(state, before);
    }

    #[test]
    fn full_collection_reference_and_anchor_retain_source_chapter_zero() {
        let at = from_reference("Sirach:0").unwrap();
        assert_eq!(at.chapter, 0);
        let rows = [Row::new("Sirach", 0, 1)];
        let mut state = State::default();
        anchor(
            &rows,
            &mut state,
            Location {
                verse: Some(1),
                ..at
            },
        )
        .unwrap();
    }

    #[test]
    fn bookmark_round_trips_and_a_missing_file_means_the_default_place() {
        let dir = std::env::temp_dir().join(format!("kata-reading-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let base = dir.join("state.json").to_string_lossy().into_owned();
        assert_eq!(load_bookmark(&base).unwrap(), Location::default());
        let at = from_reference("John:3").unwrap();
        save_bookmark(&base, at).unwrap();
        assert_eq!(load_bookmark(&base).unwrap(), at);
        let text = fs::read_to_string(format!("{base}.free-selection.json")).unwrap();
        assert_eq!(
            text,
            r#"{"version":1,"title_id":"new-testament","section":"John","chapter":3}"#
        );
        fs::remove_dir_all(dir).unwrap();
    }
}
