//! Terminal user interface, ported from `src/tui.zig`: the three-pane reader with its search
//! panel, plan/day pickers, the filterable numbered menu, text prompts and status screen.
//! Frames are built into a `String` and written in one call; keys are single bytes from the
//! shared decoder.

use crate::backend::{self, Read, Size};
use crate::input::Decoder;
use crate::{color, theme};
use kata_core::catalog;
use kata_core::graphics::{self, Capabilities};
use kata_core::layout::{self, Line, Row};
use kata_core::picker::{Action, Kind, Picker};
use kata_core::plan::Plan;
use kata_core::search::{self, Hit, Query, SearchState};
use kata_core::state::{self, Position, State, StateError};
use std::fmt::Write as _;
use std::io;
use std::path::PathBuf;

const SESSION_OFF: &str = "\x1b[?2004l\x1b[0m\x1b[?25h\x1b[?1049l";

fn other<E: std::fmt::Display>(error: E) -> io::Error {
    io::Error::other(error.to_string())
}

/// The longest prefix of `text` that fits in `width` terminal cells.
pub fn clipped(text: &str, width: usize) -> &str {
    let mut end = 0;
    for (index, ch) in text.char_indices() {
        let next = index + ch.len_utf8();
        if layout::display_width(&text[..next]) > width {
            break;
        }
        end = next;
    }
    &text[..end]
}

/// `text` clipped and space-padded to exactly `width` cells.
pub fn cell(w: &mut String, text: &str, width: usize) {
    let visible = clipped(text, width);
    w.push_str(visible);
    for _ in layout::display_width(visible)..width {
        w.push(' ');
    }
}

fn slice(text: &str, from: usize, to: usize) -> &str {
    text.get(from..to).unwrap_or("")
}

const MARK_ON: &str = concat!("\x1b[48;2;125;130;217m", "\x1b[38;2;6;11;30m");
const MARK_OFF: &str = concat!("\x1b[48;2;6;11;30m", "\x1b[38;2;255;206;173m");

/// Writes `full[from..to]`, highlighting query matches found from `search_from`, restoring
/// `after_match` after each match.
pub fn write_marked_styled(
    w: &mut String,
    full: &str,
    from: usize,
    to: usize,
    query: &Query,
    search_from: usize,
    after_match: &str,
) {
    let (mut at, mut cursor) = (from, search_from);
    while let Some(range) = search::next(full, query, cursor) {
        if range.start >= to {
            break;
        }
        cursor = range.end;
        if range.end <= at {
            continue;
        }
        let first = range.start.max(at);
        let last = range.end.min(to);
        w.push_str(slice(full, at, first));
        w.push_str(MARK_ON);
        w.push_str(slice(full, first, last));
        w.push_str(after_match);
        at = last;
    }
    w.push_str(slice(full, at, to));
}

fn write_marked(
    w: &mut String,
    full: &str,
    from: usize,
    to: usize,
    query: &Query,
    search_from: usize,
) {
    write_marked_styled(w, full, from, to, query, search_from, MARK_OFF);
}

/// True when every space-separated word of `filter` starts a word in `haystack` (case- and
/// accent-insensitive, same matcher as in-book search).
pub fn matches_filter(haystack: &str, filter: &str) -> bool {
    filter
        .split(' ')
        .filter(|token| !token.is_empty())
        .all(|token| {
            let query = Query::new(token);
            query.is_empty() || search::next(haystack, &query, 0).is_some()
        })
}

/// Reader options. `search` is owned by the application so results survive opening another
/// chapter.
pub struct Options<'a> {
    pub title: String,
    pub plan_mode: bool,
    /// `c` may mark the plan day complete (always true in plan mode; set for plan days opened
    /// from a hub).
    pub completable: bool,
    pub today: i32,
    pub state_path: PathBuf,
    pub plans: &'a [catalog::Entry],
    pub active_plan: usize,
    pub plan: Option<&'a Plan>,
    pub notice: String,
    pub search: Option<&'a mut SearchState>,
    /// Editions available per column (1 = bundled only) and display names.
    pub editions: [u8; 3],
    pub names: [String; 3],
}

impl Options<'_> {
    pub fn new(title: &str, state_path: PathBuf) -> Options<'static> {
        Options {
            title: title.to_string(),
            plan_mode: false,
            completable: false,
            today: 0,
            state_path,
            plans: &[],
            active_plan: 0,
            plan: None,
            notice: "Verse-label alignment; numbering variants are not yet mapped.".into(),
            search: None,
            editions: [1, 1, 1],
            names: ["kjv".into(), "grb".into(), "vul".into()],
        }
    }
}

fn selected_count(enabled: [bool; 3]) -> usize {
    enabled.iter().filter(|&&on| on).count()
}

fn move_focus(state: &mut State, direction: i32) {
    let mut next = state.focus;
    for _ in 0..3 {
        next = if direction > 0 {
            (next + 1) % 3
        } else {
            (next + 2) % 3
        };
        if state.enabled[next] {
            state.focus = next;
            return;
        }
    }
}

fn max_top(lines: &[Line], height: usize) -> usize {
    lines.len().saturating_sub(height)
}

fn shift(tops: &mut [usize; 3], state: &State, delta: i64, maximum: usize) {
    for (pane, top) in tops.iter_mut().enumerate() {
        if !state.linked && pane != state.focus {
            continue;
        }
        *top = if delta < 0 {
            top.saturating_sub(delta.unsigned_abs() as usize)
        } else {
            maximum.min(top.saturating_add(delta as usize))
        };
    }
}

fn restore(lines: &[Line], position: Position) -> usize {
    for (index, line) in lines.iter().enumerate() {
        if line.row >= position.row {
            let mut end = index;
            while end < lines.len() && lines[end].row == line.row {
                end += 1;
            }
            return index + position.line.min(end - index - 1);
        }
    }
    0
}

fn remember(lines: &[Line], tops: [usize; 3], state: &mut State) {
    if lines.is_empty() {
        return;
    }
    for (pane, &top) in tops.iter().enumerate() {
        let at = top.min(lines.len() - 1);
        let mut first = at;
        while first > 0 && lines[first - 1].row == lines[at].row {
            first -= 1;
        }
        state.positions[pane] = Position {
            row: lines[at].row,
            line: at - first,
        };
    }
}

/// Marks today's assignment complete; false when it already was.
fn complete_day(state: &mut State, today: i32) -> Result<bool, StateError> {
    match state.complete(today) {
        Ok(()) => Ok(true),
        Err(StateError::AlreadyCompletedToday) => Ok(false),
        Err(error) => Err(error),
    }
}

