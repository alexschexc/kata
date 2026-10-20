//! In-app reading plan builder UI, ported from `src/plan_builder.zig`. Plans are saved as JSON in
//! `<config>/kata/plans/`. The rules (rates, rendering, slugs) live in `kata_core::plan_builder`.

use crate::tui::Tui;
use kata_core::plan::{Book, Plan, Stream};
use kata_core::plan_builder::{
    BuildError, Draft, LibraryOption, PRESETS, book_of, catalog_index, chapters_of, default_unit,
    plannable, render, slug, total,
};
use kata_core::source::CATALOG;
use std::io;
use std::path::Path;

/// Adds Bible books to `books` from presets or the catalog until Esc.
fn add_books(tui: &mut Tui, books: &mut Vec<Book>) -> io::Result<()> {
    let offered: Vec<usize> = CATALOG
        .iter()
        .enumerate()
        .filter(|(_, entry)| plannable(entry))
        .map(|(i, _)| i)
        .collect();
    let mut selected = 0;
    while !tui.is_interrupted() {
        let mut labels: Vec<String> = PRESETS.iter().map(|p| format!("＋ {}", p.label)).collect();
        for &i in &offered {
            let entry = &CATALOG[i];
            let added = books.iter().any(|b| b.name == entry.name);
            labels.push(format!(
                "{}{} · {} chapters",
                if added { "✓ " } else { "  " },
                entry.name,
                chapters_of(entry)
            ));
        }
        let draft = Draft {
            books: books.clone(),
            ..Draft::default()
        };
        let description = format!(
            "Books are read in the order added. This stream: {}. Enter adds; Esc returns.",
            draft.describe(&[])
        );
        let Some(pick) = tui.choose("Add Bible books", &description, &labels, selected, 1)? else {
            return Ok(());
        };
        selected = pick;
        if pick < PRESETS.len() {
            let preset = &PRESETS[pick];
            let (first, last) = (catalog_index(preset.first), catalog_index(preset.last));
            for entry in &CATALOG[first..=last] {
                if plannable(entry) {
                    books.push(book_of(entry));
                }
            }
        } else {
            books.push(book_of(&CATALOG[offered[pick - PRESETS.len()]]));
        }
    }
    Ok(())
}

/// Picks a library book for a stream; None when cancelled.
fn pick_library(tui: &mut Tui, options: &[LibraryOption]) -> io::Result<Option<usize>> {
    let labels: Vec<String> = options
        .iter()
        .map(|o| {
            format!(
                "{} · {} {} · {} {}",
                o.title,
                o.chapters.len(),
                o.chapter_name,
                o.sections.len(),
                o.section_name
            )
        })
        .collect();
    tui.choose("Choose library book", "Front matter (title pages, contents, prefaces, introductions, indexes) is left out of plans.", &labels, 0, 1)
}

/// Edits one stream. Returns false when the stream should be deleted.
fn edit_stream(
    tui: &mut Tui,
    draft: &mut Draft,
    number: usize,
    options: &[LibraryOption],
) -> io::Result<bool> {
    if draft.is_empty() {
        add_books(tui, &mut draft.books)?;
    }
    let title = format!("Stream {number}");
    while !tui.is_interrupted() {
        if let Some(i) = draft.library {
            let option = &options[i];
            let sections = draft.unit == "section";
            let other = if sections { "chapter" } else { "section" };
            let mut labels = vec![
                format!(
                    "Read by {} ({}) · Enter switches to {} ({})",
                    if sections {
                        &option.section_name
                    } else {
                        &option.chapter_name
                    },
                    option.labels(&draft.unit).len(),
                    if sections {
                        &option.chapter_name
                    } else {
                        &option.section_name
                    },
                    option.labels(other).len()
                ),
                "✕ Delete this stream".to_string(),
            ];
            labels.extend(
                option
                    .labels(&draft.unit)
                    .iter()
                    .map(|label| format!("  {label}")),
            );
            let Some(choice) = tui.choose(&title, &draft.describe(options), &labels, 0, 1)? else {
                return Ok(true);
            };
            match choice {
                0 => draft.unit = other.to_string(),
                1 => return Ok(false),
                _ => {}
            }
            continue;
        }
        let mut labels = vec![
            "＋ Add books…".to_string(),
            "− Remove last book".to_string(),
            "✕ Delete this stream".to_string(),
        ];
        labels.extend(
            draft
                .books
                .iter()
                .map(|b| format!("  {} · {} chapters", b.name, b.chapters)),
        );
        let Some(choice) = tui.choose(&title, &draft.describe(options), &labels, 0, 1)? else {
            return Ok(!draft.is_empty());
        };
        match choice {
            0 => add_books(tui, &mut draft.books)?,
            1 => {
                draft.books.pop();
            }
            2 => return Ok(false),
            _ => {}
        }
    }
    Ok(!draft.is_empty())
}

