//! The interactive application loop, ported from `src/app.zig`: library menu, plans, free
//! reading, chapter navigation, folder setup and the ingest refresh. Features that need the
//! ingested-book reader or the plan builder report a notice until those modules are ported.

use crate::cli::{self, GOSPELS, OPTINA};
use crate::clock;
use kata_core::catalog::{self, Catalog, Entry};
use kata_core::document::{self, Document};
use kata_core::graphics::Capabilities;
use kata_core::ingest::{self, Folders, IndexEntry};
use kata_core::layout::{self, Row};
use kata_core::library::{self, Location};
use kata_core::paths::{self, Kind};
use kata_core::picker::Action;
use kata_core::plan_builder::LibraryOption;
use kata_core::reading;
use kata_core::reading_units::{self, Kind as UnitKind};
use kata_core::search::Query;
use kata_core::search::SearchState;
use kata_core::source::{self, Reference};
use kata_core::state::{self, State};
use kata_core::wyhash;
use kata_term::book_reader::{self, Completion, Exit, Portion, Position};
use kata_term::start_menu::{self, Book, Choice};
use kata_term::tui::{self, ChooseOptions, Options, Tui};
use std::error::Error;
use std::io;
use std::path::{Path, PathBuf};

type AppResult = Result<(), Box<dyn Error>>;

const DEFAULT_NOTICE: &str = "Verse-label alignment; numbering variants are not yet mapped.";

fn failure<T>(name: &str) -> Result<T, Box<dyn Error>> {
    Err(Box::from(name.to_string()))
}

/// Ingested library state for this run.
#[derive(Default)]
struct Shelf {
    folders: Folders,
    entries: Vec<IndexEntry>,
    books: Vec<Book>,
    notice: Option<String>,
}

fn shelve(shelf: &mut Shelf, entries: Vec<IndexEntry>) {
    shelf.books = entries
        .iter()
        .map(|entry| Book {
            title: entry.title.clone(),
            author: entry.author.clone(),
            chapters: entry.chapters,
        })
        .collect();
    shelf.entries = entries;
}

fn has_library(entry: &Entry) -> bool {
    entry
        .plan
        .config()
        .streams
        .iter()
        .any(|stream| stream.library.is_some())
}

/// Quick startup check: convert only new/changed EPUBs, then load the index.
fn refresh_shelf(tui: &Tui, shelf: &mut Shelf) {
    if !shelf.folders.configured() {
        return;
    }
    let _ = tui.status("Checking ingest folder for new or changed EPUBs…");
    let library = PathBuf::from(&shelf.folders.library);
    match ingest::run(&shelf.folders, false) {
        Err(error) => {
            shelf.notice = Some(format!(
                "Ingest check failed: {error}. Existing library kept."
            ));
            shelve(shelf, ingest::load_index(&library).unwrap_or_default());
        }
        Ok(summary) => {
            shelve(shelf, ingest::load_index(&library).unwrap_or_default());
            if summary.converted > 0 || summary.failed > 0 {
                shelf.notice = Some(format!(
                    "Ingest: {} converted, {} failed, {} unchanged. Run `kata ingest` in a shell for details.",
                    summary.converted, summary.failed, summary.unchanged
                ));
            }
        }
    }
}

/// Nearest existing directory at or above `path` ("/" at worst).
fn existing_ancestor(path: &str) -> String {
    let mut at = Path::new(path);
    loop {
        if at.is_dir() {
            return at.to_string_lossy().into_owned();
        }
        match at.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => at = parent,
            _ => return "/".to_string(),
        }
    }
}

/// Subfolder names of `path` (hidden ones only when `hidden`), sorted case-insensitively.
fn list_folders(path: &str, hidden: bool) -> io::Result<Vec<String>> {
    let mut names = Vec::new();
    for entry in std::fs::read_dir(path)? {
        let entry = entry?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if entry.file_type()?.is_dir() && (hidden || !name.starts_with('.')) {
            names.push(name);
        }
    }
    names.sort_by_key(|name| name.to_ascii_lowercase());
    Ok(names)
}

/// Menu-driven folder picker starting at `from` (or its nearest existing parent). Returns the
/// chosen absolute path, or None when cancelled.
fn browse_folders(tui: &mut Tui, title: &str, from: &str) -> io::Result<Option<String>> {
    let mut current = existing_ancestor(from);
    let (mut hidden, mut note) = (false, String::new());
    while !tui.is_interrupted() {
        let folders = list_folders(&current, hidden).unwrap_or_else(|error| {
            note = format!("Cannot list this folder ({error}). ");
            Vec::new()
        });
        let mut labels = vec![
            format!("✓ Use this folder: {current}"),
            "↑ Parent folder".to_string(),
            "+ New folder here…".to_string(),
            if hidden {
                "· Hide hidden folders"
            } else {
                "· Show hidden folders"
            }
            .to_string(),
        ];
        labels.extend(folders.iter().map(|name| format!("{name}/")));
        let description = format!(
            "{note}Browsing {current} · {} folders · Enter opens a folder; choose ✓ to use the current one; Esc returns to typing",
            folders.len()
        );
        note.clear();
        let Some(choice) = tui.choose(
            title,
            &description,
            &labels,
            if folders.is_empty() { 0 } else { 4 },
            1,
        )?
        else {
            return Ok(None);
        };
        match choice {
            0 => return Ok(Some(current)),
            1 => {
                if let Some(parent) = Path::new(&current)
                    .parent()
                    .filter(|p| !p.as_os_str().is_empty())
                {
                    current = parent.to_string_lossy().into_owned();
                }
            }
            2 => {
                let Some(name) = tui.prompt(
                    "New folder",
                    &format!("Name of the new folder inside {current}"),
                    "",
                )?
                else {
                    continue;
                };
                if name.contains(['/', '\\']) || name == "." || name == ".." {
                    note = "Folder names cannot contain slashes. ".into();
                    continue;
                }
                let path = Path::new(&current).join(&name);
                if let Err(error) = std::fs::create_dir_all(&path) {
                    note = format!("Cannot create {name}: {error}. ");
                    continue;
                }
                current = path.to_string_lossy().into_owned();
            }
            3 => hidden = !hidden,
            index => {
                current = Path::new(&current)
                    .join(&folders[index - 4])
                    .to_string_lossy()
                    .into_owned()
            }
        }
    }
    Ok(None)
}

