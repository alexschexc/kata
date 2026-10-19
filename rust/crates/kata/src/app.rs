//! The interactive application loop, ported from `src/app.zig`: library menu, plans, free
//! reading, chapter navigation, folder setup and the ingest refresh. Features that need the
//! ingested-book reader or the plan builder report a notice until those modules are ported.

use crate::cli::{self, GOSPELS, OPTINA};
use crate::clock;
use kata_core::catalog::{self, Catalog, Entry};
use kata_core::ingest::{self, Folders, IndexEntry};
use kata_core::layout::{self, Row};
use kata_core::library::{self, Location};
use kata_core::paths::{self, Kind};
use kata_core::picker::Action;
use kata_core::reading;
use kata_core::search::SearchState;
use kata_core::source::{self, Reference};
use kata_core::state::{self, State};
use kata_core::wyhash;
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

const NO_BOOK_READER: &str =
    "Reading ingested books is not yet available in the Rust build (book reader port, KAT-29).";

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
    let _graphics = if cli::env_var("KATA_IMAGES").as_deref() == Some("off") {
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
            Choice::Book(_) | Choice::SearchBooks => {
                active = None;
                notice = NO_BOOK_READER.into();
                continue;
            }
            Choice::Plans => {
                active = plans_menu(&mut tui, &mut plans, &mut selected, base, &user_plans)?;
                continue;
            }
            Choice::BuildPlan => {
                active = None;
                notice =
                    "Building plans is not yet available in the Rust build (plan builder port)."
                        .into();
                continue;
            }
        }
        let entry_id = plans.entries[selected].id.clone();
        let entry_hash = plans.entries[selected].hash;
        if free.is_none() && has_library(&plans.entries[selected]) {
            active = None;
            notice = NO_BOOK_READER.into();
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
