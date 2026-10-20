//! Command-line entry points that need no terminal: `--dump`, `--check-plan`, `--complete`,
//! `--start-day`, `--licenses`, `--help`. Ported from `src/cli.zig`; output text is identical.
//! The interactive reader (no flags) arrives with the TUI port.

use kata_core::catalog;
use kata_core::layout;
use kata_core::paths::{self, Kind};
use kata_core::plan::Plan;
use kata_core::source::{self, Reference};
use kata_core::state::{self, State};
use kata_core::wyhash;
use std::error::Error;
use std::io::Write;
use std::path::Path;

pub type CliResult = Result<(), Box<dyn Error>>;

pub(crate) const OPTINA: &str = include_str!("../../../../config/optina.json");
pub(crate) const GOSPELS: &str = include_str!("../../../../config/gospels.json");
const NOTICES: &str = include_str!("../../../../src/third_party_notices.txt");
const PROVENANCE: &str = include_str!("../../../../src/data/provenance.json");

const HELP: &str = "Kata · parallel terminal reader\n  kata                         library → title → free reading or plans\n  kata --passage 'Genesis:1'   ad-hoc passage from any available book\n  kata ingest [--force]        convert new/changed EPUBs and index the library\n    --ingest-dir P --library-dir P   use these folders for this run only\n    --save                           with both flags: remember them\n  --plan PATH                  custom JSON reading plan\n  --state PATH                 isolated persistent state\n  --dump                       plain-text output, no state writes\n  --licenses                   bundled text credits and license notices\n  --detect-graphics            report the terminal's inline image support\n  --check-plan                 print complete cycle, no state writes\n  --complete                   explicitly mark today complete and exit\n  --start-day N                preview starting at plan day N (1-based)\n  --start-day N --confirm      save preceding days as complete; N pending\nKeys: j/k or ↑/↓, Ctrl-d/u, g/G; h/l, ←/→ or Tab focus; s linked scroll;\n      m library; o choose free-reading place; H/L or Shift+←/→ previous/next chapter;\n      p choose plan; d choose day; 1/2/3 sources; c then y complete; q quit.\n      / search focused translation; r results; n/N next/previous; x close search.\nVerse-label alignment only: numbering variants need explicit mapping.\n";

#[derive(Debug)]
struct CliError(&'static str);

impl std::fmt::Display for CliError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Error for CliError {}

fn fail<T>(name: &'static str) -> Result<T, Box<dyn Error>> {
    Err(Box::new(CliError(name)))
}

fn write_out(text: &str) -> CliResult {
    let mut out = std::io::stdout().lock();
    out.write_all(text.as_bytes())?;
    out.flush()?;
    Ok(())
}

fn parse_plan_day(raw: &str) -> Result<usize, Box<dyn Error>> {
    if raw.is_empty() || !raw.bytes().all(|c| c.is_ascii_digit()) {
        return fail("InvalidPlanDay");
    }
    match raw.parse::<usize>() {
        Ok(day) if day > 0 => Ok(day),
        _ => fail("InvalidPlanDay"),
    }
}

pub(crate) fn env_var(name: &str) -> Option<String> {
    std::env::var_os(name).map(|v| v.to_string_lossy().into_owned())
}

fn ingest_command(args: &[String]) -> CliResult {
    use kata_core::ingest::{self, Folders};
    let (mut force, mut save) = (false, false);
    let (mut ingest_dir, mut library_dir): (Option<String>, Option<String>) = (None, None);
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--force" => force = true,
            "--save" => save = true,
            "--ingest-dir" | "--library-dir" => {
                i += 1;
                let Some(value) = args.get(i) else {
                    return fail("MissingArgument");
                };
                if args[i - 1] == "--ingest-dir" {
                    ingest_dir = Some(value.clone())
                } else {
                    library_dir = Some(value.clone())
                }
            }
            _ => return fail("UnknownArgument"),
        }
        i += 1;
    }
    let config_home = paths::root(env_var, cfg!(windows), Kind::Config)?;
    let saved = ingest::load_folders(Path::new(&config_home))?;
    if save && (ingest_dir.is_none() || library_dir.is_none()) {
        return fail("SaveNeedsBothFolders");
    }
    let home = env_var("HOME");
    if let Some(d) = &ingest_dir {
        ingest_dir = Some(ingest::normalize_folder(d, home.as_deref())?);
    }
    if let Some(d) = &library_dir {
        library_dir = Some(ingest::normalize_folder(d, home.as_deref())?);
    }
    if ingest_dir.is_none() != library_dir.is_none() && saved.is_none() {
        return fail("BothFoldersRequired");
    }
    let folders = Folders {
        version: 1,
        ingest: match (ingest_dir, &saved) {
            (Some(d), _) => d,
            (None, Some(f)) => f.ingest.clone(),
            (None, None) => {
                write_out(
                    "kata: no ingest folders chosen yet. Launch kata to choose them, or pass\n  kata ingest --ingest-dir PATH --library-dir PATH\n",
                )?;
                return fail("FoldersNotChosen");
            }
        },
        library: library_dir.unwrap_or_else(|| {
            saved
                .as_ref()
                .map(|f| f.library.clone())
                .unwrap_or_default()
        }),
    };
    // Flags are one-shot unless --save is given: a verification run must never overwrite the
    // folders the user chose.
    if save {
        ingest::save_folders(Path::new(&config_home), &folders)?;
    }
    write_out(&format!(
        "Ingest: {}\nLibrary: {}\n",
        folders.ingest, folders.library
    ))?;
    let summary = ingest::run(&folders, force)?;
    for report in &summary.reports {
        let sep = if report.detail.is_empty() { "" } else { " · " };
        write_out(&format!(
            "  {:<9} {}{sep}{}\n",
            report.outcome.name(),
            report.name,
            report.detail
        ))?;
    }
    write_out(&format!(
        "{} converted, {} unchanged, {} failed, {} orphaned; {} documents indexed{}.\n",
        summary.converted,
        summary.unchanged,
        summary.failed,
        summary.orphaned,
        summary.indexed,
        if summary.index_rebuilt {
            " (index rebuilt)"
        } else {
            " (index current)"
        }
    ))?;
    if summary.failed > 0 {
        return fail("IngestFailures");
    }
    Ok(())
}