/// Runs the builder; returns the saved plan file path, or None if cancelled. `options` are the
/// library books offered; `preset` (an option's file) starts the plan with that book as stream 1.
pub fn run(
    tui: &mut Tui,
    plans_dir: &Path,
    options: &[LibraryOption],
    preset: Option<&str>,
) -> io::Result<Option<String>> {
    let mut drafts: Vec<Draft> = Vec::new();
    let mut default_name = String::new();
    if let Some(file) = preset
        && let Some((i, option)) = options.iter().enumerate().find(|(_, o)| o.file == file)
    {
        drafts.push(Draft {
            library: Some(i),
            unit: default_unit(option).to_string(),
            ..Draft::default()
        });
        default_name = option.title.clone();
    }
    let Some(mut name) = tui.prompt(
        "New reading plan",
        "Name the plan (shown in the plan list).",
        &default_name,
    )?
    else {
        return Ok(None);
    };
    let mut repeat = preset.is_none();
    let mut days: Option<u16> = None;
    if drafts.is_empty() {
        drafts.push(Draft::default());
        if !edit_stream(tui, &mut drafts[0], 1, options)? {
            drafts.pop();
        }
    }
    let mut note = String::new();
    const FIXED_ITEMS: usize = 6;
    while !tui.is_interrupted() {
        let streams: Vec<Stream> = drafts.iter().map(|d| d.stream(options)).collect();
        let longest = streams.iter().map(|s| total(&s.books)).max().unwrap_or(0);
        let length: u16 = days.unwrap_or(longest.min(usize::from(u16::MAX)) as u16);
        let mut labels = vec![
            "✓ Save plan".to_string(),
            format!("Name: {name}"),
            format!(
                "Length: {length} days{}",
                if days.is_none() {
                    " (one unit a day for the longest stream)"
                } else {
                    ""
                }
            ),
            format!(
                "Repeat when finished: {}",
                if repeat { "yes" } else { "no" }
            ),
            "＋ Add a Bible stream (read alongside the others)".to_string(),
            if options.is_empty() {
                "＋ Add a library book stream (no converted books yet)"
            } else {
                "＋ Add a library book stream"
            }
            .to_string(),
        ];
        for (i, (draft, stream)) in drafts.iter().zip(&streams).enumerate() {
            let per_day = if length > 0 {
                total(&stream.books) as f64 / f64::from(length)
            } else {
                0.0
            };
            labels.push(format!(
                "Stream {}: {} · ~{per_day:.1}/day",
                i + 1,
                draft.describe(options)
            ));
        }
        let mut preview = String::from("Add a stream to begin.");
        if !streams.is_empty() && length > 0 {
            preview = match render(&name, repeat, &streams, length) {
                Ok(json) => {
                    let plan = Plan::new(json.as_bytes()).map_err(io::Error::other)?;
                    let first = plan.assignments(0).map_err(io::Error::other)?;
                    let parts: Vec<String> = first
                        .iter()
                        .map(|c| match drafts[c.stream].library {
                            Some(o) => options[o].labels(&drafts[c.stream].unit)
                                [usize::from(c.chapter) - 1]
                                .clone(),
                            None => format!("{} {}", c.book, c.chapter),
                        })
                        .collect();
                    format!("Day 1: {}", parts.join(", "))
                }
                Err(BuildError::TooManyDays) => {
                    format!("Too many days: the longest stream has only {longest} units.")
                }
                Err(other) => other.to_string(),
            };
        }
        let description = format!("{note}{preview}");
        note.clear();
        let Some(choice) = tui.choose("Build reading plan", &description, &labels, 0, 1)? else {
            return Ok(None);
        };
        match choice {
            0 => {
                if streams.is_empty() {
                    note = "Add at least one stream. ".into();
                    continue;
                }
                let Ok(json) = render(&name, repeat, &streams, length) else {
                    note = "Fix the length first. ".into();
                    continue;
                };
                std::fs::create_dir_all(plans_dir)?;
                let stem = slug(&name);
                let mut path = plans_dir.join(format!("{stem}.json"));
                let mut n = 2;
                while path.exists() {
                    path = plans_dir.join(format!("{stem}-{n}.json"));
                    n += 1;
                }
                std::fs::write(&path, json)?;
                return Ok(Some(path.to_string_lossy().into_owned()));
            }
            1 => {
                if let Some(renamed) = tui.prompt("Plan name", "Name the plan.", &name)? {
                    name = renamed;
                }
            }
            2 => {
                let description = format!(
                    "Number of days (1–{longest}). Every stream finishes on the last day. 0 restores the default."
                );
                let Some(raw) = tui.prompt("Plan length", &description, "")? else {
                    continue;
                };
                match raw.parse::<u16>() {
                    Ok(0) => days = None,
                    Ok(value) => days = Some(value),
                    Err(_) => note = "Enter a whole number of days. ".into(),
                }
            }
            3 => repeat = !repeat,
            4 => {
                drafts.push(Draft::default());
                let at = drafts.len() - 1;
                if !edit_stream(tui, &mut drafts[at], at + 1, options)? {
                    drafts.pop();
                }
            }
            5 => {
                if options.is_empty() {
                    note = "Convert a book first (put an EPUB in the ingest folder). ".into();
                    continue;
                }
                let Some(picked) = pick_library(tui, options)? else {
                    continue;
                };
                drafts.push(Draft {
                    library: Some(picked),
                    unit: default_unit(&options[picked]).to_string(),
                    ..Draft::default()
                });
            }
            other => {
                let index = other - FIXED_ITEMS;
                if !edit_stream(tui, &mut drafts[index], index + 1, options)? {
                    drafts.remove(index);
                }
            }
        }
    }
    Ok(None)
}