fn day_label(plan: &Plan, day: usize) -> String {
    let mut label = format!("Day {} · ", day + 1);
    for chapter in plan.assignments(day).unwrap_or_default() {
        let _ = write!(label, "{}:{}  ", chapter.book, chapter.chapter);
    }
    label
}

fn render_picker(
    w: &mut String,
    picker: &Picker,
    options: &Options,
    columns: usize,
    height: usize,
) {
    if columns < 24 || height < 8 {
        w.push_str("Enlarge terminal. Esc/q cancels picker.");
        return;
    }
    let is_plans = picker.kind == Kind::Plans;
    let heading = match picker.kind {
        Kind::Plans => "Choose plan",
        Kind::ConfirmDay => "Confirm reading position",
        _ => "Choose plan day",
    };
    let _ = write!(w, " K A T A  |  {heading}\x1b[2;1H");
    let Some(plan) = options.plan else {
        // A day picker needs a plan; without one only the plan list can be drawn.
        if !is_plans {
            return;
        }
        return render_plan_rows(w, picker, options, columns, height);
    };
    if !is_plans {
        w.push_str(clipped(plan.name(), columns));
    }
    if picker.kind == Kind::ConfirmDay {
        let label = day_label(plan, picker.selected);
        w.push_str("\x1b[4;1H");
        w.push_str(clipped(&label, columns));
        let _ = write!(
            w,
            "\x1b[6;1H{} preceding days count as completed elsewhere.",
            picker.selected
        );
        w.push_str(
            "\x1b[7;1HSelected day stays pending. Later progress in this cycle is replaced.",
        );
    } else {
        let count = if is_plans {
            options.plans.len()
        } else {
            plan.total_days()
        };
        let capacity = height - 6;
        let first = picker
            .selected
            .saturating_sub(capacity / 2)
            .min(count.saturating_sub(capacity));
        for index in first..count.min(first + capacity) {
            let _ = write!(w, "\x1b[{};1H", index - first + 3);
            w.push_str(if index == picker.selected {
                concat!("\x1b[38;2;125;130;217m", "▶ ")
            } else {
                concat!("\x1b[38;2;255;206;173m", "  ")
            });
            let label = if is_plans {
                plan_label(options, index)
            } else {
                day_label(plan, index)
            };
            w.push_str(clipped(&label, columns - 2));
        }
    }
    render_picker_footer(w, picker, options, columns, height);
}

fn plan_label(options: &Options, index: usize) -> String {
    let entry = &options.plans[index];
    format!(
        "{} · {} days{}",
        entry.plan.name(),
        entry.plan.total_days(),
        if index == options.active_plan {
            " · active"
        } else {
            ""
        }
    )
}

fn render_plan_rows(
    w: &mut String,
    picker: &Picker,
    options: &Options,
    columns: usize,
    height: usize,
) {
    let count = options.plans.len();
    let capacity = height - 6;
    let first = picker
        .selected
        .saturating_sub(capacity / 2)
        .min(count.saturating_sub(capacity));
    for index in first..count.min(first + capacity) {
        let _ = write!(w, "\x1b[{};1H", index - first + 3);
        w.push_str(if index == picker.selected {
            concat!("\x1b[38;2;125;130;217m", "▶ ")
        } else {
            concat!("\x1b[38;2;255;206;173m", "  ")
        });
        w.push_str(clipped(&plan_label(options, index), columns - 2));
    }
    render_picker_footer(w, picker, options, columns, height);
}

fn render_picker_footer(
    w: &mut String,
    picker: &Picker,
    options: &Options,
    columns: usize,
    height: usize,
) {
    let _ = write!(w, "\x1b[{};1H{}", height - 1, theme::ACCENT);
    let help = match picker.kind {
        Kind::ConfirmDay => "y confirms · any other key cancels confirmation · Esc/q returns",
        Kind::Plans => "j/k select · Enter open plan · Esc/q cancel · progress kept per plan",
        _ => "j/k select · type day number · Enter preview · Esc/q cancel",
    };
    w.push_str(clipped(help, columns));
    let _ = write!(w, "\x1b[{height};1H{}", theme::DIM);
    if picker.digit_count > 0 {
        let _ = write!(w, "Go to day: {}", String::from_utf8_lossy(picker.digits()));
    } else {
        w.push_str(clipped(&options.notice, columns));
    }
}

/// Byte offset of wrapped line `k` of `text` within `text`.
fn wrapped_offset(text: &str, width: usize, k: usize) -> Option<(usize, usize)> {
    let lines = layout::wrap(text, width).ok()?;
    let line = lines.get(k)?;
    Some((line.as_ptr() as usize - text.as_ptr() as usize, line.len()))
}

/// A reader cell with search matches highlighted. Cells are slices of the line's full
/// "chapter:verse text", so matches split by wrapping still mark.
fn marked_cell(
    w: &mut String,
    lines: &[Line],
    at: usize,
    pane: usize,
    query: Option<&Query>,
    width: usize,
    wrap_width: usize,
) {
    let line = &lines[at];
    let text = &line.cells[pane];
    let Some(query) = query else {
        return cell(w, text, width);
    };
    let full = &line.texts[pane];
    let visible = clipped(text, width);
    if visible.is_empty() || full.ends_with(layout::ABSENT) {
        return cell(w, text, width);
    }
    // Which wrapped line of the verse this is: count the preceding lines of the same row.
    let mut k = 0;
    while k < at && lines[at - k - 1].row == line.row {
        k += 1;
    }
    let Some((origin, length)) = wrapped_offset(full, wrap_width, k) else {
        return cell(w, text, width);
    };
    if length != text.len() {
        return cell(w, text, width);
    }
    let body = full.len().min(full.find(' ').unwrap_or(full.len()) + 1);
    write_marked(w, full, origin, origin + visible.len(), query, body);
    for _ in layout::display_width(visible)..width {
        w.push(' ');
    }
}

fn locate(rows: &[Row], hit: &Hit) -> Option<usize> {
    rows.iter().position(|row| {
        row.chapter == hit.chapter && row.number == hit.number && row.book == hit.book()
    })
}