pub fn run(args: &[String]) -> CliResult {
    let mut passage: Option<&str> = None;
    let mut plan_path: Option<&str> = None;
    let mut state_path: Option<&str> = None;
    let (mut dump, mut check_plan, mut complete, mut confirm) = (false, false, false, false);
    let mut start_day: Option<usize> = None;
    if args.len() >= 2 && args[1] == "ingest" {
        return ingest_command(&args[2..]);
    }
    let mut i = 1;
    while i < args.len() {
        let arg = args[i].as_str();
        match arg {
            "--help" => return write_out(HELP),
            "--detect-graphics" => {
                let mut tui = kata_term::tui::Tui::new()?;
                let caps = tui.detect_graphics(400);
                drop(tui);
                return write_out(&format!(
                    "Inline images: {}; cell {}x{} px\n",
                    caps.describe(),
                    caps.cell_width,
                    caps.cell_height
                ));
            }
            "--licenses" => {
                return write_out(&format!(
                    "{NOTICES}\nBundled dataset provenance (build-time extraction):\n{PROVENANCE}"
                ));
            }
            "--passage" | "--plan" | "--state" | "--start-day" => {
                i += 1;
                let Some(value) = args.get(i) else {
                    return fail("MissingArgument");
                };
                match arg {
                    "--passage" => passage = Some(value),
                    "--plan" => plan_path = Some(value),
                    "--state" => state_path = Some(value),
                    _ => start_day = Some(parse_plan_day(value)?),
                }
            }
            "--confirm" => confirm = true,
            "--dump" => dump = true,
            "--check-plan" => check_plan = true,
            "--complete" => complete = true,
            _ => return fail("UnknownArgument"),
        }
        i += 1;
    }
    if complete && (passage.is_some() || dump || check_plan) {
        return fail("ConflictingArguments");
    }
    if confirm && start_day.is_none() {
        return fail("ConflictingArguments");
    }
    if start_day.is_some() && (passage.is_some() || complete || dump || check_plan) {
        return fail("ConflictingArguments");
    }
    let base = match state_path {
        Some(path) => path.to_string(),
        None => format!(
            "{}/kata/state.json",
            paths::root(env_var, cfg!(windows), Kind::State)?
        ),
    };
    if !dump && !check_plan && !complete && start_day.is_none() {
        return crate::app::run(&base, plan_path, passage);
    }

    let remembered = if plan_path.is_none() {
        catalog::load_selected(&base)?
    } else {
        None
    };
    let plan_bytes: Vec<u8> = if let Some(path) = plan_path {
        read_plan_file(path)?
    } else if let Some(id) = &remembered {
        match id.as_str() {
            "builtin:optina" => OPTINA.as_bytes().to_vec(),
            "builtin:gospels" => GOSPELS.as_bytes().to_vec(),
            other => read_plan_file(other)?,
        }
    } else {
        OPTINA.as_bytes().to_vec()
    };
    let plan = Plan::new(&plan_bytes)?;
    if plan.name().bytes().any(|c| c < 32 || c == 127) {
        return fail("InvalidPlanName");
    }
    if check_plan {
        let mut text = format!("{}: {} assignments\n", plan.name(), plan.total_days());
        for day in 0..plan.total_days() {
            text.push_str(&format!("{}: ", day + 1));
            for chapter in plan.assignments(day)? {
                text.push_str(&format!("{}:{}  ", chapter.book, chapter.chapter));
            }
            text.push('\n');
        }
        return write_out(&text);
    }
    let plan_hash = wyhash::hash(0, &plan_bytes);
    let base_state = state::load(Path::new(&base))?;
    let path = if remembered.is_some() {
        catalog::progress_path(&base, base_state.plan_hash, plan_hash)
    } else {
        base.clone()
    };
    let mut state = state::load(Path::new(&path))?;
    let date = crate::clock::today().ok_or(CliError("ClockUnavailable"))?;
    if passage.is_none() {
        if state.plan_hash != 0 && state.plan_hash != plan_hash {
            return fail("PlanChangedUseSeparateState");
        }
        state.plan_hash = plan_hash;
    }
    if let Some(day) = start_day {
        return start_day_command(&plan, state, &path, day, confirm, date);
    }
    if complete {
        plan.assignments(state.assignment(date))?;
        state.complete(date)?;
        state::save(Path::new(&path), &state)?;
        return write_out(&format!(
            "Completed today. Next assignment tomorrow. State: {path}\n"
        ));
    }
    dump_passage(&plan, plan_hash, state, passage, date)
}

