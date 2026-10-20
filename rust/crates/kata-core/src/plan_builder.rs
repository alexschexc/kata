//! Plan builder logic, ported from `src/plan_builder.zig`: a plan is one or more streams finished
//! together in a chosen number of days; `phases` computes the even per-day rates the plan format
//! needs. A stream is either Bible books read back to back, or one ingested library book read in
//! chapters or sections (see `reading_units`).

use crate::book_catalog::Entry;
use crate::plan::{Book, Config, Phase, Plan, PlanError, Stream};
use crate::source::CATALOG;

/// An ingested book offered to the builder, with its unit labels.
#[derive(Debug, Clone)]
pub struct LibraryOption {
    pub file: String,
    pub title: String,
    pub chapter_name: String,
    pub section_name: String,
    pub chapters: Vec<String>,
    pub sections: Vec<String>,
}

impl LibraryOption {
    pub fn labels(&self, unit: &str) -> &[String] {
        if unit == "section" {
            &self.sections
        } else {
            &self.chapters
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum BuildError {
    InvalidPlan,
    TooManyDays,
    Plan(String),
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            BuildError::Plan(message) => write!(f, "{message}"),
            other => write!(f, "{other:?}"),
        }
    }
}

impl std::error::Error for BuildError {}

impl From<PlanError> for BuildError {
    fn from(e: PlanError) -> Self {
        BuildError::Plan(e.to_string())
    }
}

/// Chapter count used for a catalog book: the largest of its sources.
pub fn chapters_of(entry: &Entry) -> u16 {
    entry.chapters.into_iter().max().unwrap_or(0)
}

/// Plan book names must be plain letters, digits and spaces.
pub fn plannable(entry: &Entry) -> bool {
    entry
        .name
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || c == b' ')
        && chapters_of(entry) > 0
}

pub fn total(books: &[Book]) -> usize {
    books.iter().map(|b| usize::from(b.chapters)).sum()
}

/// Phases finishing every stream in exactly `days` days, each stream's daily rate differing by at
/// most one unit (extra units come first). Every day must read something, so `days` ≤ the
/// longest stream.
pub fn phases(streams: &[Stream], days: u16) -> Result<Vec<Phase>, BuildError> {
    if streams.is_empty() || days == 0 {
        return Err(BuildError::InvalidPlan);
    }
    let longest = streams.iter().map(|s| total(&s.books)).max().unwrap_or(0);
    if usize::from(days) > longest {
        return Err(BuildError::TooManyDays);
    }
    let mut cuts: Vec<u16> = vec![0, days];
    for stream in streams {
        let extra = (total(&stream.books) % usize::from(days)) as u16;
        if extra > 0 {
            cuts.push(extra);
        }
    }
    cuts.sort_unstable();
    let mut out = Vec::new();
    for pair in cuts.windows(2) {
        let (from, to) = (pair[0], pair[1]);
        if to == from {
            continue;
        }
        let rates = streams
            .iter()
            .map(|stream| {
                let units = total(&stream.books);
                (units / usize::from(days)
                    + usize::from(usize::from(from) < units % usize::from(days)))
                    as u16
            })
            .collect();
        out.push(Phase {
            days: to - from,
            rates,
        });
    }
    Ok(out)
}

/// Plan JSON, validated by parsing it back with the real plan loader.
pub fn render(
    name: &str,
    repeat: bool,
    streams: &[Stream],
    days: u16,
) -> Result<String, BuildError> {
    let config = Config {
        name: name.to_string(),
        repeat,
        streams: streams.to_vec(),
        phases: phases(streams, days)?,
    };
    let json =
        serde_json::to_string_pretty(&config).map_err(|e| BuildError::Plan(e.to_string()))?;
    Plan::new(json.as_bytes())?;
    Ok(json)
}

pub struct Preset {
    pub label: &'static str,
    pub first: &'static str,
    pub last: &'static str,
}

pub const PRESETS: [Preset; 7] = [
    Preset {
        label: "Gospels (Matthew–John)",
        first: "Matthew",
        last: "John",
    },
    Preset {
        label: "New Testament (Matthew–Revelation)",
        first: "Matthew",
        last: "Revelation",
    },
    Preset {
        label: "Epistles (Romans–Jude)",
        first: "Romans",
        last: "Jude",
    },
    Preset {
        label: "Old Testament (Genesis–Malachi)",
        first: "Genesis",
        last: "Malachi",
    },
    Preset {
        label: "Torah (Genesis–Deuteronomy)",
        first: "Genesis",
        last: "Deuteronomy",
    },
    Preset {
        label: "Psalms",
        first: "Psalms",
        last: "Psalms",
    },
    Preset {
        label: "Wisdom (Job–Song of Solomon)",
        first: "Job",
        last: "Song of Solomon",
    },
];

pub fn catalog_index(name: &str) -> usize {
    CATALOG
        .iter()
        .position(|entry| entry.name == name)
        .expect("preset names are catalog books")
}

pub fn book_of(entry: &Entry) -> Book {
    Book {
        name: entry.name.to_string(),
        chapters: chapters_of(entry),
    }
}