/// First-run (or on request) folder choice. Never guesses: the user confirms or edits each
/// absolute path. Returns false when cancelled.
fn choose_folders(
    tui: &mut Tui,
    config_home: &str,
    home: Option<&str>,
    shelf: &mut Shelf,
) -> io::Result<bool> {
    let suggestion = if shelf.folders.configured() {
        shelf.folders.clone()
    } else if let Some(home) = home {
        ingest::default_folders(Path::new(home))
    } else {
        Folders::default()
    };
    let mut folders = Folders::default();
    let mut error_note = String::new();
    for field in 0..2 {
        loop {
            let description = if field == 0 {
                "Where should Kata look for EPUB files to convert? Use an absolute path (~/ is expanded). The folder is created if missing; Kata only reads from it."
            } else {
                "Where should Kata keep converted books? Each EPUB becomes a readable document with the same name here, plus a small index. Use an absolute path."
            };
            let full = format!(
                "{description}{}{error_note}",
                if error_note.is_empty() { "" } else { "  ⚠ " }
            );
            let label = if field == 0 {
                "Ingest folder (kataIngest)"
            } else {
                "Library folder (kataLibrary)"
            };
            let chosen = if field == 0 {
                &folders.ingest
            } else {
                &folders.library
            };
            let initial = if !chosen.is_empty() {
                chosen.clone()
            } else if field == 0 {
                suggestion.ingest.clone()
            } else {
                suggestion.library.clone()
            };
            let Some(mut raw) = tui.prompt_with(label, &full, &initial, true)? else {
                return Ok(false);
            };
            let store = |folders: &mut Folders, value: String| {
                if field == 0 {
                    folders.ingest = value;
                } else {
                    folders.library = value;
                }
            };
            if raw.starts_with(tui::BROWSE_MARKER) {
                let typed_source = if raw.len() > 1 { &raw[1..] } else { "~/" };
                let typed = ingest::normalize_folder(typed_source, home)
                    .unwrap_or_else(|_| home.unwrap_or("/").to_string());
                match browse_folders(tui, label, &typed)? {
                    None => {
                        store(&mut folders, typed);
                        continue;
                    }
                    Some(picked) => raw = picked,
                }
            }
            match ingest::normalize_folder(&raw, home) {
                Err(ingest::IngestError::FolderMustBeAbsolute) => {
                    error_note = "That path is relative; enter an absolute path.".into();
                }
                Err(other) => error_note = other.to_string(),
                Ok(value) => {
                    store(&mut folders, value);
                    error_note.clear();
                    break;
                }
            }
        }
    }
    if folders.ingest == folders.library {
        shelf.notice = Some("Ingest and library folders must differ. Folders unchanged.".into());
        return Ok(false);
    }
    if let Err(error) = ingest::save_folders(Path::new(config_home), &folders) {
        shelf.notice = Some(format!(
            "Cannot create folders: {error}. Folders unchanged."
        ));
        return Ok(false);
    }
    shelf.folders = folders;
    Ok(true)
}

fn load_book(shelf: &Shelf, index: usize) -> Result<Document, Box<dyn Error>> {
    let path = Path::new(&shelf.folders.library).join(&shelf.entries[index].file);
    Ok(document::parse(&std::fs::read(path)?)?)
}

/// Reads library book `index` (at `start`, else the saved position).
#[allow(clippy::too_many_arguments)]
fn open_book(
    tui: &mut Tui,
    shelf: &Shelf,
    index: usize,
    base: &str,
    caps: Capabilities,
    start: Option<Position>,
    portion: Option<&Portion>,
    completion: Option<Completion<'_>>,
) -> (Exit, Option<String>) {
    let entry = &shelf.entries[index];
    let doc = match load_book(shelf, index) {
        Ok(doc) => doc,
        Err(error) => {
            return (
                Exit::Home,
                Some(format!(
                    "Cannot open {}: {error}. Re-run ingest.",
                    entry.title
                )),
            );
        }
    };
    match book_reader::run(
        tui,
        &doc,
        &entry.file,
        base,
        &shelf.folders.library,
        caps,
        start,
        portion,
        completion,
    ) {
        Ok(exit) => (exit, None),
        Err(error) => (
            Exit::Home,
            Some(format!("Cannot read {}: {error}.", entry.title)),
        ),
    }
}