fn read_plan_file(path: &str) -> Result<Vec<u8>, Box<dyn Error>> {
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(
        &mut std::io::Read::take(std::fs::File::open(path)?, 1024 * 1024 + 1),
        &mut bytes,
    )?;
    if bytes.len() > 1024 * 1024 {
        return fail("StreamTooLong");
    }
    Ok(bytes)
}

fn start_day_command(
    plan: &Plan,
    state: State,
    path: &str,
    day: usize,
    confirm: bool,
    date: i32,
) -> CliResult {
    let mut changed = state;
    // A nonrepeating plan has only one cycle, even after its final day.
    if !plan.config().repeat {
        changed.next_day = 0;
        changed.completed_on = 0;
    }
    changed.start_at_day(day, plan.total_days(), date)?;
    let assignments = plan.assignments(changed.next_day)?;
    let mut text = format!(
        "{}: start day {}/{}, cycle {}\nReading: ",
        plan.name(),
        day,
        plan.total_days(),
        changed.next_day / plan.total_days() + 1
    );
    for chapter in &assignments {
        text.push_str(&format!("{}:{}  ", chapter.book, chapter.chapter));
    }
    text.push_str(&format!(
        "\n{} preceding days in this cycle count as completed elsewhere.\nThe selected day remains pending and can be read and completed today.\nThis replaces progress within this cycle, resets reading positions, and clears today's completion marker.\nState: {}\n",
        day - 1,
        path
    ));
    write_out(&text)?;
    if !confirm {
        return write_out(
            "Preview only: no state was changed. Repeat this command with --confirm to save.\n",
        );
    }
    state::save(Path::new(path), &changed)?;
    write_out("Saved. Launch Kata normally to read the selected assignment.\n")
}

fn dump_passage(
    plan: &Plan,
    plan_hash: u64,
    mut state: State,
    passage: Option<&str>,
    date: i32,
) -> CliResult {
    let mut references: Vec<Reference> = Vec::new();
    let title: String;
    let scope_hash: u64;
    if let Some(raw) = passage {
        let reference = Reference::new(raw)?;
        title = reference.query.clone();
        scope_hash = wyhash::hash(0, title.as_bytes());
        references.push(reference);
    } else {
        let ordinal = state.assignment(date);
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
        scope_hash = wyhash::hash(plan_hash, &(ordinal as u64).to_le_bytes());
    }
    if state.scope_hash != scope_hash {
        state.positions = Default::default();
        state.scope_hash = scope_hash;
    }
    let mut rows: Vec<layout::Row> = Vec::new();
    for reference in &references {
        rows.extend(layout::align_verses(&source::streams(reference)?)?);
    }
    if rows.is_empty() {
        return fail("NoVerses");
    }
    let mut text = format!(
        "{title}\nNOTE: aligned by verse labels; KJV Psalms follow Greek/Latin psalm numbering with KJV verse labels; other numbering variants are not inferred.\n"
    );
    for row in &rows {
        text.push_str(&format!("\n{} {}:{}\n", row.book, row.chapter, row.number));
        for (pane, tool) in source::TOOLS.iter().enumerate() {
            if !state.enabled[pane] {
                continue;
            }
            let body = row.texts[pane]
                .as_deref()
                .unwrap_or("[not present under this verse label]");
            // A source label that differs from the row position (KJV Psalms) is printed as
            // `tool (chapter:verse): text`.
            match row.labels[pane] {
                Some(own) => text.push_str(&format!(
                    "{tool} ({}:{}): {body}\n",
                    own.chapter, own.number
                )),
                None => text.push_str(&format!("{tool}: {body}\n")),
            }
        }
    }
    write_out(&text)
}
