//! Library start menu and place chooser, ported from `src/start_menu.zig`.

use crate::tui::{ChooseOptions, Tui};
use kata_core::catalog::Entry;
use kata_core::library::{self, Location};
use kata_core::plan::Plan;
use kata_core::source;
use std::io;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Plan(usize),
    Free(Location),
    Book(usize),
    Folders,
    SearchBooks,
    BuildPlan,
    Plans,
}

/// Ingested documents offered in the Library after the built-in titles.
#[derive(Debug, Clone, Default)]
pub struct Book {
    pub title: String,
    pub author: String,
    pub chapters: u32,
}

/// Titles shown in the Library: every title except the duplicated legacy New Testament.
fn visible_titles() -> Vec<usize> {
    library::titles()
        .iter()
        .enumerate()
        .filter(|(_, title)| title.id != "new-testament")
        .map(|(index, _)| index)
        .collect()
}

/// Maps a location in one title to the same section in another (by name), or that title's start.
fn menu_location(previous: Location, title_index: usize) -> Location {
    if previous.title == title_index {
        return previous;
    }
    if library::valid(previous) {
        let name = library::titles()[previous.title].sections[previous.section].name;
        for (index, section) in library::titles()[title_index].sections.iter().enumerate() {
            if section.name != name {
                continue;
            }
            let mapped = Location {
                title: title_index,
                section: index,
                chapter: previous.chapter,
                verse: previous.verse,
            };
            if library::valid(mapped) {
                return mapped;
            }
        }
    }
    Location {
        title: title_index,
        ..Location::default()
    }
}

fn other<E: std::fmt::Display>(error: E) -> io::Error {
    io::Error::other(error.to_string())
}

/// Book, chapter, then starting verse. None when the user backs out of the first stage.
pub fn choose_place(tui: &mut Tui, initial: Location) -> io::Result<Option<Location>> {
    #[derive(Clone, Copy)]
    enum Stage {
        Book,
        Chapter,
        Verse,
    }
    let mut at = initial;
    let titles = library::titles();
    let title = titles
        .get(at.title)
        .ok_or_else(|| io::Error::other("InvalidLocation"))?;
    let books: Vec<String> = title
        .sections
        .iter()
        .map(|s| format!("{} · {} chapters", s.name, s.chapters))
        .collect();
    let mut stage = Stage::Book;
    while !tui.is_interrupted() {
        match stage {
            Stage::Book => {
                let Some(section) = tui.choose("Choose book", title.name, &books, at.section, 1)?
                else {
                    return Ok(None);
                };
                if at.section != section {
                    at.chapter = 1;
                }
                at.section = section;
                stage = Stage::Chapter;
            }
            Stage::Chapter => {
                let section = &title.sections[at.section];
                let (mut labels, mut numbers, mut selected) = (Vec::new(), Vec::new(), 0);
                for chapter in 0..=section.chapters {
                    let candidate = Location {
                        title: at.title,
                        section: at.section,
                        chapter,
                        verse: None,
                    };
                    if library::reference(candidate).is_err() {
                        continue;
                    }
                    if at.chapter == chapter {
                        selected = labels.len();
                    }
                    numbers.push(chapter);
                    labels.push(format!("Chapter {chapter}"));
                }
                let Some(chapter) =
                    tui.choose("Choose chapter", section.name, &labels, selected, 1)?
                else {
                    stage = Stage::Book;
                    continue;
                };
                at.chapter = numbers[chapter];
                at.verse = None;
                stage = Stage::Verse;
            }
            Stage::Verse => {
                let query = library::reference(at).map_err(other)?;
                let reference = source::Reference::new(&query).map_err(other)?;
                let streams = source::streams(&reference).map_err(other)?;
                let Some(verses) = streams.iter().find(|stream| !stream.is_empty()) else {
                    return Err(io::Error::other("NoVerses"));
                };
                let mut labels =
                    vec!["Resume saved position, or chapter beginning if new".to_string()];
                for verse in verses {
                    labels.push(match verse.label {
                        Some(own) => {
                            format!("Verse {}:{} · {}", own.chapter, own.number, verse.text)
                        }
                        None => format!("Verse {} · {}", verse.number, verse.text),
                    });
                }
                let Some(verse) = tui.choose("Choose starting verse", &query, &labels, 0, 0)?
                else {
                    stage = Stage::Chapter;
                    continue;
                };
                at.verse = if verse == 0 || verses[verse - 1].number == 0 {
                    None
                } else {
                    Some(verses[verse - 1].number)
                };
                return Ok(Some(at));
            }
        }
    }
    Ok(None)
}

/// "Matthew–John + Acts–Revelation": what a plan reads, one entry per stream.
pub fn coverage(plan: &Plan) -> String {
    let mut out = String::new();
    for (index, stream) in plan.config().streams.iter().enumerate() {
        if index > 0 {
            out.push_str(" + ");
        }
        let books = &stream.books;
        if books.is_empty() {
            continue;
        }
        if stream.library.is_some() || books.len() == 1 {
            out.push_str(&books[0].name);
        } else {
            out.push_str(&format!(
                "{}–{}",
                books[0].name,
                books[books.len() - 1].name
            ));
        }
    }
    out
}