/// Library-wide search: one query over every converted book, results in a menu; a chosen result
/// opens its book there, and returning from the book comes back to the results.
fn search_books(
    tui: &mut Tui,
    shelf: &Shelf,
    base: &str,
    caps: Capabilities,
) -> io::Result<(Exit, Option<String>)> {
    let description = "Word or phrase (word-prefix, case- and accent-insensitive). Enter searches every converted book; Esc cancels.";
    let Some(raw) = tui.prompt("Search all books", description, "")? else {
        return Ok((Exit::Home, None));
    };
    let query = Query::new(&raw);
    if query.is_empty() {
        return Ok((Exit::Home, None));
    }
    let _ = tui.status("Searching every converted book…");
    const LIMIT: usize = 2000;
    let (mut found, mut labels): (Vec<(usize, Position)>, Vec<String>) = (Vec::new(), Vec::new());
    let (mut occurrences, mut books_with_hits) = (0, 0);
    for (index, entry) in shelf.entries.iter().enumerate() {
        let Ok(doc) = load_book(shelf, index) else {
            continue;
        };
        let (hits, count) = book_reader::find(&doc, &query);
        if !hits.is_empty() {
            books_with_hits += 1;
        }
        occurrences += count;
        for hit in hits {
            if found.len() >= LIMIT {
                break;
            }
            let mut begin = hit.match_range.start.saturating_sub(30);
            while begin > 0 && begin < hit.text.len() && !hit.text.is_char_boundary(begin) {
                begin -= 1;
            }
            let mut end = hit.text.len().min(hit.match_range.end + 60);
            while end > begin && !hit.text.is_char_boundary(end) {
                end -= 1;
            }
            let title = &doc.chapters[hit.section].title;
            labels.push(format!(
                "{} · {title} · {}{}",
                entry.title,
                if begin > 0 { "…" } else { "" },
                &hit.text[begin..end]
            ));
            found.push((
                index,
                Position {
                    version: 1,
                    chapter: hit.section,
                    block: usize::from(hit.number) - 1,
                },
            ));
        }
    }
    if found.is_empty() {
        return Ok((
            Exit::Home,
            Some(format!(
                "No matches for “{raw}” in {} books.",
                shelf.entries.len()
            )),
        ));
    }
    let description = format!(
        "“{raw}”: {} passages in {books_with_hits} book{} · {occurrences} matches{} · Enter opens; returning from the book comes back here",
        found.len(),
        if books_with_hits == 1 { "" } else { "s" },
        if found.len() >= LIMIT {
            " (first 2000 shown)"
        } else {
            ""
        }
    );
    let mut selected = 0;
    while !tui.is_interrupted() {
        let Some(pick) = tui.choose("Search results", &description, &labels, selected, 1)? else {
            return Ok((Exit::Home, None));
        };
        selected = pick;
        let (book, position) = found[pick];
        let (exit, _) = open_book(tui, shelf, book, base, caps, Some(position), None, None);
        if exit == Exit::Quit {
            return Ok((exit, None));
        }
    }
    Ok((Exit::Home, None))
}

/// Every converted book with its plan units, for the plan builder.
fn library_options(tui: &Tui, shelf: &Shelf) -> Vec<LibraryOption> {
    let _ = tui.status("Measuring chapters and sections of converted books…");
    let mut out = Vec::new();
    for (index, entry) in shelf.entries.iter().enumerate() {
        let Ok(doc) = load_book(shelf, index) else {
            continue;
        };
        let chapters = reading_units::units(&doc, UnitKind::Chapter);
        let sections = reading_units::units(&doc, UnitKind::Section);
        if chapters.is_empty() {
            continue;
        }
        out.push(LibraryOption {
            file: entry.file.clone(),
            title: entry.title.clone(),
            chapter_name: reading_units::chapter_name(&doc).to_string(),
            section_name: reading_units::section_name(&doc).to_string(),
            chapters: chapters.into_iter().map(|u| u.label).collect(),
            sections: sections.into_iter().map(|u| u.label).collect(),
        });
    }
    out
}

/// Runs the plan builder and selects the saved plan.
fn build_plan(
    tui: &mut Tui,
    shelf: &Shelf,
    plans: &mut Catalog,
    user_plans: &Path,
    preset: Option<&str>,
) -> Result<(Option<usize>, String), Box<dyn Error>> {
    let options = library_options(tui, shelf);
    let saved = match kata_term::plan_builder::run(tui, user_plans, &options, preset) {
        Err(error) => return Ok((None, format!("Plan builder failed: {error}."))),
        Ok(None) => return Ok((None, "Plan builder closed without saving.".into())),
        Ok(Some(path)) => path,
    };
    match plans.add_file(Path::new(&saved)) {
        Ok(index) => Ok((Some(index), format!("Plan saved to {saved} and selected."))),
        Err(error) => Ok((None, format!("Saved {saved} but cannot load it: {error}."))),
    }
}

/// One day's reading, grouped: consecutive Bible chapters form one item; consecutive units of one
/// library stream form one portion.
struct DayItem {
    label: String,
    /// Bible references ("John:3") for a Bible item.
    references: Vec<String>,
    book: Option<usize>,
    start: Position,
    portion: Option<Portion>,
}