fn render_panel(
    w: &mut String,
    finder: &SearchState,
    x: usize,
    width: usize,
    top: usize,
    bottom: usize,
) {
    let inner = width.saturating_sub(3);
    let tool = &finder.names[finder.pane];
    let mut row = top;
    let header = format!("Search {tool} · “{}”", finder.submitted_text());
    let _ = write!(
        w,
        "\x1b[{row};{x}H{}┃ {}",
        theme::RULE,
        if finder.focus {
            concat!("\x1b[1m", "\x1b[38;2;125;130;217m")
        } else {
            theme::DIM
        }
    );
    cell(w, &header, inner);
    let _ = write!(w, "\x1b[0m{}", theme::PAPER);
    row += 1;
    let summary = format!(
        "{} verses · {} matches · {}/{}",
        finder.hits.len(),
        finder.occurrences,
        if finder.hits.is_empty() {
            0
        } else {
            finder.selected + 1
        },
        finder.hits.len()
    );
    let _ = write!(w, "\x1b[{row};{x}H{}┃ {}", theme::RULE, theme::DIM);
    cell(w, &summary, inner);
    row += 1;
    let capacity = (bottom + 1).saturating_sub(row);
    if capacity == 0 {
        return;
    }
    let first = finder
        .selected
        .saturating_sub(capacity / 2)
        .min(finder.hits.len().saturating_sub(capacity));
    for offset in 0..capacity {
        let _ = write!(w, "\x1b[{};{x}H{}┃ ", row + offset, theme::RULE);
        let index = first + offset;
        let Some(hit) = finder.hits.get(index) else {
            cell(w, "", inner);
            continue;
        };
        let chosen = index == finder.selected;
        let own = hit.label.chapter != hit.chapter || hit.label.number != hit.number;
        let marker = if chosen { "▶ " } else { "  " };
        let place = if own {
            format!(
                "{marker}{} {}:{} ({tool} {}:{}) ",
                hit.book(),
                hit.chapter,
                hit.number,
                hit.label.chapter,
                hit.label.number
            )
        } else {
            format!("{marker}{} {}:{} ", hit.book(), hit.chapter, hit.number)
        };
        w.push_str(if chosen { theme::ACCENT } else { theme::DIM });
        let place_visible = clipped(&place, inner);
        w.push_str(place_visible);
        let mut used = layout::display_width(place_visible);
        w.push_str(theme::INK);
        // Show a little context before the first match, from a word start.
        let mut from = 0;
        let start = hit.match_range.start;
        if start > 24 {
            from = start - 24;
            while from < start && (hit.text.as_bytes()[from] & 0xC0) == 0x80 {
                from += 1;
            }
            if let Some(space) = hit.text.as_bytes()[from..start]
                .iter()
                .position(|&b| b == b' ')
            {
                from += space + 1;
            }
            if used + 1 < inner {
                w.push('…');
                used += 1;
            }
        }
        let visible = clipped(&hit.text[from..], inner.saturating_sub(used));
        write_marked(w, &hit.text, from, from + visible.len(), &finder.active, 0);
        used += layout::display_width(visible);
        for _ in used..inner {
            w.push(' ');
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Pick(usize),
    Remove(usize),
}

#[derive(Default)]
pub struct ChooseOptions<'a> {
    /// Text each row is matched against by the `/` filter (default: the label).
    pub haystack: Option<&'a [String]>,
    /// The first N rows can be removed with `x` (after a y confirmation).
    pub removable_rows: usize,
}

fn refilter(labels: &[String], haystack: Option<&[String]>, filter: &str) -> Vec<usize> {
    labels
        .iter()
        .enumerate()
        .filter(|(index, label)| {
            let text = haystack.map_or(label.as_str(), |h| h[*index].as_str());
            filter.is_empty() || matches_filter(text, filter)
        })
        .map(|(index, _)| index)
        .collect()
}

/// Drops the last character of `buffer` the way the Zig prompts do: one byte, then any
/// continuation bytes it exposed.
fn pop_character(buffer: &mut Vec<u8>) {
    if buffer.is_empty() {
        return;
    }
    let mut len = buffer.len() - 1;
    while len > 0 && (buffer[len] & 0xC0) == 0x80 {
        len -= 1;
    }
    buffer.truncate(len);
}

/// Browse marker: returned text starts with this byte, followed by the text typed so far (the
/// caller opens a folder browser there).
pub const BROWSE_MARKER: char = '\t';

/// The owned terminal session: alternate screen, raw mode, bracketed paste. Restores everything
/// on drop.
pub struct Tui {
    decoder: Decoder,
    colors_256: bool,
    _raw: backend::Terminal,
}

impl Tui {
    pub fn new() -> io::Result<Tui> {
        let raw = backend::Terminal::new()?;
        let colors_256 = color::detect_256(|name| std::env::var(name).ok(), cfg!(windows));
        let tui = Tui {
            decoder: Decoder::new(),
            colors_256,
            _raw: raw,
        };
        tui.write("\x1b[?1049h\x1b[?25l\x1b[?2004h")?;
        Ok(tui)
    }

    pub fn colors_256(&self) -> bool {
        self.colors_256
    }

    /// Writes a frame, rewriting 24-bit colours when the terminal cannot show them.
    pub fn write(&self, text: &str) -> io::Result<()> {
        if self.colors_256 {
            backend::write_all(&color::downsample(text.as_bytes()))
        } else {
            backend::write_all(text.as_bytes())
        }
    }

    /// Raw bytes (sixel/kitty graphics) bypass colour rewriting.
    pub fn write_raw(&self, bytes: &[u8]) -> io::Result<()> {
        backend::write_all(bytes)
    }

    pub fn is_interrupted(&self) -> bool {
        backend::is_interrupted()
    }

    /// Input already waiting: skip redrawing until the burst is applied.
    pub fn input_pending(&self) -> bool {
        backend::pending()
    }

    pub fn screen_size(&self) -> io::Result<Size> {
        backend::size()
    }

    /// Text entry: printable and UTF-8 bytes (including pasted text) reach the caller.
    pub fn set_text_input(&mut self, on: bool) {
        self.decoder.text = on;
    }

    /// Queries the terminal's image support. Call after `new` (raw mode), before any key
    /// reading. Bounded by `timeout_ms`; never blocks longer.
    pub fn detect_graphics(&mut self, timeout_ms: u32) -> Capabilities {
        if self.write(graphics::QUERY).is_err() {
            return Capabilities::default();
        }
        let (mut reply, mut waited) = (Vec::new(), 0);
        while waited < timeout_ms && reply.len() < 512 {
            match backend::read_byte() {
                Err(_) => return Capabilities::default(),
                Ok(Read::Byte(byte)) => {
                    reply.push(byte);
                    if byte == b'c'
                        && let Some(caps) = graphics::parse(&reply)
                    {
                        return match backend::size() {
                            Err(_) => caps,
                            Ok(size) => caps
                                .with_cells_from(size.columns, size.rows)
                                .with_window_pixels(
                                    size.columns,
                                    size.rows,
                                    size.pixel_width,
                                    size.pixel_height,
                                ),
                        };
                    }
                }
                Ok(Read::Timeout) => waited += 80,
                Ok(Read::Ignored) => {}
            }
        }
        Capabilities::default()
    }

    /// Next key, or None on timeout/ignored bytes. Ctrl-C raises the interrupt flag.
    pub fn read_key(&mut self) -> io::Result<Option<u8>> {
        if backend::is_interrupted() {
            return Ok(None);
        }
        let key = match backend::read_byte()? {
            Read::Byte(byte) => self.decoder.feed(byte),
            Read::Timeout => self.decoder.timeout(),
            Read::Ignored => None,
        };
        if key == Some(3) {
            backend::interrupt();
            return Ok(None);
        }
        Ok(key)
    }

    /// One-line status screen shown while slow work (EPUB conversion) runs.
    pub fn status(&self, text: &str) -> io::Result<()> {
        let size = backend::size().unwrap_or(Size {
            columns: 80,
            rows: 24,
            pixel_width: 0,
            pixel_height: 0,
        });
        let mut w = String::new();
        let _ = write!(
            w,
            "\x1b[H\x1b[2J{}{} K A T A  |  ",
            theme::PAPER,
            theme::INK
        );
        w.push_str(clipped(text, size.columns.saturating_sub(14)));
        self.write(&w)
    }

    /// Single-line text prompt. Returns the entered text, or None when cancelled with Esc.
    pub fn prompt(
        &mut self,
        title: &str,
        description: &str,
        initial: &str,
    ) -> io::Result<Option<String>> {
        self.prompt_with(title, description, initial, false)
    }

    /// As `prompt`; with `browsable`, Tab returns `BROWSE_MARKER` followed by the text.
    pub fn prompt_with(
        &mut self,
        title: &str,
        description: &str,
        initial: &str,
        browsable: bool,
    ) -> io::Result<Option<String>> {
        let mut buffer: Vec<u8> = initial.as_bytes()[..initial.len().min(1024)].to_vec();
        self.decoder.text = true;
        let result = self.prompt_loop(title, description, &mut buffer, browsable);
        self.decoder.text = false;
        result
    }

    fn prompt_loop(
        &mut self,
        title: &str,
        description: &str,
        buffer: &mut Vec<u8>,
        browsable: bool,
    ) -> io::Result<Option<String>> {
        let (mut redraw, mut invalid) = (true, false);
        let mut old = Size {
            columns: 0,
            rows: 0,
            pixel_width: 0,
            pixel_height: 0,
        };
        while !backend::is_interrupted() {
            let size = backend::size()?;
            if size.columns != old.columns || size.rows != old.rows {
                redraw = true;
            }
            if redraw {
                let mut w = String::new();
                let _ = write!(
                    w,
                    "\x1b[H\x1b[2J{}{} K A T A  |  ",
                    theme::PAPER,
                    theme::INK
                );
                w.push_str(clipped(title, size.columns.saturating_sub(14)));
                if size.columns < 24 || size.rows < 10 {
                    w.push_str("\r\nEnlarge terminal. Esc cancels.");
                } else {
                    let wrapped = layout::wrap(description, size.columns.saturating_sub(2))
                        .unwrap_or_else(|_| vec![description]);
                    let shown = wrapped.len().min(size.rows.saturating_sub(7));
                    for (index, line) in wrapped[..shown].iter().enumerate() {
                        let _ = write!(w, "\x1b[{};1H{}", index + 3, theme::DIM);
                        w.push_str(clipped(line, size.columns));
                    }
                    let text = String::from_utf8_lossy(buffer).into_owned();
                    let visible = if layout::display_width(&text) + 4 > size.columns {
                        // Show the end of a long entry.
                        let mut start = 0;
                        while start < text.len()
                            && layout::display_width(text.get(start..).unwrap_or("")) + 4
                                > size.columns
                        {
                            start += 1;
                        }
                        while start < text.len() && !text.is_char_boundary(start) {
                            start += 1;
                        }
                        text[start..].to_string()
                    } else {
                        text
                    };
                    let _ = write!(w, "\x1b[{};1H{}> {}", shown + 4, theme::ACCENT, theme::INK);
                    w.push_str(&visible);
                    w.push('█');
                    let _ = write!(w, "\x1b[{};1H{}", size.rows - 1, theme::ACCENT);
                    w.push_str(clipped(
                        if browsable {
                            "Type or paste · Tab browses folders · Enter accepts · Backspace deletes · Ctrl-u clears · Esc cancels"
                        } else {
                            "Type or paste · Enter accepts · Backspace deletes · Ctrl-u clears · Esc cancels"
                        },
                        size.columns,
                    ));
                    if invalid {
                        let _ = write!(w, "\x1b[{};1H{}", size.rows, theme::DIM);
                        w.push_str(clipped(
                            "Enter some text first (it must be valid UTF-8).",
                            size.columns,
                        ));
                    }
                }
                self.write(&w)?;
                old = size;
                redraw = false;
            }
            let Some(key) = self.read_key()? else {
                continue;
            };
            redraw = true;
            invalid = false;
            match key {
                27 => return Ok(None),
                10 | 13 => {
                    let trimmed = String::from_utf8(buffer.clone())
                        .ok()
                        .map(|s| s.trim_matches(' ').to_string());
                    match trimmed {
                        Some(text) if !text.is_empty() => return Ok(Some(text)),
                        _ => invalid = true,
                    }
                }
                127 | 8 => pop_character(buffer),
                21 => buffer.clear(),
                9 if browsable => {
                    return Ok(Some(format!(
                        "{BROWSE_MARKER}{}",
                        String::from_utf8_lossy(buffer)
                    )));
                }
                9 => {}
                key if key >= 0x20 && key != 0x7f && buffer.len() < 1024 => buffer.push(key),
                _ => {}
            }
        }
        Ok(None)
    }

    /// Generic numbered menu.
    pub fn choose(
        &mut self,
        title: &str,
        description: &str,
        labels: &[String],
        initial: usize,
        number_base: usize,
    ) -> io::Result<Option<usize>> {
        Ok(self
            .choose_opts(
                title,
                description,
                labels,
                initial,
                number_base,
                &ChooseOptions::default(),
            )?
            .map(|choice| match choice {
                Choice::Pick(index) | Choice::Remove(index) => index,
            }))
    }

    /// As `choose`, with a vim-style `/` filter and optional `x` removal. Numbers shown and typed
    /// refer to positions in the (filtered) list; the returned index is always into `labels`.
    pub fn choose_opts(
        &mut self,
        title: &str,
        description: &str,
        labels: &[String],
        initial: usize,
        number_base: usize,
        options: &ChooseOptions,
    ) -> io::Result<Option<Choice>> {
        if labels.is_empty() {
            return Err(io::Error::other("EmptyMenu"));
        }
        let mut picker = Picker::numbered(initial.min(labels.len() - 1), number_base);
        let mut filter: Vec<u8> = Vec::new();
        let (mut filtering, mut confirming) = (false, false);
        let mut visible = refilter(labels, options.haystack, "");
        let (mut redraw, mut old_columns, mut old_height) = (true, 0, 0);
        let result = (|| -> io::Result<Option<Choice>> {
            while !backend::is_interrupted() {
                let size = backend::size()?;
                let (columns, height) = (size.columns, size.rows);
                if columns != old_columns || height != old_height {
                    redraw = true;
                }
                if redraw {
                    let filter_text = String::from_utf8_lossy(&filter).into_owned();
                    let mut w = String::new();
                    let _ = write!(
                        w,
                        "\x1b[H\x1b[2J{}{} K A T A  |  ",
                        theme::PAPER,
                        theme::INK
                    );
                    w.push_str(clipped(title, columns.saturating_sub(14)));
                    if columns < 24 || height < 8 {
                        w.push_str("\r\nEnlarge terminal. Esc/q returns.");
                    } else {
                        let _ = write!(w, "\x1b[2;1H{}", theme::DIM);
                        w.push_str(clipped(description, columns));
                        let capacity = height - 6;
                        let count = visible.len();
                        let first = picker
                            .selected
                            .saturating_sub(capacity / 2)
                            .min(count.saturating_sub(capacity));
                        for position in first..count.min(first + capacity) {
                            let _ = write!(w, "\x1b[{};1H", position - first + 3);
                            w.push_str(if position == picker.selected {
                                concat!("\x1b[38;2;125;130;217m", "▶ ")
                            } else {
                                concat!("\x1b[38;2;255;206;173m", "  ")
                            });
                            let _ = write!(w, "{}. ", position + number_base);
                            w.push_str(clipped(
                                &labels[visible[position]],
                                columns.saturating_sub(9),
                            ));
                        }
                        if count == 0 {
                            let _ = write!(w, "\x1b[3;1H{}  No matches.", theme::DIM);
                        }
                        let _ = write!(w, "\x1b[{};1H{}", height - 1, theme::ACCENT);
                        if confirming {
                            let ask = format!(
                                "Remove \"{}\"? y confirms · any other key cancels",
                                labels[visible[picker.selected]]
                            );
                            w.push_str(clipped(&ask, columns));
                        } else if filtering {
                            w.push_str(clipped("Type to filter · Enter keeps the filter · Esc clears · Backspace deletes", columns));
                        } else {
                            w.push_str(clipped(
                                if options.removable_rows > 0 {
                                    "j/k select · number · Enter opens · / filter · x remove · Esc/q back"
                                } else {
                                    "j/k select · type number · Enter opens · / filter · Esc/q back (or quit at Library)"
                                },
                                columns,
                            ));
                        }
                        let _ = write!(w, "\x1b[{height};1H{}", theme::DIM);
                        if filtering {
                            let _ =
                                write!(w, "/{}█", clipped(&filter_text, columns.saturating_sub(3)));
                        } else if !filter.is_empty() {
                            let _ = write!(
                                w,
                                "Filter: {} · {} of {} shown · Esc clears",
                                clipped(&filter_text, columns.saturating_sub(40)),
                                count,
                                labels.len()
                            );
                        } else if picker.digit_count > 0 {
                            let _ = write!(
                                w,
                                "Selection: {}",
                                String::from_utf8_lossy(picker.digits())
                            );
                        }
                    }
                    self.write(&w)?;
                    old_columns = columns;
                    old_height = height;
                    redraw = false;
                }
                let Some(key) = self.read_key()? else {
                    continue;
                };
                redraw = true;
                if confirming {
                    confirming = false;
                    if key == b'y' {
                        return Ok(Some(Choice::Remove(visible[picker.selected])));
                    }
                    continue;
                }
                if filtering {
                    let mut changed = false;
                    match key {
                        10 | 13 => {
                            filtering = false;
                            self.decoder.text = false;
                        }
                        27 => {
                            filter.clear();
                            filtering = false;
                            self.decoder.text = false;
                            changed = true;
                        }
                        127 | 8 => {
                            if filter.is_empty() {
                                filtering = false;
                                self.decoder.text = false;
                            } else {
                                pop_character(&mut filter);
                                changed = true;
                            }
                        }
                        21 => {
                            filter.clear();
                            changed = true;
                        }
                        key if key >= 0x20 && key != 0x7f && filter.len() < 64 => {
                            filter.push(key);
                            changed = true;
                        }
                        _ => {}
                    }
                    if changed {
                        visible =
                            refilter(labels, options.haystack, &String::from_utf8_lossy(&filter));
                        picker.selected = 0;
                    }
                    continue;
                }
                if key == b'/' {
                    filtering = true;
                    self.decoder.text = true;
                    picker.digit_count = 0;
                    continue;
                }
                if (key == 27 || key == b'q') && !filter.is_empty() {
                    // Clear the filter first, keeping the highlighted row highlighted.
                    let kept = visible.get(picker.selected).copied().unwrap_or(0);
                    filter.clear();
                    visible = refilter(labels, options.haystack, "");
                    picker.selected = kept.min(visible.len().saturating_sub(1));
                    continue;
                }
                if key == b'x' && options.removable_rows > 0 {
                    if picker.selected < visible.len()
                        && visible[picker.selected] < options.removable_rows
                    {
                        confirming = true;
                    }
                    continue;
                }
                if let Some(Action::ChoosePlan(choice)) =
                    handle_picker(&mut picker, key, visible.len(), &mut redraw)
                {
                    return Ok(Some(Choice::Pick(visible[choice])));
                }
                if picker.kind == Kind::Closed {
                    return Ok(None);
                }
            }
            Ok(None)
        })();
        self.decoder.text = false;
        result
    }

    /// The caller owns terminal mode for the entire application, including plan changes. Returns
    /// the action that leaves the reader, or None when the user quit.
    pub fn read(
        &mut self,
        rows: &[Row],
        state: &mut State,
        options: Options<'_>,
    ) -> io::Result<Option<Action>> {
        let result = self.read_loop(rows, state, options);
        self.decoder.text = false;
        result
    }

    fn read_loop(
        &mut self,
        rows: &[Row],
        state: &mut State,
        mut options: Options<'_>,
    ) -> io::Result<Option<Action>> {
        let mut lines: Vec<Line> = Vec::new();
        let mut tops = [0usize; 3];
        let (mut old_width, mut old_height) = (0usize, 0usize);
        let (mut screen_columns, mut screen_rows) = (0usize, 0usize);
        let (mut redraw, mut confirm) = (true, false);
        let mut picker = Picker::default();
        let mut notice = options.notice.clone();
        let mut finder = options.search.take();
        let mut pending_row: Option<usize> = None;
        if let Some(f) = finder.as_deref_mut()
            && f.reveal
        {
            // A search result opened this reading: show it in the searched source.
            f.reveal = false;
            state.enabled[f.pane] = true;
            state.focus = f.pane;
        }
        if !state.enabled[state.focus] {
            move_focus(state, 1);
        }
        while !backend::is_interrupted() {
            let size = backend::size()?;
            let (columns, height) = (size.columns, size.rows);
            if columns != screen_columns || height != screen_rows {
                screen_columns = columns;
                screen_rows = height;
                redraw = true;
            }
            let panel_open = finder.as_deref().is_some_and(|f| f.open);
            let panel_width = if !panel_open {
                0
            } else if columns >= 72 {
                (columns * 2 / 5).clamp(30, 64)
            } else {
                columns
            };
            let reader_visible = panel_width < columns;
            let count = selected_count(state.enabled).max(1);
            let pane_width = ((columns - panel_width) / count).saturating_sub(3);
            let body_height = height.saturating_sub(6);
            let usable = body_height >= 3 && (!reader_visible || pane_width >= 12);
            if usable && reader_visible && (pane_width != old_width || height != old_height) {
                remember(&lines, tops, state);
                lines = layout::render_lines(rows, pane_width, state.enabled).map_err(other)?;
                for (top, position) in tops.iter_mut().zip(state.positions) {
                    *top = restore(&lines, position).min(max_top(&lines, body_height));
                }
                if state.linked {
                    tops = [tops[state.focus]; 3];
                }
                old_width = pane_width;
                old_height = height;
                redraw = true;
            }
            if let Some(target) = pending_row
                && !lines.is_empty()
                && old_width != 0
            {
                pending_row = None;
                let line_index = lines
                    .iter()
                    .position(|line| line.row == target)
                    .unwrap_or(0);
                let top = line_index.min(max_top(&lines, body_height));
                if state.linked {
                    tops = [top; 3];
                } else {
                    tops[state.focus] = top;
                }
                redraw = true;
            }
            if redraw {
                let mut w = String::new();
                let _ = write!(w, "\x1b[H\x1b[2J{}{}", theme::PAPER, theme::INK);
                let prompting = finder.as_deref().is_some_and(|f| f.prompt);
                let panel_focus = finder.as_deref().is_some_and(|f| f.open && f.focus);
                if picker.kind != Kind::Closed {
                    render_picker(&mut w, &picker, &options, columns, height);
                } else if !usable {
                    w.push_str("Kata: enlarge the terminal or press 1/2/3 to hide panes. q quits.");
                } else if rows.is_empty() {
                    w.push_str(" K A T A  |  Plan complete. m opens Library; p chooses a plan; d restarts at a day; q quits.");
                } else {
                    let _ = write!(
                        w,
                        "\x1b[1;1H\x1b[1m{} K A T A  \x1b[0m{}{}",
                        theme::ACCENT,
                        theme::PAPER,
                        theme::INK
                    );
                    let title = format!(
                        "{}  [{}]{}",
                        options.title,
                        if state.linked {
                            "LINKED"
                        } else {
                            "INDEPENDENT"
                        },
                        if options.plan_mode { "" } else { " [FREE]" }
                    );
                    w.push_str(clipped(&title, columns.saturating_sub(12)));
                    let _ = write!(w, "\x1b[2;1H{}", theme::DIM);
                    w.push_str(clipped(&notice, columns));
                    let highlight = finder.as_deref().and_then(|f| f.highlight());
                    if reader_visible {
                        w.push_str("\x1b[3;1H");
                        for (pane, &top) in tops.iter().enumerate() {
                            if !state.enabled[pane] {
                                continue;
                            }
                            let row_index = if lines.is_empty() {
                                0
                            } else {
                                lines[top.min(lines.len() - 1)].row
                            };
                            let row = &rows[row_index];
                            let label = format!(
                                "{}{} · {} {}",
                                if state.focus == pane { "▶ " } else { "  " },
                                options.names[pane],
                                row.book,
                                row.label(pane).chapter
                            );
                            w.push_str(if state.focus == pane && !panel_focus {
                                theme::ACCENT
                            } else {
                                theme::DIM
                            });
                            cell(&mut w, &label, pane_width);
                            w.push_str(" │ ");
                        }
                        for line_index in 0..body_height {
                            let _ = write!(w, "\x1b[{};1H", line_index + 4);
                            for (pane, &top) in tops.iter().enumerate() {
                                if !state.enabled[pane] {
                                    continue;
                                }
                                w.push_str(theme::INK);
                                let at = top + line_index;
                                let query = highlight.and_then(|(p, q)| (p == pane).then_some(q));
                                if at < lines.len() {
                                    marked_cell(
                                        &mut w, &lines, at, pane, query, pane_width, pane_width,
                                    );
                                } else {
                                    cell(&mut w, "", pane_width);
                                }
                                let _ = write!(w, "{} │ ", theme::RULE);
                            }
                        }
                    }
                    if panel_open && let Some(f) = finder.as_deref() {
                        render_panel(
                            &mut w,
                            f,
                            columns - panel_width + 1,
                            panel_width,
                            3,
                            height - 2,
                        );
                    }
                    let _ = write!(w, "\x1b[{};1H{}", height - 1, theme::ACCENT);
                    let help = if confirm {
                        "Mark today's assignment complete? y confirms · any other key cancels"
                    } else if prompting {
                        "Type a word or phrase · matches highlight as you type · Enter searches the whole source · Esc cancels"
                    } else if panel_focus {
                        "j/k select · Enter open · n/N next/previous · Tab/Esc back to reader · / new search · x close search"
                    } else if panel_open {
                        "j/k scroll · h/l focus · / search · r results · n/N next/previous match · x close search"
                    } else {
                        "j/k or ↑/↓ scroll · Ctrl-d/u page · h/l or ←/→ focus · s sync · 1/2/3 panes · g/G ends · / search · e edition"
                    };
                    w.push_str(clipped(help, columns));
                    let _ = write!(w, "\x1b[{height};1H{}", theme::DIM);
                    if prompting && let Some(f) = finder.as_deref() {
                        let line = format!(
                            "/{}█   searching {}",
                            f.prompt_text(),
                            f.names[f.prompt_pane]
                        );
                        w.push_str(theme::INK);
                        w.push_str(clipped(&line, columns));
                    } else {
                        w.push_str(clipped(
                            if options.plan_mode {
                                "m library · o free reading · p plans · d choose day · c complete day · q quit"
                            } else if options.completable {
                                "m plan · c complete day · q quit"
                            } else {
                                "m library · o choose place · H/L (Shift+←/→) previous/next chapter · p plans · q quit"
                            },
                            columns,
                        ));
                    }
                }
                self.write(&w)?;
                redraw = false;
            }
            self.decoder.text =
                finder.as_deref().is_some_and(|f| f.prompt) && picker.kind == Kind::Closed;
            let Some(key) = self.read_key()? else {
                continue;
            };
            if picker.kind != Kind::Closed {
                let menu_count = if picker.kind == Kind::Plans {
                    options.plans.len()
                } else {
                    options.plan.map_or(0, |plan| plan.total_days())
                };
                if let Some(action) = handle_picker(&mut picker, key, menu_count, &mut redraw) {
                    remember(&lines, tops, state);
                    return Ok(Some(action));
                }
                continue;
            }
            if confirm {
                confirm = false;
                if key == b'y' {
                    if !complete_day(state, options.today).map_err(other)? {
                        notice = "Already complete today. Your next assignment becomes available tomorrow.".into();
                        redraw = true;
                        continue;
                    }
                    remember(&lines, tops, state);
                    state::save(&options.state_path, state).map_err(other)?;
                    notice = "Completed and saved. Next assignment tomorrow; no catch-up workload."
                        .into();
                }
                redraw = true;
                continue;
            }
            // Search prompt and results panel take keys before the reader.
            let mut want_jump = false;
            if let Some(f) = finder.as_deref_mut() {
                if f.prompt {
                    match key {
                        27 => f.cancel(),
                        10 | 13 => {
                            let query = f.prompt_text().to_string();
                            f.submit();
                            if f.open && f.hits.is_empty() {
                                notice =
                                    format!("No matches for “{query}” in {}.", f.names[f.pane]);
                                f.close();
                            } else if f.open {
                                // Start the list at the first result in this reading, if any.
                                if let Some(first) =
                                    f.hits.iter().position(|hit| locate(rows, hit).is_some())
                                {
                                    f.selected = first;
                                }
                                notice = format!(
                                    "{} verses contain “{query}” in {}. Enter opens a result; n/N steps through them.",
                                    f.hits.len(),
                                    f.names[f.pane]
                                );
                            }
                        }
                        127 | 8 => f.backspace(),
                        21 => f.clear(),
                        key => f.input(key),
                    }
                    redraw = true;
                    continue;
                }
                let last = f.hits.len().saturating_sub(1);
                if f.open && f.focus {
                    let page = (body_height / 2).max(1);
                    match key {
                        b'j' => f.selected = last.min(f.selected + 1),
                        b'k' => f.selected = f.selected.saturating_sub(1),
                        4 | b'f' => f.selected = last.min(f.selected + page),
                        21 | b'b' => f.selected = f.selected.saturating_sub(page),
                        b'g' => f.selected = 0,
                        b'G' => f.selected = last,
                        10 | 13 => want_jump = true,
                        b'n' => {
                            f.selected = last.min(f.selected + 1);
                            want_jump = true;
                        }
                        b'N' => {
                            f.selected = f.selected.saturating_sub(1);
                            want_jump = true;
                        }
                        27 | 9 | b'q' | b'h' => f.focus = false,
                        b'x' => f.close(),
                        b'/' => f.begin(state.focus),
                        _ => continue,
                    }
                    if !want_jump {
                        redraw = true;
                        continue;
                    }
                } else if f.open && (key == b'n' || key == b'N') {
                    f.selected = if key == b'n' {
                        last.min(f.selected + 1)
                    } else {
                        f.selected.saturating_sub(1)
                    };
                    want_jump = true;
                }
                if want_jump {
                    let Some(hit) = f.current().cloned() else {
                        continue;
                    };
                    if let Some(target) = locate(rows, &hit) {
                        if !state.enabled[f.pane] {
                            remember(&lines, tops, state);
                            state.enabled[f.pane] = true;
                            old_width = 0;
                        }
                        state.focus = f.pane;
                        pending_row = Some(target);
                        redraw = true;
                        continue;
                    }
                    remember(&lines, tops, state);
                    f.reveal = true;
                    return Ok(Some(Action::OpenLocation(hit.location())));
                }
            }
            let page = (body_height / 2).max(1) as i64;
            let maximum = max_top(&lines, body_height);
            match key {
                b'q' | 3 => break,
                b'e' => {
                    if options.editions[state.focus] < 2 {
                        notice = if state.focus == 0 {
                            "No other edition here. With a Psalter ingested, e switches column 1 between KJV and it in the Psalms.".into()
                        } else {
                            "Only one edition for this column.".into()
                        };
                    } else {
                        remember(&lines, tops, state);
                        state.edition[state.focus] =
                            (state.edition[state.focus] + 1) % options.editions[state.focus];
                        return Ok(Some(Action::Reload));
                    }
                }
                b'm' | b'o' | b'H' | b'L' => {
                    if (key == b'H' || key == b'L') && options.plan_mode {
                        continue;
                    }
                    remember(&lines, tops, state);
                    return Ok(Some(match key {
                        b'm' => Action::Home,
                        b'o' => Action::OpenPlace,
                        b'H' => Action::PreviousChapter,
                        _ => Action::NextChapter,
                    }));
                }
                b'j' => shift(&mut tops, state, 1, maximum),
                b'k' => shift(&mut tops, state, -1, maximum),
                4 | b'f' => shift(&mut tops, state, page, maximum),
                21 | b'b' => shift(&mut tops, state, -page, maximum),
                b'g' => shift(&mut tops, state, -(lines.len() as i64), maximum),
                b'G' => shift(&mut tops, state, lines.len() as i64, maximum),
                b'h' => move_focus(state, -1),
                b'l' | 9 => move_focus(state, 1),
                b's' => {
                    state.linked = !state.linked;
                    if state.linked {
                        tops = [tops[state.focus]; 3];
                    }
                }
                b'1' | b'2' | b'3' => {
                    let pane = usize::from(key - b'1');
                    if state.enabled[pane] && count == 1 {
                        continue;
                    }
                    remember(&lines, tops, state);
                    state.enabled[pane] = !state.enabled[pane];
                    if !state.enabled[state.focus] {
                        move_focus(state, 1);
                    }
                    old_width = 0;
                }
                b'c' => {
                    if options.plan_mode || options.completable {
                        if rows.is_empty() {
                            notice = "Plan complete. Choose a plan or restart at a day.".into();
                        } else {
                            confirm = true;
                        }
                    }
                }
                b'p' => {
                    if !options.plans.is_empty() {
                        picker = Picker::at(Kind::Plans, options.active_plan);
                    }
                }
                b'd' => {
                    if options.plan_mode
                        && let Some(plan) = options.plan
                    {
                        let total = plan.total_days();
                        picker = Picker::at(
                            Kind::Days,
                            total
                                .saturating_sub(1)
                                .min(state.assignment(options.today) % total.max(1)),
                        );
                    }
                }
                b'/' => {
                    let Some(f) = finder.as_deref_mut() else {
                        continue;
                    };
                    if rows.is_empty() {
                        continue;
                    }
                    f.begin(state.focus);
                }
                b'r' => {
                    let Some(f) = finder.as_deref_mut() else {
                        continue;
                    };
                    if !f.open {
                        continue;
                    }
                    f.focus = true;
                }
                b'x' => {
                    let Some(f) = finder.as_deref_mut() else {
                        continue;
                    };
                    if !f.open {
                        continue;
                    }
                    f.close();
                    notice = "Search closed.".into();
                }
                _ => continue,
            }
            redraw = true;
        }
        remember(&lines, tops, state);
        Ok(None)
    }
}

impl Drop for Tui {
    fn drop(&mut self) {
        let _ = backend::write_all(SESSION_OFF.as_bytes());
    }
}

fn handle_picker(picker: &mut Picker, key: u8, count: usize, redraw: &mut bool) -> Option<Action> {
    let (kind, selected, digits, invalid) = (
        picker.kind,
        picker.selected,
        picker.digit_count,
        picker.invalid_input,
    );
    let action = picker.handle(key, count);
    if kind != picker.kind
        || selected != picker.selected
        || digits != picker.digit_count
        || invalid != picker.invalid_input
    {
        *redraw = true;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use kata_core::layout::Row;

    #[test]
    fn clipping_and_cells_count_terminal_columns_not_bytes() {
        assert_eq!(clipped("ἀγάπη", 3), "ἀγά");
        assert_eq!(clipped("日本語", 3), "日");
        let mut w = String::new();
        cell(&mut w, "ab", 5);
        assert_eq!(w, "ab   ");
        w.clear();
        cell(&mut w, "日本語", 4);
        assert_eq!(w, "日本");
    }

    #[test]
    fn menu_filter_matches_words_by_prefix_any_order_accent_insensitively() {
        assert!(matches_filter(
            "Building Microservices · Sam Newman",
            "micro"
        ));
        assert!(matches_filter(
            "Building Microservices · Sam Newman",
            "newman build"
        ));
        assert!(!matches_filter(
            "Building Microservices · Sam Newman",
            "services"
        ));
        assert!(matches_filter("Élan Vital · Bergson", "elan"));
        assert!(matches_filter("anything", ""));
        assert!(!matches_filter("", "x"));
    }

    #[test]
    fn marked_text_highlights_matches_and_restores_the_colour() {
        let mut w = String::new();
        let query = Query::new("love");
        write_marked_styled(&mut w, "I love; he loveth.", 0, 18, &query, 0, "<off>");
        assert_eq!(w, format!("I {MARK_ON}love<off>; he {MARK_ON}loveth<off>."));
        w.clear();
        // A match cut by the visible window is clipped to it.
        write_marked_styled(&mut w, "he loveth", 0, 6, &query, 0, "<off>");
        assert_eq!(w, format!("he {MARK_ON}lov<off>"));
    }

    #[test]
    fn scrolling_moves_linked_panes_together_and_clamps() {
        let state = State {
            linked: true,
            ..State::default()
        };
        let mut tops = [0, 5, 9];
        shift(&mut tops, &state, 3, 10);
        assert_eq!(tops, [3, 8, 10]);
        shift(&mut tops, &state, -20, 10);
        assert_eq!(tops, [0, 0, 0]);
        let single = State {
            linked: false,
            focus: 1,
            ..State::default()
        };
        shift(&mut tops, &single, 4, 10);
        assert_eq!(tops, [0, 4, 0]);
    }

    #[test]
    fn positions_round_trip_through_remember_and_restore() {
        let lines: Vec<Line> = [0, 0, 0, 1, 1, 2]
            .iter()
            .map(|&row| Line {
                cells: Default::default(),
                row,
                texts: Default::default(),
            })
            .collect();
        let mut state = State::default();
        remember(&lines, [4, 1, 5], &mut state);
        assert_eq!(state.positions[0], Position { row: 1, line: 1 });
        assert_eq!(state.positions[1], Position { row: 0, line: 1 });
        assert_eq!(restore(&lines, state.positions[0]), 4);
        assert_eq!(restore(&lines, Position { row: 1, line: 9 }), 4);
        assert_eq!(restore(&lines, Position { row: 9, line: 0 }), 0);
    }

    #[test]
    fn focus_skips_hidden_panes() {
        let mut state = State {
            enabled: [true, false, true],
            focus: 0,
            ..State::default()
        };
        move_focus(&mut state, 1);
        assert_eq!(state.focus, 2);
        move_focus(&mut state, -1);
        assert_eq!(state.focus, 0);
    }

    #[test]
    fn prompt_backspace_removes_whole_characters() {
        let mut buffer = "aλ".as_bytes().to_vec();
        pop_character(&mut buffer);
        assert_eq!(buffer, b"a");
        pop_character(&mut buffer);
        pop_character(&mut buffer);
        assert!(buffer.is_empty());
    }

    #[test]
    fn completion_preserves_real_state_overflow_errors() {
        let mut state = State {
            next_day: usize::MAX,
            ..State::default()
        };
        assert_eq!(
            complete_day(&mut state, 20261007),
            Err(StateError::ProgressOverflow)
        );
        let mut done = State::default();
        assert_eq!(complete_day(&mut done, 20261007), Ok(true));
        assert_eq!(complete_day(&mut done, 20261007), Ok(false));
    }

    #[test]
    fn wrapped_offsets_locate_each_line_inside_the_full_text() {
        let text = "1:1 the quick brown fox jumps over";
        let lines = layout::wrap(text, 12).unwrap();
        for (k, line) in lines.iter().enumerate() {
            let (offset, length) = wrapped_offset(text, 12, k).unwrap();
            assert_eq!(&text[offset..offset + length], *line);
        }
        let _ = Row::new("Psalms", 1, 1);
    }
}