/// True when the plan reads any Bible book (a stream without a library file).
pub fn reads_bible(plan: &Plan) -> bool {
    plan.config()
        .streams
        .iter()
        .any(|stream| stream.library.is_none())
}

/// The Library: built-in titles, plans, ingested books, search and folder setup.
pub fn run(
    tui: &mut Tui,
    plans: &[Entry],
    selected_plan: usize,
    last_free: Location,
    notice: &str,
    books: &[Book],
) -> io::Result<Option<Choice>> {
    let titles = library::titles();
    let visible = visible_titles();
    let mut labels: Vec<String> = Vec::new();
    // What the `/` filter searches: title and author only, never the descriptions.
    let mut haystack: Vec<String> = Vec::new();
    let mut selected_title = 0;
    for (row, &index) in visible.iter().enumerate() {
        let title = &titles[index];
        if index == last_free.title {
            selected_title = row;
        }
        labels.push(format!("{} · {}", title.name, title.description));
        haystack.push(title.name.to_string());
    }
    haystack.push("Reading plans".into());
    labels.push(format!(
        "Reading plans · {} {} · daily assignments, saved progress, build or remove plans",
        plans.len(),
        if plans.len() == 1 { "plan" } else { "plans" }
    ));
    for book in books {
        haystack.push(format!("{} {}", book.title, book.author));
        let author = if book.author.is_empty() {
            String::new()
        } else {
            format!(" · {}", book.author)
        };
        labels.push(format!(
            "{}{author} · {} chapters",
            book.title, book.chapters
        ));
    }
    if !books.is_empty() {
        haystack.push("Search all books".into());
        labels.push("Search all books · find a word or phrase across every converted book".into());
    }
    haystack.push("Ingest folders".into());
    labels.push(
        "Ingest folders · choose where EPUBs are read from and converted books are kept".into(),
    );
    while !tui.is_interrupted() {
        let options = ChooseOptions {
            haystack: Some(&haystack),
            removable_rows: 0,
        };
        let Some(picked) =
            tui.choose_opts("Library", notice, &labels, selected_title, 1, &options)?
        else {
            return Ok(None);
        };
        let title_row = match picked {
            crate::tui::Choice::Pick(row) | crate::tui::Choice::Remove(row) => row,
        };
        let plans_row = visible.len();
        let books_end = plans_row + 1 + books.len();
        if title_row == plans_row {
            return Ok(Some(Choice::Plans));
        }
        if title_row < books_end && title_row > plans_row {
            return Ok(Some(Choice::Book(title_row - plans_row - 1)));
        }
        if !books.is_empty() && title_row == books_end {
            return Ok(Some(Choice::SearchBooks));
        }
        if title_row > books_end || (books.is_empty() && title_row == books_end) {
            return Ok(Some(Choice::Folders));
        }
        let title_index = visible[title_row];
        // A title offers free reading and its own plans; all plans (any book) are also under
        // Library → Reading plans.
        let title = &titles[title_index];
        while !tui.is_interrupted() {
            let mut choices = vec!["Free reading · choose any book/chapter/verse".to_string()];
            let mut rows: Vec<usize> = Vec::new();
            if title.supports_plans {
                for (index, entry) in plans.iter().enumerate() {
                    if !reads_bible(&entry.plan) {
                        continue;
                    }
                    rows.push(index);
                    choices.push(format!(
                        "Plan · {} · {} days · {}{}",
                        entry.plan.name(),
                        entry.plan.total_days(),
                        coverage(&entry.plan),
                        if index == selected_plan {
                            " · selected"
                        } else {
                            ""
                        }
                    ));
                }
                choices.push(
                    "＋ Build a new plan · choose books, length, and parallel streams".into(),
                );
            }
            let Some(mode) = tui.choose(
                title.name,
                "Read freely, follow a plan, or build one. Plans for your other books are under Library → Reading plans.",
                &choices,
                0,
                1,
            )?
            else {
                break;
            };
            if mode == 0 {
                if let Some(at) = choose_place(tui, menu_location(last_free, title_index))? {
                    return Ok(Some(Choice::Free(at)));
                }
            } else if mode == choices.len() - 1 {
                return Ok(Some(Choice::BuildPlan));
            } else {
                return Ok(Some(Choice::Plan(rows[mode - 1])));
            }
        }
    }
    Ok(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_library_excludes_the_duplicated_legacy_new_testament() {
        let visible = visible_titles();
        assert_eq!(visible.len(), 1);
        assert_eq!(library::titles()[visible[0]].id, "bible");
    }

    #[test]
    fn legacy_new_testament_location_opens_the_same_book_in_bible() {
        let at = menu_location(
            Location {
                title: 0,
                section: 3,
                chapter: 20,
                verse: None,
            },
            visible_titles()[0],
        );
        assert_eq!(library::reference(at).unwrap(), "John:20");
    }
}