/// Reading units of library books, computed once per (book, kind) while a hub is open.
#[derive(Default)]
struct UnitCache(std::collections::HashMap<(usize, bool), Vec<reading_units::Unit>>);

impl UnitCache {
    fn get(
        &mut self,
        shelf: &Shelf,
        index: usize,
        unit: &str,
    ) -> Result<&[reading_units::Unit], Box<dyn Error>> {
        let sections = unit == "section";
        if let std::collections::hash_map::Entry::Vacant(slot) = self.0.entry((index, sections)) {
            let doc = load_book(shelf, index)?;
            slot.insert(reading_units::units(
                &doc,
                if sections {
                    UnitKind::Section
                } else {
                    UnitKind::Chapter
                },
            ));
        }
        Ok(&self.0[&(index, sections)])
    }
}

fn shelf_index(shelf: &Shelf, file: &str) -> Option<usize> {
    shelf.entries.iter().position(|entry| entry.file == file)
}

fn day_items(
    shelf: &Shelf,
    cache: &mut UnitCache,
    entry: &Entry,
    ordinal: usize,
) -> Result<Vec<DayItem>, Box<dyn Error>> {
    let streams = &entry.plan.config().streams;
    let chapters = entry.plan.assignments(ordinal)?;
    let mut items = Vec::new();
    let mut i = 0;
    while i < chapters.len() {
        let stream = &streams[chapters[i].stream];
        let mut j = i;
        while j < chapters.len() && chapters[j].stream == chapters[i].stream {
            j += 1;
        }
        if let Some(file) = &stream.library {
            let missing = |items: &mut Vec<DayItem>| {
                items.push(DayItem {
                    label: format!(
                        "{} · not in the library (convert it again)",
                        stream.books[0].name
                    ),
                    references: Vec::new(),
                    book: None,
                    start: Position::default(),
                    portion: None,
                });
            };
            let Some(index) = shelf_index(shelf, file) else {
                missing(&mut items);
                i = j;
                continue;
            };
            let unit = stream.unit.as_deref().ok_or("InvalidPlan")?;
            let units = cache.get(shelf, index, unit)?;
            if units.is_empty() {
                missing(&mut items);
                i = j;
                continue;
            }
            let pick =
                |chapter: u16| &units[usize::from(chapter).saturating_sub(1).min(units.len() - 1)];
            let (first, last) = (pick(chapters[i].chapter), pick(chapters[j - 1].chapter));
            let label = if i + 1 == j {
                first.label.clone()
            } else {
                format!("{}  →  {}", first.label, last.label)
            };
            items.push(DayItem {
                label: format!("{} · {label}", shelf.entries[index].title),
                references: Vec::new(),
                book: Some(index),
                start: Position {
                    version: 1,
                    chapter: first.chapter,
                    block: first.start,
                },
                portion: Some(Portion {
                    chapter: last.chapter,
                    end: last.end,
                    label,
                }),
            });
        } else {
            let references: Vec<String> = chapters[i..j]
                .iter()
                .map(|c| format!("{}:{}", c.book, c.chapter))
                .collect();
            let names: Vec<String> = chapters[i..j]
                .iter()
                .map(|c| format!("{} {}", c.book, c.chapter))
                .collect();
            items.push(DayItem {
                label: format!("Bible · {}", names.join(", ")),
                references,
                book: None,
                start: Position::default(),
                portion: None,
            });
        }
        i = j;
    }
    Ok(items)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HubExit {
    Home,
    Quit,
}

/// Daily view for plans with library streams: lists today's portions (Bible chapters open in the
/// Bible reader, library portions in the book reader with an end-of-reading marker), marks the
/// day complete, or picks another day. Progress uses the same state files as Bible plans.
fn plan_hub(
    tui: &mut Tui,
    shelf: &Shelf,
    entry: &Entry,
    base: &str,
    base_hash: &mut u64,
    caps: Capabilities,
    finder: &mut SearchState,
) -> Result<HubExit, Box<dyn Error>> {
    let path = catalog::progress_path(base, *base_hash, entry.hash);
    let mut state = state::load(Path::new(&path))?;
    if state.plan_hash != 0 && state.plan_hash != entry.hash {
        return failure("PlanChangedUseSeparateState");
    }
    state.plan_hash = entry.hash;
    state::save(Path::new(&path), &state)?;
    if base == path {
        *base_hash = entry.hash;
    }
    catalog::save_selected(base, &entry.id)?;
    let plan = &entry.plan;
    let days = plan.total_days();
    let mut cache = UnitCache::default();
    let (mut note, mut selected) = (String::new(), 0usize);
    while !tui.is_interrupted() {
        let date = clock::today().ok_or("ClockUnavailable")?;
        let ordinal = state.assignment(date);
        let finished = !plan.config().repeat && ordinal >= days;
        let items = if finished {
            Vec::new()
        } else {
            day_items(shelf, &mut cache, entry, ordinal)?
        };
        let mut labels: Vec<String> = items
            .iter()
            .map(|item| format!("▸ {}", item.label))
            .collect();
        let done_today = state.completed_on >= date;
        labels.push(
            if done_today {
                "✓ Completed today · next day opens tomorrow"
            } else {
                "✓ Mark today's reading complete"
            }
            .into(),
        );
        labels.push("Choose day…".into());
        labels.push("Back to Library".into());
        let heading = if finished {
            "Plan finished. Choose day… to read again.".to_string()
        } else {
            format!(
                "Day {}/{days}{} · Enter opens a reading; finish at the end-of-reading marker, then m.",
                ordinal % days + 1,
                if done_today {
                    " · completed today"
                } else {
                    ""
                }
            )
        };
        let description = format!("{note}{heading}");
        note.clear();
        let Some(choice) = tui.choose(
            plan.name(),
            &description,
            &labels,
            selected.min(labels.len() - 1),
            1,
        )?
        else {
            return Ok(HubExit::Home);
        };
        selected = choice;
        if choice < items.len() {
            let item = &items[choice];
            if let Some(index) = item.book {
                let completion = Completion {
                    state: &mut state,
                    path: &path,
                    today: date,
                };
                let (exit, message) = open_book(
                    tui,
                    shelf,
                    index,
                    base,
                    caps,
                    Some(item.start),
                    item.portion.as_ref(),
                    Some(completion),
                );
                if exit == Exit::Quit {
                    return Ok(HubExit::Quit);
                }
                if let Some(message) = message {
                    note = format!("{message} ");
                }
            } else if !item.references.is_empty() {
                let mut rows = Vec::new();
                for raw in &item.references {
                    rows.extend(layout::align_verses(&source::streams(&Reference::new(
                        raw,
                    )?)?)?);
                }
                let scope = wyhash::hash(entry.hash, &(ordinal as u64).to_le_bytes());
                if state.scope_hash != scope {
                    state.positions = Default::default();
                    state.scope_hash = scope;
                }
                let action = {
                    let mut options = Options::new(
                        &format!("{} · day {}/{days}", plan.name(), ordinal % days + 1),
                        PathBuf::from(&path),
                    );
                    options.plan_mode = false;
                    options.completable = true;
                    options.today = date;
                    options.notice = "Plan reading · m returns to the plan".into();
                    options.search = Some(&mut *finder);
                    tui.read(&rows, &mut state, options)?
                };
                state::save(Path::new(&path), &state)?;
                if action.is_none() {
                    return Ok(HubExit::Quit);
                }
            }
            continue;
        }
        match choice - items.len() {
            0 => {
                if !finished {
                    match state.complete(date) {
                        Ok(()) => {}
                        Err(state::StateError::AlreadyCompletedToday) => {
                            note = "Already completed today. ".into();
                            continue;
                        }
                        Err(error) => return Err(error.into()),
                    }
                    state::save(Path::new(&path), &state)?;
                    note = "Completed and saved. ".into();
                }
            }
            1 => {
                let mut day_labels = Vec::with_capacity(days);
                for day in 0..days {
                    let parts: Vec<String> = day_items(shelf, &mut cache, entry, day)?
                        .into_iter()
                        .map(|item| item.label)
                        .collect();
                    day_labels.push(format!("Day {} · {}", day + 1, parts.join("  |  ")));
                }
                let Some(day) = tui.choose(
                    "Choose plan day",
                    "Earlier days in this cycle count as complete; the chosen day is pending.",
                    &day_labels,
                    ordinal % days,
                    1,
                )?
                else {
                    continue;
                };
                if !plan.config().repeat {
                    state.next_day = 0;
                    state.completed_on = 0;
                }
                state.start_at_day(day + 1, days, date)?;
                state::save(Path::new(&path), &state)?;
                note = format!("Moved to day {}. ", day + 1);
                selected = 0;
            }
            _ => return Ok(HubExit::Home),
        }
    }
    Ok(HubExit::Quit)
}

struct Session {
    title: String,
    rows: Vec<Row>,
}

/// Rows for a plan day or a passage, resetting positions when the scope changed.
fn prepare(
    entry: &Entry,
    state: &mut State,
    date: i32,
    passage: Option<&str>,
) -> Result<Session, Box<dyn Error>> {
    let mut references: Vec<Reference> = Vec::new();
    let (title, scope);
    if let Some(raw) = passage {
        let reference = Reference::new(raw)?;
        title = reference.query.clone();
        scope = wyhash::hash(0, title.as_bytes());
        references.push(reference);
    } else {
        state.plan_hash = entry.hash;
        let ordinal = state.assignment(date);
        let plan = &entry.plan;
        if !plan.config().repeat && ordinal >= plan.total_days() {
            return Ok(Session {
                title: plan.name().to_string(),
                rows: Vec::new(),
            });
        }
        for chapter in plan.assignments(ordinal)? {
            references.push(Reference::new(&format!(
                "{}:{}",
                chapter.book, chapter.chapter
            ))?);
        }
        title = format!(
            "{} · day {}/{}",
            plan.name(),
            ordinal % plan.total_days() + 1,
            plan.total_days()
        );
        scope = wyhash::hash(entry.hash, &(ordinal as u64).to_le_bytes());
    }
    if state.scope_hash != scope {
        state.positions = Default::default();
        state.scope_hash = scope;
    }
    let mut rows = Vec::new();
    for reference in &references {
        rows.extend(layout::align_verses(&source::streams(reference)?)?);
    }
    if rows.is_empty() {
        return failure("NoVerses");
    }
    Ok(Session { title, rows })
}

/// Reading plans list: pick, build or remove. Returns the next mode when a plan is chosen.
fn plans_menu(
    tui: &mut Tui,
    plans: &mut Catalog,
    selected: &mut usize,
    base: &str,
    user_plans: &Path,
) -> Result<Option<Choice>, Box<dyn Error>> {
    let mut note = String::from(
        "Pick a plan to read, build a new one, or press x to remove one you no longer want.",
    );
    while !tui.is_interrupted() {
        let mut labels: Vec<String> = plans
            .entries
            .iter()
            .enumerate()
            .map(|(i, entry)| {
                format!(
                    "{} · {} days · {}{}",
                    entry.plan.name(),
                    entry.plan.total_days(),
                    start_menu::coverage(&entry.plan),
                    if i == *selected { " · selected" } else { "" }
                )
            })
            .collect();
        labels.push("＋ Build a new plan · choose books, length, and parallel streams".into());
        let options = ChooseOptions {
            haystack: None,
            removable_rows: plans.entries.len(),
        };
        let initial = (*selected).min(labels.len() - 1);
        match tui.choose_opts("Reading plans", &note, &labels, initial, 1, &options)? {
            None => break,
            Some(tui::Choice::Pick(index)) => {
                return Ok(Some(if index == plans.entries.len() {
                    Choice::BuildPlan
                } else {
                    Choice::Plan(index)
                }));
            }
            Some(tui::Choice::Remove(index)) => {
                if plans.entries.len() <= 1 {
                    note = "Keep at least one plan; build another before removing this one.".into();
                    continue;
                }
                let id = plans.entries[index].id.clone();
                let name = plans.entries[index].plan.name().to_string();
                let prefix = format!("{}{}", user_plans.display(), std::path::MAIN_SEPARATOR);
                if id.starts_with(&prefix) {
                    // Plans you built or added live in your config folder: delete the file.
                    if let Err(error) = std::fs::remove_file(&id) {
                        note = format!("Could not remove the plan file: {error}.");
                        continue;
                    }
                } else if let Err(error) = catalog::hide(base, &id) {
                    // Built-in and project-folder plans have no file of ours to delete: hide them.
                    note = format!("Could not hide the plan: {error}.");
                    continue;
                }
                note = format!("Removed \"{name}\". Saved progress for it was kept.");
                plans.remove(index);
                if *selected == index {
                    *selected = 0;
                    catalog::save_selected(base, &plans.entries[0].id)?;
                } else if *selected > index {
                    *selected -= 1;
                }
            }
        }
    }
    Ok(None)
}

pub fn run(base: &str, explicit_plan: Option<&str>, initial_passage: Option<&str>) -> AppResult {
    let mut plans = Catalog::new();
    plans.add("builtin:optina", OPTINA.as_bytes())?;
    plans.add("builtin:gospels", GOSPELS.as_bytes())?;
    plans.discover(Path::new("config"))?;
    let config_home = paths::root(cli::env_var, cfg!(windows), Kind::Config)?;
    let user_plans = Path::new(&config_home).join("kata").join("plans");
    plans.discover(&user_plans)?;
    plans.drop_hidden(&catalog::load_hidden(base)?);
    let mut selected = 0;
    let mut notice = DEFAULT_NOTICE.to_string();
    if let Some(path) = explicit_plan {
        selected = plans.add_file(Path::new(path))?;
    } else if let Some(id) = catalog::load_selected(base)? {
        match plans.entries.iter().position(|entry| entry.id == id) {
            Some(index) => selected = index,
            None => {
                selected = plans.add_file(Path::new(&id)).unwrap_or_else(|_| {
                    notice = "Saved plan unavailable. p selects another; its previous progress is preserved.".into();
                    0
                });
            }
        }
    }
    if plans.skipped > 0 {
        notice = format!(
            "{} invalid plan files ignored. p chooses plans. Verse-number variants are unmapped.",
            plans.skipped
        );
    }
    let mut base_hash = state::load(Path::new(base))?.plan_hash;
    if explicit_plan.is_some()
        && initial_passage.is_none()
        && base_hash != 0
        && base_hash != plans.entries[selected].hash
    {
        return failure("PlanChangedUseSeparateState");
    }
    let mut last_free = reading::load_bookmark(base).unwrap_or_default();
    let mut active: Option<Choice> = if let Some(raw) = initial_passage {
        Some(Choice::Free(reading::from_reference(raw)?))
    } else if explicit_plan.is_some() {
        Some(Choice::Plan(selected))
    } else {
        None
    };
    let mut override_query: Option<String> = initial_passage.map(str::to_string);
    let mut finder = SearchState::default();
    let mut build_for: Option<String> = None;
    let mut shelf = Shelf::default();
    let home = cli::env_var("HOME").or_else(|| cli::env_var("USERPROFILE"));
    let folders_saved = match ingest::load_folders(Path::new(&config_home)) {
        Ok(saved) => saved,
        Err(error) => {
            notice = format!(
                "Ingest folder settings unreadable ({error}); choose them again from the Library."
            );
            None
        }
    };
    if let Some(saved) = &folders_saved {
        shelf.folders = saved.clone();
    }
    let mut tui = Tui::new()?;
    // One capability query per launch (bounded); KATA_IMAGES=off disables.
    let graphics = if cli::env_var("KATA_IMAGES").as_deref() == Some("off") {
        Default::default()
    } else {
        tui.detect_graphics(300)
    };
    if active.is_none() {
        if folders_saved.is_none()
            && shelf.notice.is_none()
            && !choose_folders(&mut tui, &config_home, home.as_deref(), &mut shelf)?
            && shelf.notice.is_none()
        {
            shelf.notice = Some(
                "Ingest folders not chosen. Choose them any time from the last Library entry."
                    .into(),
            );
        }
        refresh_shelf(&tui, &mut shelf);
        if let Some(message) = &shelf.notice {
            notice = message.clone();
        }
    }
    while !tui.is_interrupted() {
        if active.is_none() {
            match start_menu::run(
                &mut tui,
                &plans.entries,
                selected,
                last_free,
                &notice,
                &shelf.books,
            ) {
                Err(error) => {
                    notice = format!(
                        "Cannot open requested reading: {error}. Choose another place or mode."
                    );
                    continue;
                }
                Ok(None) => return Ok(()),
                Ok(choice) => active = choice,
            }
            override_query = None;
        }
        let mut free: Option<Location> = None;
        match active.expect("set above") {
            Choice::Plan(index) => selected = index,
            Choice::Free(at) => free = Some(at),
            Choice::Folders => {
                active = None;
                shelf.notice = None;
                if choose_folders(&mut tui, &config_home, home.as_deref(), &mut shelf)? {
                    refresh_shelf(&tui, &mut shelf);
                    notice = shelf.notice.clone().unwrap_or_else(|| {
                        format!("Folders saved. {} books in the library.", shelf.books.len())
                    });
                } else {
                    notice = shelf
                        .notice
                        .clone()
                        .unwrap_or_else(|| "Folder change cancelled.".into());
                }
                continue;
            }
            Choice::Book(index) => {
                active = None;
                // Book menu: read, or a reading plan that includes this book.
                let file = shelf.entries[index].file.clone();
                let mut choices = vec!["Read · resume where you left off".to_string()];
                let mut plan_rows: Vec<usize> = Vec::new();
                for (p, plan_entry) in plans.entries.iter().enumerate() {
                    if plan_entry
                        .plan
                        .config()
                        .streams
                        .iter()
                        .any(|stream| stream.library.as_deref() == Some(file.as_str()))
                    {
                        choices.push(format!(
                            "Reading plan · {} · {} days",
                            plan_entry.plan.name(),
                            plan_entry.plan.total_days()
                        ));
                        plan_rows.push(p);
                    }
                }
                choices.push("＋ Build a reading plan for this book".into());
                let Some(pick) = tui.choose(
                    &shelf.entries[index].title,
                    "Read freely, follow a reading plan, or build one.",
                    &choices,
                    0,
                    1,
                )?
                else {
                    continue;
                };
                if pick == choices.len() - 1 {
                    build_for = Some(file);
                    active = Some(Choice::BuildPlan);
                } else if pick > 0 {
                    active = Some(Choice::Plan(plan_rows[pick - 1]));
                } else {
                    let (exit, note) =
                        open_book(&mut tui, &shelf, index, base, graphics, None, None, None);
                    if exit == Exit::Quit {
                        return Ok(());
                    }
                    notice = note.unwrap_or_else(|| "Library".into());
                }
                continue;
            }
            Choice::SearchBooks => {
                active = None;
                let (exit, note) = search_books(&mut tui, &shelf, base, graphics)?;
                if exit == Exit::Quit {
                    return Ok(());
                }
                notice = note.unwrap_or_else(|| "Library".into());
                continue;
            }
            Choice::Plans => {
                active = plans_menu(&mut tui, &mut plans, &mut selected, base, &user_plans)?;
                continue;
            }
            Choice::BuildPlan => {
                active = None;
                let (saved, message) = build_plan(
                    &mut tui,
                    &shelf,
                    &mut plans,
                    &user_plans,
                    build_for.as_deref(),
                )?;
                if let Some(index) = saved {
                    active = Some(Choice::Plan(index));
                    build_for = None;
                }
                notice = message;
                continue;
            }
        }
        let entry_id = plans.entries[selected].id.clone();
        let entry_hash = plans.entries[selected].hash;
        if free.is_none() && has_library(&plans.entries[selected]) {
            active = None;
            match plan_hub(
                &mut tui,
                &shelf,
                &plans.entries[selected],
                base,
                &mut base_hash,
                graphics,
                &mut finder,
            ) {
                Err(error) => notice = format!("Cannot open plan: {error}."),
                Ok(HubExit::Quit) => return Ok(()),
                Ok(HubExit::Home) => notice = "Library".into(),
            }
            continue;
        }
        let query: Option<String> = match free {
            Some(at) => Some(match &override_query {
                Some(raw) => raw.clone(),
                None => library::reference(at)?,
            }),
            None => None,
        };
        let path = match free {
            Some(at) => reading::path(base, at, override_query.as_deref())?,
            None => catalog::progress_path(base, base_hash, entry_hash),
        };
        let mut state = match state::load(Path::new(&path)) {
            Ok(state) => state,
            Err(error) => {
                notice = format!("Cannot load progress: {error}. Saved data was not changed.");
                active = None;
                continue;
            }
        };
        if free.is_none() && state.plan_hash != 0 && state.plan_hash != entry_hash {
            return failure("PlanChangedUseSeparateState");
        }
        if free.is_some()
            && (state.plan_hash != 0 || state.next_day != 0 || state.completed_on != 0)
        {
            return failure("InvalidFreeState");
        }
        let date = clock::today().ok_or("ClockUnavailable")?;
        state.edition = [0; 3];
        let mut session =
            match prepare(&plans.entries[selected], &mut state, date, query.as_deref()) {
                Ok(session) => session,
                Err(error) => {
                    notice = format!("Cannot load reading: {error}. Progress was not changed.");
                    active = None;
                    continue;
                }
            };
        if let Some(at) = free {
            reading::anchor(&session.rows, &mut state, at)?;
            session.title = format!("{} · free reading", session.title);
            last_free = Location { verse: None, ..at };
            active = Some(Choice::Free(last_free));
        }
        finder.names = ["kjv".into(), "grb".into(), "vul".into()];
        finder.editions = [None, None, None];
        let action = {
            let mut options = Options::new(&session.title, PathBuf::from(&path));
            options.plan_mode = free.is_none();
            options.today = date;
            options.plans = &plans.entries;
            options.active_plan = selected;
            options.plan = Some(&plans.entries[selected].plan);
            options.notice = notice.clone();
            options.search = Some(&mut finder);
            tui.read(&session.rows, &mut state, options)?
        };
        state.edition = [0; 3];
        state::save(Path::new(&path), &state)?;
        if free.is_some() {
            reading::save_bookmark(base, last_free)?;
        } else {
            catalog::save_selected(base, &entry_id)?;
            if base == path {
                base_hash = state.plan_hash;
            }
        }
        let Some(action) = action else { return Ok(()) };
        let mut target = active.expect("set above");
        let mut changed_day: Option<usize> = None;
        match action {
            Action::Home => {
                active = None;
                override_query = None;
                continue;
            }
            Action::OpenPlace => match start_menu::choose_place(&mut tui, last_free) {
                Err(error) => {
                    notice =
                        format!("Cannot choose reading place: {error}. Previous view retained.");
                    continue;
                }
                Ok(None) => continue,
                Ok(Some(at)) => target = Choice::Free(at),
            },
            Action::NextChapter | Action::PreviousChapter => {
                let Some(at) = free else { continue };
                let Some(next) = library::adjacent(at, action == Action::NextChapter) else {
                    notice = "This is the boundary of the selected title. o chooses any place; m opens the library.".into();
                    continue;
                };
                target = Choice::Free(next);
            }
            Action::ChoosePlan(index) => target = Choice::Plan(index),
            Action::OpenLocation(at) => target = Choice::Free(at),
            Action::Reload => continue,
            Action::StartDay(day) => {
                if free.is_some() {
                    continue;
                }
                changed_day = Some(day);
            }
        }
        // Validate the requested content before committing any mode/location change.
        let (target_plan, target_free) = match target {
            Choice::Plan(index) => (index, None),
            Choice::Free(at) => (selected, Some(at)),
            _ => continue,
        };
        let target_entry = &plans.entries[target_plan];
        if target_free.is_none() && changed_day.is_none() && has_library(target_entry) {
            active = Some(target);
            continue;
        }
        let target_path = match target_free {
            Some(at) => reading::path(base, at, None)?,
            None => catalog::progress_path(base, base_hash, target_entry.hash),
        };
        let mut changed = match state::load(Path::new(&target_path)) {
            Ok(state) => state,
            Err(error) => {
                notice =
                    format!("Cannot open requested progress: {error}. Previous view retained.");
                continue;
            }
        };
        if target_free.is_none() && changed.plan_hash != 0 && changed.plan_hash != target_entry.hash
        {
            notice = "Plan identity mismatch. Previous view retained.".into();
            continue;
        }
        if let Some(day) = changed_day {
            if !target_entry.plan.config().repeat {
                changed.next_day = 0;
                changed.completed_on = 0;
            }
            if let Err(error) = changed.start_at_day(day, target_entry.plan.total_days(), date) {
                notice = format!("Cannot change day: {error}");
                continue;
            }
        }
        let target_query = match target_free {
            Some(at) => Some(library::reference(at)?),
            None => None,
        };
        changed.edition = [0; 3];
        let validated = match prepare(target_entry, &mut changed, date, target_query.as_deref()) {
            Ok(validated) => validated,
            Err(error) => {
                notice = format!("Cannot load requested reading: {error}. Previous view retained.");
                continue;
            }
        };
        if let Some(at) = target_free
            && reading::anchor(&validated.rows, &mut changed, at).is_err()
        {
            notice = "Starting verse unavailable. Previous view retained.".into();
            finder.reveal = false;
            continue;
        }
        state::save(Path::new(&target_path), &changed)?;
        let target_id = target_entry.id.clone();
        if let Some(at) = target_free {
            reading::save_bookmark(base, at)?;
        } else {
            catalog::save_selected(base, &target_id)?;
            if base == target_path {
                base_hash = changed.plan_hash;
            }
        }
        active = Some(target);
        override_query = None;
        notice = if matches!(action, Action::OpenLocation(_)) {
            "Search result opened in free reading · n/N next/previous · r results · x closes search"
        } else if changed_day.is_some() {
            "Reading position saved. Preceding days count as complete; selected day is pending."
        } else {
            "Reading selected. Free-reading positions and plan progress are kept separately; verse-number variants are unmapped."
        }
        .into();
    }
    Ok(())
}