/// Builder-side stream: Bible books, or one library book (`library` indexes the options) read in
/// `unit`s.
#[derive(Debug, Clone)]
pub struct Draft {
    pub books: Vec<Book>,
    pub library: Option<usize>,
    pub unit: String,
}

impl Default for Draft {
    fn default() -> Self {
        Draft {
            books: Vec::new(),
            library: None,
            unit: "chapter".into(),
        }
    }
}

impl Draft {
    pub fn stream(&self, options: &[LibraryOption]) -> Stream {
        match self.library {
            Some(i) => {
                let option = &options[i];
                let units = option.labels(&self.unit).len().min(usize::from(u16::MAX)) as u16;
                Stream {
                    books: vec![Book {
                        name: option.title.clone(),
                        chapters: units,
                    }],
                    library: Some(option.file.clone()),
                    unit: Some(self.unit.clone()),
                }
            }
            None => Stream {
                books: self.books.clone(),
                library: None,
                unit: None,
            },
        }
    }

    pub fn describe(&self, options: &[LibraryOption]) -> String {
        if let Some(i) = self.library {
            let option = &options[i];
            let name = if self.unit == "section" {
                &option.section_name
            } else {
                &option.chapter_name
            };
            return format!(
                "{} · {} {}",
                option.title,
                option.labels(&self.unit).len(),
                name
            );
        }
        let books = &self.books;
        let (Some(first), Some(last)) = (books.first(), books.last()) else {
            return "no books yet".into();
        };
        let span = if books.len() == 1 {
            first.name.clone()
        } else {
            format!("{} … {} ({} books)", first.name, last.name, books.len())
        };
        format!("Bible: {span} · {} chapters", total(books))
    }

    pub fn is_empty(&self) -> bool {
        self.library.is_none() && self.books.is_empty()
    }
}

pub fn default_unit(option: &LibraryOption) -> &'static str {
    if option.sections.len() > option.chapters.len() {
        "section"
    } else {
        "chapter"
    }
}

pub fn slug(name: &str) -> String {
    let mut out = String::new();
    for c in name.bytes() {
        if c.is_ascii_alphanumeric() {
            out.push(c.to_ascii_lowercase() as char);
        } else if !out.is_empty() && !out.ends_with('-') {
            out.push('-');
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() { "plan".into() } else { out }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book(name: &str, chapters: u16) -> Book {
        Book {
            name: name.into(),
            chapters,
        }
    }

    #[test]
    fn phases_finish_every_stream_together_with_even_rates() {
        let gospels = vec![
            book("Matthew", 28),
            book("Mark", 16),
            book("Luke", 24),
            book("John", 21),
        ];
        let epistles = vec![book("Romans", 16), book("James", 5)];
        let streams = [
            Stream {
                books: gospels,
                library: None,
                unit: None,
            },
            Stream {
                books: epistles,
                library: None,
                unit: None,
            },
        ];
        let json = render("Two streams", false, &streams, 30).unwrap();
        let plan = Plan::new(json.as_bytes()).unwrap();
        assert_eq!(plan.total_days(), 30);
        // 89 chapters / 30 days = 2.97 → 29 days of 3, 1 of 2; 21 / 30 → 21 of 1, 9 of 0.
        let mut seen = [0usize; 2];
        for day in 0..30 {
            let chapters = plan.assignments(day).unwrap();
            assert!(!chapters.is_empty());
            for c in &chapters {
                seen[c.stream] += 1;
            }
        }
        assert_eq!(seen, [89, 21]);
        let last = plan.assignments(29).unwrap();
        let tail = last.last().unwrap();
        assert_eq!((tail.book.as_str(), tail.chapter), ("John", 21));
        assert_eq!(phases(&streams, 90).err(), Some(BuildError::TooManyDays));
    }

    #[test]
    fn library_streams_render_with_file_and_unit_and_titles_may_use_punctuation() {
        let options = [LibraryOption {
            file: "bms".into(),
            title: "Building Microservices: 2nd Ed.".into(),
            chapter_name: "chapters".into(),
            section_name: "sections".into(),
            chapters: vec!["1".into(), "2".into()],
            sections: ["1a", "1b", "2a", "2b", "2c"].map(String::from).to_vec(),
        }];
        let draft = Draft {
            library: Some(0),
            unit: "section".into(),
            ..Draft::default()
        };
        let streams = [
            draft.stream(&options),
            Stream {
                books: vec![book("John", 21)],
                library: None,
                unit: None,
            },
        ];
        let json = render("Mixed", false, &streams, 5).unwrap();
        assert!(json.contains("\"library\": \"bms\""));
        assert!(!json.contains("\"library\": null"));
        let plan = Plan::new(json.as_bytes()).unwrap();
        let day = plan.assignments(0).unwrap();
        assert_eq!((day[0].stream, day[0].chapter), (0, 1));
    }

    #[test]
    fn catalog_presets_resolve_to_plannable_books_and_slugs_are_clean() {
        let nt = (catalog_index("Matthew")..=catalog_index("Revelation"))
            .filter(|&i| plannable(&CATALOG[i]))
            .count();
        assert_eq!(nt, 27);
        assert_eq!(slug("One chapter, a day!"), "one-chapter-a-day");
        assert_eq!(slug("!!!"), "plan");
    }
}
