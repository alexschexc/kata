//! Single-pane reader for ingested library documents, ported from `src/book_reader.zig`.
//! Chapters come from the document; `H`/`L` (Shift+←/→) move between them, `/` searches the whole
//! document with the same matching and results panel behaviour as the Bible reader. Positions
//! are saved per document.

use crate::theme::{ACCENT as GOLD, CODE as CODE_INK, DIM, INK, PAPER, RULE};
use crate::tui::{self, Tui};
use kata_core::document::{Block, Chapter, Document, Kind};
use kata_core::graphics::{Capabilities, Protocol};
use kata_core::inline_image::Renderer;
use kata_core::layout;
use kata_core::search::{self, Hit, Query, SearchState};
use kata_core::source::Label;
use kata_core::state::{self, State};
use kata_core::wyhash;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use std::io::{self, Write as _};
use std::path::Path;
use std::rc::Rc;

const GUTTER_WIDTH: usize = 6;
const CELL_SEPARATOR: &str = " │ ";
const STOP_TEXT: &str = "━━━━━━ end of today's reading · m returns to the plan ━━━━━━";

/// Saved position for one document (`<state>.books/<hash>.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Position {
    pub version: u8,
    pub chapter: usize,
    pub block: usize,
}

impl Default for Position {
    fn default() -> Self {
        Position {
            version: 1,
            chapter: 0,
            block: 0,
        }
    }
}

/// A plan day's reading from one book: the units to read, in order, as `(chapter, start, end)`
/// block ranges. The reader joins them into one continuous buffer ending in an end-of-reading
/// marker, like a Bible plan day.
#[derive(Debug, Clone)]
pub struct Portion {
    pub ranges: Vec<(usize, usize, usize)>,
    pub label: String,
    /// Identifies this plan, day and item; the reading place is saved under it.
    pub key: String,
}

/// The day's text as a single-chapter document.
pub fn portion_document(doc: &Document, portion: &Portion) -> Document {
    let blocks = portion
        .ranges
        .iter()
        .flat_map(|&(chapter, start, end)| {
            let all = &doc.chapters[chapter].blocks;
            all[start.min(all.len())..end.min(all.len())]
                .iter()
                .cloned()
        })
        .collect();
    Document {
        title: doc.title.clone(),
        author: doc.author.clone(),
        source: doc.source.clone(),
        converter: doc.converter,
        chapters: vec![Chapter {
            title: portion.label.clone(),
            blocks,
        }],
    }
}

/// Lets `c` mark the open plan day complete (plan readings only).
pub struct Completion<'a> {
    pub state: &'a mut State,
    pub path: &'a str,
    pub today: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Quit,
    Home,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Style {
    Body,
    Heading,
    Code,
    Quote,
    Note,
    Figure,
    Page,
}

/// One rendered line: `text` is the slice of `full` (the block's display text) starting at
/// `offset`, so search highlights work across wrapped lines.
#[derive(Debug, Clone)]
struct Line {
    block: usize,
    text: String,
    full: Rc<str>,
    offset: usize,
    gutter: String,
    style: Style,
    /// For inline images: asset name and which reserved row this line is.
    image: String,
    image_row: u16,
}

impl Line {
    fn plain(block: usize, text: &str, style: Style) -> Line {
        Line {
            block,
            text: text.to_string(),
            full: Rc::from(text),
            offset: 0,
            gutter: String::new(),
            style,
            image: String::new(),
            image_row: 0,
        }
    }
}

fn style_for(kind: Kind) -> Style {
    match kind {
        Kind::Heading => Style::Heading,
        Kind::Code => Style::Code,
        Kind::Quote | Kind::Superscription | Kind::Rubric => Style::Quote,
        Kind::Footnote => Style::Note,
        Kind::Figure | Kind::Image => Style::Figure,
        _ => Style::Body,
    }
}

fn offset_in(piece: &str, whole: &str) -> usize {
    if piece.is_empty() {
        0
    } else {
        piece.as_ptr() as usize - whole.as_ptr() as usize
    }
}

/// Wraps, falling back to the unwrapped text when the width is too small to wrap at all.
fn wrap_pieces(text: &str, width: usize) -> Vec<&str> {
    layout::wrap(text, width).unwrap_or_else(|_| vec![text])
}

/// Code keeps its spacing; long lines are hard-wrapped, not reflowed.
fn wrap_code(text: &str, width: usize) -> Vec<&str> {
    let mut out = Vec::new();
    for row in text.split('\n') {
        if row.is_empty() {
            out.push(row);
            continue;
        }
        let mut start = 0;
        while start < row.len() {
            let (mut end, mut used) = (start, 0);
            while end < row.len() {
                let n = row[end..].chars().next().map_or(1, char::len_utf8);
                let w = layout::display_width(&row[end..end + n]);
                if used + w > width && end > start {
                    break;
                }
                used += w;
                end += n;
            }
            out.push(&row[start..end]);
            start = end;
        }
    }
    out
}

/// Lays out one chapter at `width` columns (the text column excludes the gutter).
#[cfg(test)]
fn render(chapter: &Chapter, width: usize) -> Vec<Line> {
    render_with(chapter, width, None, 0, None)
}

/// `images` (when inline images are enabled) reserves rows for each image block instead of the
/// placeholder line; `max_rows` caps image height.
fn render_with(
    chapter: &Chapter,
    width: usize,
    mut images: Option<&mut Renderer>,
    max_rows: usize,
    stop: Option<usize>,
) -> Vec<Line> {
    let mut lines: Vec<Line> = Vec::new();
    let mut page = "";
    let stacked = stacked_lines(&chapter.blocks);
    for (index, block) in chapter.blocks.iter().enumerate() {
        if stop == Some(index) {
            append_stop(&mut lines, index);
        }
        if !block.page.is_empty() && block.page != page {
            if index > 0 {
                lines.push(Line::plain(
                    index,
                    &format!("── page {} ──", block.page),
                    Style::Page,
                ));
            }
            page = &block.page;
        }
        if index > 0 && block.kind.spaced() && !stacked[index] {
            lines.push(Line::plain(index, "", Style::Body));
        }
        let text = match block.kind {
            Kind::Image => format!("▣ image · i opens it  ({})", block.text),
            Kind::ListItem => format!("• {}", block.text),
            Kind::TableRow => format!("  {}", block.text),
            _ => block.text.clone(),
        };
        if block.kind == Kind::TableRow {
            // The first row of a run lays out the whole table as a grid.
            if index > 0 && chapter.blocks[index - 1].kind == Kind::TableRow {
                continue;
            }
            let end = (index..chapter.blocks.len())
                .find(|&i| chapter.blocks[i].kind != Kind::TableRow)
                .unwrap_or(chapter.blocks.len());
            if append_table(&mut lines, &chapter.blocks, index, end, width) {
                continue;
            }
            // Too narrow for a grid: rows fall through to flattened text.
            for (at, row) in chapter.blocks[index..end]
                .iter()
                .enumerate()
                .map(|(k, row)| (index + k, row))
            {
                let flat = format!("  {}", row.text);
                let full: Rc<str> = Rc::from(flat.as_str());
                for piece in wrap_pieces(&flat, width) {
                    lines.push(Line {
                        block: at,
                        text: piece.to_string(),
                        full: full.clone(),
                        offset: offset_in(piece, &flat),
                        ..Line::plain(at, "", Style::Body)
                    });
                }
            }
            continue;
        }
        if block.kind == Kind::Image
            && let Some(reserved) = images
                .as_mut()
                .and_then(|renderer| renderer.rows(&block.text, width, max_rows))
        {
            for r in 0..reserved {
                lines.push(Line {
                    image: block.text.clone(),
                    image_row: r,
                    ..Line::plain(index, "", Style::Figure)
                });
            }
            continue;
        }
        let gutter = if block.verse > 0 {
            block.verse.to_string()
        } else {
            String::new()
        };
        let mut wrapped: Vec<&str> = Vec::new();
        if block.kind == Kind::Code {
            wrapped.extend(wrap_code(&text, width.saturating_sub(2)));
        } else {
            for part in text.split('\n') {
                wrapped.extend(wrap_pieces(part, width));
            }
        }
        let full: Rc<str> = Rc::from(text.as_str());
        for (i, piece) in wrapped.into_iter().enumerate() {
            lines.push(Line {
                block: index,
                text: piece.to_string(),
                full: full.clone(),
                offset: offset_in(piece, &text),
                gutter: if i == 0 {
                    gutter.clone()
                } else {
                    String::new()
                },
                style: style_for(block.kind),
                image: String::new(),
                image_row: 0,
            });
        }
    }
    if let Some(stop) = stop
        && stop >= chapter.blocks.len()
    {
        append_stop(&mut lines, chapter.blocks.len().saturating_sub(1));
    }
    lines
}

fn append_stop(lines: &mut Vec<Line>, block: usize) {
    lines.push(Line::plain(block, "", Style::Body));
    lines.push(Line::plain(block, STOP_TEXT, Style::Heading));
    lines.push(Line::plain(block, "", Style::Body));
}

/// Lays out table rows [start, end) (cells joined by " │ " at conversion) as aligned columns
/// within `width`, wrapping long cells inside their column. Returns false when the columns cannot
/// fit (the caller flattens).
fn append_table(
    lines: &mut Vec<Line>,
    blocks: &[Block],
    start: usize,
    end: usize,
    width: usize,
) -> bool {
    const INDENT: usize = 2;
    const MIN_COLUMN: usize = 4;
    let mut rows: Vec<Vec<String>> = Vec::new();
    let mut columns = 0;
    for row in &blocks[start..end] {
        // Split on the bar alone: an empty last cell leaves a bare " │".
        let cells: Vec<String> = row
            .text
            .split('│')
            .map(|cell| cell.trim_matches(' ').to_string())
            .collect();
        columns = columns.max(cells.len());
        rows.push(cells);
    }
    if columns == 0 {
        return false;
    }
    // The separator width is its byte length, as in the Zig build.
    let separators = (columns - 1) * CELL_SEPARATOR.len();
    if INDENT + separators + columns * MIN_COLUMN > width {
        return false;
    }
    let mut natural = vec![1usize; columns];
    for cells in &rows {
        for (c, cell) in cells.iter().enumerate() {
            natural[c] = natural[c].max(layout::display_width(cell));
        }
    }
    // Water-fill: columns narrower than an equal share keep their natural width; the rest split
    // what remains evenly.
    let mut widths = vec![0usize; columns];
    let mut fixed = vec![false; columns];
    let mut available = width - INDENT - separators;
    let mut open = columns;
    let mut changed = true;
    while changed && open > 0 {
        changed = false;
        let share = available / open;
        for c in 0..columns {
            if !fixed[c] && natural[c] <= share {
                widths[c] = natural[c];
                fixed[c] = true;
                available -= natural[c];
                open -= 1;
                changed = true;
            }
        }
    }
    if let Some(share) = available.checked_div(open) {
        if share < MIN_COLUMN {
            return false;
        }
        let mut extra = available - share * open;
        for (width, &is_fixed) in widths.iter_mut().zip(&fixed) {
            if !is_fixed {
                *width = share + usize::from(extra > 0);
                extra = extra.saturating_sub(1);
            }
        }
    }
    // A cell narrower than MIN_COLUMN (or than wrap's 2-cell minimum) would be unreadable.
    if widths
        .iter()
        .zip(&natural)
        .any(|(&w, &n)| w < n.min(MIN_COLUMN) || (w < 2 && n > 1))
    {
        return false;
    }
    let mut wrapped_any = false;
    let mut cell_lines: Vec<Vec<Vec<&str>>> = Vec::new();
    for cells in &rows {
        let mut per: Vec<Vec<&str>> = Vec::with_capacity(columns);
        for (c, &column_width) in widths.iter().enumerate() {
            let cell = cells.get(c).map_or("", String::as_str);
            let pieces = if cell.is_empty() {
                Vec::new()
            } else {
                match layout::wrap(cell, column_width.max(2)) {
                    Ok(pieces) => pieces,
                    Err(_) => return false,
                }
            };
            if pieces.len() > 1 {
                wrapped_any = true;
            }
            per.push(pieces);
        }
        cell_lines.push(per);
    }
    let table_width = separators + widths.iter().sum::<usize>();
    for (r, per) in cell_lines.iter().enumerate() {
        // Wrapped tables get a rule between rows so cells stay grouped.
        if r > 0 && wrapped_any {
            let rule_line = format!("{}{}", " ".repeat(INDENT), "─".repeat(table_width));
            lines.push(Line::plain(start + r, &rule_line, Style::Page));
        }
        let height = per.iter().map(Vec::len).max().unwrap_or(0).max(1);
        for line_index in 0..height {
            let mut text = " ".repeat(INDENT);
            for (c, cell) in per.iter().enumerate() {
                if c > 0 {
                    text.push_str(CELL_SEPARATOR);
                }
                let piece = cell.get(line_index).copied().unwrap_or("");
                text.push_str(piece);
                if c + 1 < columns {
                    text.push_str(
                        &" ".repeat(widths[c].saturating_sub(layout::display_width(piece))),
                    );
                }
            }
            lines.push(Line::plain(
                start + r,
                text.trim_end_matches(' '),
                Style::Body,
            ));
        }
    }
    true
}

/// Title pages, dedications and contents lists often use one `<p>` per line. In runs of 3+ short
/// paragraphs, true marks the blocks that follow another in the run: they are drawn without the
/// blank spacer line.
fn stacked_lines(blocks: &[Block]) -> Vec<bool> {
    const SHORT_LIMIT: usize = 48;
    let mut stacked = vec![false; blocks.len()];
    let mut start = 0;
    let mut index = 0;
    while index <= blocks.len() {
        let short = index < blocks.len()
            && blocks[index].kind == Kind::Paragraph
            && blocks[index].verse == 0
            && !blocks[index].text.contains('\n')
            && layout::display_width(&blocks[index].text) <= SHORT_LIMIT
            && (index == start || blocks[index].page.is_empty());
        if !short {
            if index - start >= 3 {
                for flag in &mut stacked[start + 1..index] {
                    *flag = true;
                }
            }
            start = index + 1;
        }
        index += 1;
    }
    stacked
}

/// Every block of the document containing the query, as search hits. `Hit.section` = chapter
/// index, `Hit.number` = block index (+1).
pub fn find(doc: &Document, query: &Query) -> (Vec<Hit>, usize) {
    let mut hits = Vec::new();
    let mut occurrences = 0;
    for (c, chapter) in doc.chapters.iter().enumerate() {
        for (b, block) in chapter.blocks.iter().enumerate() {
            let Some(first) = search::next(&block.text, query, 0) else {
                continue;
            };
            let count = search::count(&block.text, query);
            occurrences += count;
            hits.push(Hit {
                section: c,
                chapter: c.min(usize::from(u16::MAX)) as u16,
                number: (b + 1).min(usize::from(u16::MAX)) as u16,
                label: Label {
                    chapter: 0,
                    number: block.verse.min(u32::from(u16::MAX)) as u16,
                },
                text: block.text.clone(),
                match_range: first,
                occurrences: count as u32,
            });
        }
    }
    (hits, occurrences)
}

/// Arabic psalm number from a heading "Psalm <roman>. <N>" (or "Psalm N").
pub fn psalm_number(text: &str) -> Option<u16> {
    if !text.starts_with("Psalm ") {
        return None;
    }
    text.split([' ', '.'])
        .rfind(|word| !word.is_empty())?
        .parse()
        .ok()
}

fn state_path(base: &str, file: &str) -> String {
    format!("{base}.books/{:x}.json", wyhash::hash(0, file.as_bytes()))
}

pub fn load_position(base: &str, file: &str) -> Position {
    std::fs::read(state_path(base, file))
        .ok()
        .filter(|bytes| bytes.len() <= 4096)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_position(base: &str, file: &str, position: Position) -> io::Result<()> {
    let path = state_path(base, file);
    if let Some(parent) = Path::new(&path).parent() {
        std::fs::create_dir_all(parent)?;
    }
    let temporary = format!("{path}.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec(&position).map_err(io::Error::other)?,
    )?;
    std::fs::rename(temporary, path)
}

/// Image block nearest the top of the view: on screen first, then the closest one above it in
/// this chapter.
fn image_near(blocks: &[Block], lines: &[Line], top: usize, height: usize) -> Option<String> {
    for line in lines.iter().skip(top).take(height) {
        if blocks[line.block].kind == Kind::Image {
            return Some(blocks[line.block].text.clone());
        }
    }
    let mut b = if lines.is_empty() {
        0
    } else {
        lines[top.min(lines.len() - 1)].block
    };
    loop {
        if b < blocks.len() && blocks[b].kind == Kind::Image {
            return Some(blocks[b].text.clone());
        }
        if b == 0 {
            return None;
        }
        b -= 1;
    }
}

/// Opens an asset in the desktop's image viewer, detached from the terminal.
fn open_image(path: &str) -> io::Result<()> {
    use std::process::{Command, Stdio};
    let mut command = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg(path);
        c
    } else if cfg!(windows) {
        let mut c = Command::new("cmd");
        c.args(["/c", "start", "", path]);
        c
    } else {
        let mut c = Command::new("xdg-open");
        c.arg(path);
        c
    };
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(unix)]
    std::os::unix::process::CommandExt::process_group(&mut command, 0);
    command.spawn().map(|_| ())
}

fn render_panel(
    w: &mut String,
    finder: &SearchState,
    doc: &Document,
    x: usize,
    width: usize,
    top: usize,
    bottom: usize,
) {
    let inner = width.saturating_sub(3);
    let mut row = top;
    let header = format!("Search · “{}”", finder.submitted_text());
    let _ = write!(
        w,
        "\x1b[{row};{x}H{RULE}┃ {}",
        if finder.focus {
            format!("\x1b[1m{GOLD}")
        } else {
            DIM.to_string()
        }
    );
    tui::cell(w, &header, inner);
    let _ = write!(w, "\x1b[0m{PAPER}");
    row += 1;
    let summary = format!(
        "{} passages · {} matches · {}/{}",
        finder.hits.len(),
        finder.occurrences,
        if finder.hits.is_empty() {
            0
        } else {
            finder.selected + 1
        },
        finder.hits.len()
    );
    let _ = write!(w, "\x1b[{row};{x}H{RULE}┃ {DIM}");
    tui::cell(w, &summary, inner);
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
        let _ = write!(w, "\x1b[{};{x}H{RULE}┃ ", row + offset);
        let Some(hit) = finder.hits.get(first + offset) else {
            tui::cell(w, "", inner);
            continue;
        };
        let chosen = first + offset == finder.selected;
        let chapter_title = &doc.chapters[hit.section].title;
        let marker = if chosen { "▶ " } else { "  " };
        let place = if hit.label.number > 0 {
            format!("{marker}{chapter_title} {} ", hit.label.number)
        } else {
            format!("{marker}{chapter_title} ")
        };
        let short = tui::clipped(&place, inner.min(28));
        w.push_str(if chosen { GOLD } else { DIM }.as_str());
        w.push_str(short);
        let mut used = layout::display_width(short);
        if short.len() < place.len() && used < inner {
            w.push_str("… ");
            used += 2;
        }
        w.push_str(INK.as_str());
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
        // Snippets are single-line: stop at an embedded newline.
        let rest = &hit.text[from..];
        let one_line = &rest[..rest.find('\n').unwrap_or(rest.len())];
        let visible = tui::clipped(one_line, inner.saturating_sub(used));
        tui::write_marked_styled(
            w,
            &hit.text,
            from,
            from + visible.len(),
            &finder.active,
            0,
            &format!("{PAPER}{INK}"),
        );
        used += layout::display_width(visible);
        for _ in used..inner {
            w.push(' ');
        }
    }
}

fn chapter_picker(tui: &mut Tui, doc: &Document, current: usize) -> io::Result<Option<usize>> {
    let labels: Vec<String> = doc
        .chapters
        .iter()
        .map(|chapter| format!("{} · {} blocks", chapter.title, chapter.blocks.len()))
        .collect();
    tui.choose("Choose chapter", &doc.title, &labels, current, 1)
}

const ZOOM_NAMES: [&str; 5] = [
    "¼ screen",
    "½ screen",
    "¾ screen (default)",
    "full screen",
    "full screen, small images enlarged",
];

fn flush(frame: &mut Vec<u8>, w: &mut String) {
    frame.extend_from_slice(w.as_bytes());
    w.clear();
}

/// Reads `doc` inside the application terminal until the user quits or returns to the library.
/// The position is saved on exit.
#[allow(clippy::too_many_arguments)]
pub fn run(
    tui: &mut Tui,
    doc: &Document,
    file: &str,
    base: &str,
    library_dir: &str,
    caps: Capabilities,
    start: Option<Position>,
    portion: Option<&Portion>,
    mut completion: Option<Completion<'_>>,
) -> io::Result<Exit> {
    let mut position = match portion {
        Some(p) => load_position(base, &p.key),
        None => start.unwrap_or_else(|| load_position(base, file)),
    };
    if position.chapter >= doc.chapters.len() {
        position = Position::default();
    }
    let mut finder = SearchState::default();
    let mut chapter = position.chapter;
    let mut top = 0usize;
    let mut anchor_block: Option<usize> = Some(position.block);
    let mut lines: Vec<Line> = Vec::new();
    let (mut laid_width, mut laid_chapter, mut laid_height) = (0usize, usize::MAX, 0usize);
    let mut notice = match portion {
        Some(p) => format!(
            "Plan reading: {} · one continuous buffer, read to the end-of-reading marker; c marks the day complete · m returns to the plan",
            p.label
        ),
        None => "/ search · t chapters · H/L (Shift+←/→) chapters · m library · q quit".to_string(),
    };
    let mut redraw = true;
    let (mut old_columns, mut old_rows) = (0usize, 0usize);
    let mut exit = Exit::Quit;
    let mut confirm_complete = false;
    let mut images = Renderer::new(format!("{library_dir}/{file}.assets"), caps);
    let mut zoom = 2usize;
    'reader: while !tui.is_interrupted() {
        let size = tui.screen_size()?;
        if size.columns != old_columns || size.rows != old_rows {
            old_columns = size.columns;
            old_rows = size.rows;
            redraw = true;
        }
        let (columns, height) = (size.columns, size.rows);
        if images.caps.protocol != Protocol::None
            && size.pixel_width >= columns
            && size.pixel_height >= height
        {
            let (cw, ch) = (
                (size.pixel_width / columns) as u16,
                (size.pixel_height / height) as u16,
            );
            if images.set_cell(cw, ch) {
                laid_width = 0; // re-reserve image rows
            }
        }
        let panel_width = if !finder.open {
            0
        } else if columns >= 72 {
            (columns * 2 / 5).clamp(30, 64)
        } else {
            columns
        };
        let reader_visible = panel_width < columns;
        let text_width = 100.min((columns - panel_width).saturating_sub(GUTTER_WIDTH + 3));
        let body_height = height.saturating_sub(5);
        let usable = body_height >= 3 && (!reader_visible || text_width >= 20);
        // Zoom levels (+/-): ¼, ½, ¾ (default), full body, full + enlarge.
        let image_rows = 4.max(body_height * (zoom + 1).min(4) / 4);
        images.set_enlarge(zoom == 4);
        if usable
            && reader_visible
            && (text_width != laid_width
                || chapter != laid_chapter
                || (images.enabled() && image_rows != laid_height))
        {
            let keep = anchor_block.unwrap_or(if lines.is_empty() {
                0
            } else {
                lines[top.min(lines.len() - 1)].block
            });
            let stop = portion.map(|_| doc.chapters[chapter].blocks.len());
            let enabled = images.enabled();
            lines = render_with(
                &doc.chapters[chapter],
                text_width,
                if enabled { Some(&mut images) } else { None },
                image_rows,
                stop,
            );
            laid_width = text_width;
            laid_height = image_rows;
            laid_chapter = chapter;
            top = lines
                .iter()
                .position(|line| line.block >= keep)
                .unwrap_or(0);
            anchor_block = None;
            redraw = true;
        }
        let max_top = lines.len().saturating_sub(body_height);
        top = top.min(max_top);
        // Apply a burst of keys (held j, fast scrolling) before redrawing: images make each frame
        // costly, and stale frames only add lag.
        if redraw && !tui.input_pending() {
            let mut frame: Vec<u8> = Vec::new();
            let mut w = String::new();
            // Synchronized update (DEC 2026): the terminal shows the frame only when complete.
            if images.caps.protocol == Protocol::Kitty {
                // Ghostty treats ED2 as "delete visible placements and unused images"; erase line
                // by line instead (EL keeps images). Placements are reset in begin_frame.
                let _ = write!(w, "\x1b[?2026h{PAPER}{INK}");
                for row in 1..=height {
                    let _ = write!(w, "\x1b[{row};1H\x1b[2K");
                }
                w.push_str("\x1b[H");
            } else {
                let _ = write!(w, "\x1b[?2026h\x1b[H\x1b[2J{PAPER}{INK}");
            }
            if !usable {
                w.push_str("Kata: enlarge the terminal. q quits.");
            } else {
                let title = format!(
                    "{} · {}  ({}/{})",
                    doc.title,
                    doc.chapters[chapter].title,
                    chapter + 1,
                    doc.chapters.len()
                );
                let _ = write!(w, "\x1b[1;1H\x1b[1m{GOLD} K A T A  \x1b[0m{PAPER}{INK}");
                w.push_str(tui::clipped(&title, columns.saturating_sub(12)));
                let _ = write!(w, "\x1b[2;1H{DIM}");
                w.push_str(tui::clipped(
                    &crate::live_ingest::global().overlay_or(&notice),
                    columns,
                ));
                let highlight = finder.highlight().map(|(_, query)| *query);
                if reader_visible {
                    for row in 0..body_height {
                        let _ = write!(w, "\x1b[{};1H", row + 3);
                        let Some(line) = lines.get(top + row) else {
                            continue;
                        };
                        w.push_str(DIM.as_str());
                        let shown: String = line.gutter.chars().take(GUTTER_WIDTH - 1).collect();
                        let _ = write!(w, "{:>width$}", shown, width = GUTTER_WIDTH - 1);
                        w.push(' ');
                        let _ = write!(w, "{RULE}│ ");
                        let color: String = match line.style {
                            Style::Heading => format!("{PAPER}\x1b[1m{GOLD}"),
                            Style::Code => format!("{PAPER}{CODE_INK}"),
                            Style::Quote | Style::Note | Style::Figure | Style::Page => {
                                format!("{PAPER}{DIM}")
                            }
                            Style::Body => format!("{PAPER}{INK}"),
                        };
                        w.push_str(&color);
                        if line.style == Style::Code {
                            w.push_str("  ");
                        }
                        let visible = tui::clipped(&line.text, text_width);
                        match &highlight {
                            Some(query) if !visible.is_empty() => {
                                tui::write_marked_styled(
                                    &mut w,
                                    &line.full,
                                    line.offset,
                                    line.offset + visible.len(),
                                    query,
                                    0,
                                    &color,
                                );
                            }
                            _ => w.push_str(visible),
                        }
                        let _ = write!(w, "\x1b[0m{PAPER}");
                    }
                }
                flush(&mut frame, &mut w);
                images.begin_frame(&mut frame)?;
                if reader_visible && images.enabled() {
                    // Paint each visible image once, from its first visible row.
                    let mut row = 0;
                    while row < body_height {
                        let at = top + row;
                        let Some(line) = lines.get(at) else { break };
                        if line.image.is_empty() {
                            row += 1;
                            continue;
                        }
                        let mut count = 0;
                        while row + count < body_height
                            && let Some(next) = lines.get(top + row + count)
                            && next.image == line.image
                            && next.block == line.block
                        {
                            count += 1;
                        }
                        let _ = write!(w, "\x1b[{};{}H", row + 3, GUTTER_WIDTH + 3);
                        flush(&mut frame, &mut w);
                        if !images.draw(
                            &mut frame,
                            &line.image,
                            text_width,
                            image_rows,
                            usize::from(line.image_row),
                            count,
                        )? {
                            let _ = write!(w, "{DIM}▣ image could not be decoded · i opens it");
                        }
                        row += count;
                    }
                }
                if finder.open {
                    render_panel(
                        &mut w,
                        &finder,
                        doc,
                        columns - panel_width + 1,
                        panel_width,
                        3,
                        height - 2,
                    );
                }
                let _ = write!(w, "\x1b[{};1H{GOLD}", height - 1);
                let help = if finder.prompt {
                    "Type a word or phrase · matches highlight as you type · Enter searches the whole book · Esc cancels"
                } else if finder.open && finder.focus {
                    "j/k select · Enter open · n/N next/previous · Tab/Esc back to reader · / new search · x close search"
                } else {
                    "j/k or ↑/↓ scroll · Ctrl-d/u page · g/G ends · H/L chapter · t chapters · / search · n/N · i image · +/- size · x close"
                };
                w.push_str(tui::clipped(help, columns));
                let _ = write!(w, "\x1b[{height};1H{DIM}");
                if finder.prompt {
                    let line = format!("/{}█   searching this book", finder.prompt_text());
                    w.push_str(INK.as_str());
                    w.push_str(tui::clipped(&line, columns));
                } else {
                    let blocks = &doc.chapters[chapter].blocks;
                    let where_ = if lines.is_empty() {
                        0
                    } else {
                        lines[top.min(lines.len() - 1)].block
                    };
                    let page = blocks[..(where_ + 1).min(blocks.len())]
                        .iter()
                        .rev()
                        .find(|b| !b.page.is_empty())
                        .map_or("", |b| b.page.as_str());
                    let status = format!(
                        "m library · q quit · line {}/{}{}{}",
                        (top + 1).min(lines.len()),
                        lines.len(),
                        if page.is_empty() { "" } else { " · page " },
                        page
                    );
                    w.push_str(tui::clipped(&status, columns));
                }
            }
            w.push_str("\x1b[?2026l");
            flush(&mut frame, &mut w);
            tui.write_bytes(&frame)?;
            redraw = false;
        }
        tui.set_text_input(finder.prompt);
        let Some(key) = tui.read_key()? else {
            redraw |= crate::live_ingest::global().changed();
            continue;
        };
        redraw = true;
        if confirm_complete {
            confirm_complete = false;
            notice = "Not marked complete.".into();
            if key == b'y'
                && let Some(c) = completion.as_mut()
            {
                match c.state.complete(c.today) {
                    Ok(()) => match state::save(Path::new(c.path), c.state) {
                        Ok(()) => notice =
                            "Completed and saved. Next assignment tomorrow; m returns to the plan."
                                .into(),
                        Err(error) => notice = format!("Could not save progress: {error}."),
                    },
                    Err(state::StateError::AlreadyCompletedToday) => {
                        notice = "Already complete today. Your next assignment becomes available tomorrow.".into();
                    }
                    Err(error) => notice = format!("Cannot complete: {error}."),
                }
            }
            continue;
        }
        if finder.prompt {
            match key {
                27 => finder.cancel(),
                10 | 13 => {
                    finder.prompt = false;
                    if finder.typing.is_empty() {
                        continue;
                    }
                    let (hits, occurrences) = find(doc, &finder.typing);
                    let typed = finder.prompt_text().to_string();
                    if hits.is_empty() {
                        notice = format!("No matches for “{typed}” in this book.");
                        finder.close();
                        continue;
                    }
                    let count = hits.len();
                    let nearest = hits.iter().position(|hit| hit.section >= chapter);
                    finder.set_results(hits, occurrences);
                    if let Some(first) = nearest {
                        finder.selected = first;
                    }
                    notice = format!(
                        "{count} passages contain “{typed}”. Enter opens a result; n/N steps through them."
                    );
                }
                127 | 8 => finder.backspace(),
                21 => finder.clear(),
                key => finder.input(key),
            }
            continue;
        }
        let mut jump = false;
        let last = finder.hits.len().saturating_sub(1);
        if finder.open && finder.focus {
            let page = (body_height / 2).max(1);
            match key {
                b'j' => finder.selected = last.min(finder.selected + 1),
                b'k' => finder.selected = finder.selected.saturating_sub(1),
                4 | b'f' => finder.selected = last.min(finder.selected + page),
                21 | b'b' => finder.selected = finder.selected.saturating_sub(page),
                b'g' => finder.selected = 0,
                b'G' => finder.selected = last,
                10 | 13 => jump = true,
                b'n' => {
                    finder.selected = last.min(finder.selected + 1);
                    jump = true;
                }
                b'N' => {
                    finder.selected = finder.selected.saturating_sub(1);
                    jump = true;
                }
                27 | 9 | b'q' | b'h' => finder.focus = false,
                b'x' => finder.close(),
                b'/' => finder.begin(0),
                _ => {}
            }
            if !jump {
                continue;
            }
        } else if finder.open && (key == b'n' || key == b'N') {
            finder.selected = if key == b'n' {
                last.min(finder.selected + 1)
            } else {
                finder.selected.saturating_sub(1)
            };
            jump = true;
        }
        if jump {
            let Some(hit) = finder.current() else {
                continue;
            };
            chapter = hit.section;
            anchor_block = Some(usize::from(hit.number).saturating_sub(1));
            laid_chapter = usize::MAX;
            continue;
        }
        let half = (body_height / 2).max(1);
        match key {
            b'q' | 3 => break 'reader,
            b'm' => {
                exit = Exit::Home;
                break 'reader;
            }
            b'j' => top = max_top.min(top + 1),
            b'k' => top = top.saturating_sub(1),
            4 | b'f' | b' ' => top = max_top.min(top + half),
            21 | b'b' => top = top.saturating_sub(half),
            b'g' => top = 0,
            b'G' => top = max_top,
            b'L' => {
                if chapter + 1 < doc.chapters.len() {
                    chapter += 1;
                    anchor_block = Some(0);
                } else {
                    notice = if portion.is_some() {
                        "End of the day's reading is at the bottom.".into()
                    } else {
                        "Last chapter. H goes back; t lists chapters.".into()
                    };
                }
            }
            b'H' => {
                if chapter > 0 {
                    chapter -= 1;
                    anchor_block = Some(0);
                } else {
                    notice = if portion.is_some() {
                        "Start of the day's reading.".into()
                    } else {
                        "First chapter. L goes forward; t lists chapters.".into()
                    };
                }
            }
            b't' | b'o' => {
                if let Some(picked) = chapter_picker(tui, doc, chapter)? {
                    chapter = picked;
                    anchor_block = Some(0);
                    laid_chapter = usize::MAX;
                }
            }
            b'c' => {
                if completion.is_some() {
                    confirm_complete = true;
                    notice =
                        "Mark today's reading complete? y confirms · any other key cancels".into();
                }
            }
            b'/' => finder.begin(0),
            b'r' => {
                if finder.open {
                    finder.focus = true;
                }
            }
            b'i' => {
                notice = match image_near(&doc.chapters[chapter].blocks, &lines, top, body_height) {
                    Some(name) => {
                        let path = format!("{library_dir}/{file}.assets/{name}");
                        if Path::new(&path).exists() {
                            match open_image(&path) {
                                Ok(()) => format!("Opened {name} in your image viewer."),
                                Err(error) => {
                                    format!("Cannot open image viewer: {error}. File: {path}")
                                }
                            }
                        } else {
                            "Image file missing; run `kata ingest --force` to restore assets."
                                .into()
                        }
                    }
                    None => "No image on screen or above it in this chapter.".into(),
                };
            }
            b'x' => {
                if finder.open {
                    finder.close();
                    notice = "Search closed.".into();
                }
            }
            b'+' | b'=' | b'-' | b'_' => {
                let grow = key == b'+' || key == b'=';
                if grow && zoom < 4 {
                    zoom += 1;
                } else if !grow && zoom > 0 {
                    zoom -= 1;
                }
                notice = if images.enabled() {
                    format!("Images: up to {}. +/- change size.", ZOOM_NAMES[zoom])
                } else {
                    "Inline images are off in this terminal; i opens them.".into()
                };
                anchor_block = if lines.is_empty() {
                    None
                } else {
                    Some(lines[top.min(lines.len() - 1)].block)
                };
            }
            _ => {}
        }
    }
    if images.caps.protocol == Protocol::Kitty {
        // Kitty keeps image data in the terminal; free it when leaving the book.
        let _ = tui.write("\x1b_Ga=d,d=A,q=2\x1b\\");
    }
    let block = if lines.is_empty() {
        0
    } else {
        lines[top.min(lines.len() - 1)].block
    };
    let saved = Position {
        version: 1,
        chapter,
        block,
    };
    // A plan day is its own buffer: its place is saved per plan, day and item, so free reading
    // of the same book keeps its own position.
    let _ = match portion {
        Some(p) => save_position(base, &p.key, saved),
        None => save_position(base, file, saved),
    };
    let _ = io::stdout().flush();
    Ok(exit)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: Kind, text: &str) -> Block {
        Block::new(kind, text)
    }

    fn chapter(title: &str, blocks: Vec<Block>) -> Chapter {
        Chapter {
            title: title.into(),
            blocks,
        }
    }

    #[test]
    fn chapters_render_with_verse_gutters_page_rules_and_intact_code() {
        let mut heading = block(Kind::Heading, "Psalm I. 1");
        heading.level = 2;
        let mut one = block(
            Kind::Verse,
            "Blessed is the man that hath not walked in the counsel of the ungodly",
        );
        one.verse = 1;
        let mut two = block(Kind::Verse, "But his will");
        two.verse = 2;
        two.page = "26".into();
        let code = block(Kind::Code, "fn x() {\n    y();\n}");
        let lines = render(&chapter("One", vec![heading, one, two, code]), 30);
        assert_eq!(lines.iter().filter(|l| !l.gutter.is_empty()).count(), 2);
        assert!(
            lines
                .iter()
                .any(|l| l.style == Style::Page && l.text.contains("page 26"))
        );
        assert!(
            lines
                .iter()
                .any(|l| l.style == Style::Code && l.text == "    y();")
        );
        assert!(lines.iter().all(|l| layout::display_width(&l.text) <= 30));
    }

    #[test]
    fn table_rows_render_as_aligned_columns_that_wrap_within_the_width() {
        let short = chapter(
            "T",
            vec![
                block(Kind::TableRow, "Day │ Matins │ Vespers"),
                block(Kind::TableRow, "Su │ II, III │"),
                block(Kind::TableRow, "M │ IV, V │ VI"),
            ],
        );
        let grid = render(&short, 60);
        assert_eq!(grid.len(), 3);
        assert_eq!(grid[0].text, "  Day │ Matins  │ Vespers");
        assert_eq!(grid[1].text, "  Su  │ II, III │");
        assert_eq!(grid[2].text, "  M   │ IV, V   │ VI");
        assert_eq!(grid[2].block, 2);
        let long = chapter(
            "T",
            vec![
                block(Kind::TableRow, "Concept │ What you might model"),
                block(
                    Kind::TableRow,
                    "What is the goal? │ Define too slow. How fast should the information be received and why?",
                ),
            ],
        );
        let wrapped = render(&long, 40);
        assert!(wrapped.iter().all(|l| layout::display_width(&l.text) <= 40));
        assert_eq!(wrapped.iter().filter(|l| l.style == Style::Page).count(), 1);
        assert!(wrapped.len() > 4);
        let bar = wrapped[0].text.find('│').unwrap();
        for line in wrapped.iter().filter(|l| l.style != Style::Page) {
            assert_eq!(line.text.find('│'), Some(bar));
        }
    }

    #[test]
    fn a_plan_day_joins_its_units_into_one_buffer_ending_in_the_marker() {
        let doc = Document {
            title: "T".into(),
            chapters: vec![
                chapter(
                    "A",
                    vec![
                        block(Kind::Paragraph, "a0"),
                        block(Kind::Paragraph, "a1"),
                        block(Kind::Paragraph, "a2"),
                    ],
                ),
                chapter(
                    "B",
                    vec![block(Kind::Paragraph, "b0"), block(Kind::Paragraph, "b1")],
                ),
            ],
            ..Document::default()
        };
        let portion = Portion {
            ranges: vec![(0, 1, 3), (1, 0, 1)],
            label: "day".into(),
            key: "k".into(),
        };
        let day = portion_document(&doc, &portion);
        assert_eq!(day.chapters.len(), 1);
        let texts: Vec<&str> = day.chapters[0]
            .blocks
            .iter()
            .map(|b| b.text.as_str())
            .collect();
        assert_eq!(texts, ["a1", "a2", "b0"]);
        let lines = render_with(&day.chapters[0], 60, None, 0, Some(3));
        assert_eq!(lines.last().map(|l| l.style), Some(Style::Body));
        assert!(lines.iter().any(|l| l.text == STOP_TEXT));
        let marker = lines.iter().position(|l| l.text == STOP_TEXT).unwrap();
        assert!(lines[..marker].iter().any(|l| l.text == "b0"));
        assert!(lines[marker..].iter().all(|l| l.text != "b1"));
    }

    #[test]
    fn psalm_headings_give_their_arabic_number() {
        assert_eq!(psalm_number("Psalm III. 3"), Some(3));
        assert_eq!(psalm_number("Psalm CXVIII. 118"), Some(118));
        assert_eq!(psalm_number("Foreword"), None);
    }

    #[test]
    fn runs_of_short_paragraphs_stack_without_spacer_lines() {
        let blocks = vec![
            block(Kind::Paragraph, "Second Edition"),
            block(Kind::Paragraph, "Designing Fine-Grained Systems"),
            block(Kind::Paragraph, "Sam Newman"),
            block(
                Kind::Paragraph,
                "A longer paragraph that is ordinary prose and keeps its spacing above it.",
            ),
            block(Kind::Paragraph, "Short one"),
            block(Kind::Paragraph, "Short two"),
        ];
        let texts: Vec<String> = render(&chapter("Title", blocks), 100)
            .into_iter()
            .map(|l| l.text)
            .collect();
        assert_eq!(
            texts,
            [
                "Second Edition",
                "Designing Fine-Grained Systems",
                "Sam Newman",
                "",
                "A longer paragraph that is ordinary prose and keeps its spacing above it.",
                "",
                "Short one",
                "",
                "Short two"
            ]
        );
    }

    #[test]
    fn document_search_returns_chapter_and_block_locations() {
        let mut verse = block(Kind::Verse, "Mercy and mercy");
        verse.verse = 7;
        let doc = Document {
            title: "T".into(),
            chapters: vec![
                chapter("A", vec![block(Kind::Paragraph, "nothing"), verse]),
                chapter("B", vec![block(Kind::Paragraph, "merciful")]),
            ],
            ..Document::default()
        };
        let (hits, occurrences) = find(&doc, &Query::new("merc"));
        assert_eq!((hits.len(), occurrences), (2, 3));
        assert_eq!(
            (hits[0].number, hits[0].label.number, hits[1].section),
            (2, 7, 1)
        );
    }

    #[test]
    fn wrapped_lines_keep_their_offset_into_the_block_text_for_highlighting() {
        let long = block(
            Kind::Paragraph,
            "alpha beta gamma delta epsilon zeta eta theta",
        );
        let lines = render(&chapter("C", vec![long]), 16);
        assert!(lines.len() > 2);
        for line in &lines {
            assert_eq!(
                &line.full[line.offset..line.offset + line.text.len()],
                line.text
            );
        }
    }

    #[test]
    fn position_files_round_trip_and_default_when_missing() {
        let dir = std::env::temp_dir().join(format!("kata-bookpos-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let base = dir.join("state.json").to_string_lossy().into_owned();
        assert_eq!(load_position(&base, "a"), Position::default());
        save_position(
            &base,
            "a",
            Position {
                version: 1,
                chapter: 3,
                block: 9,
            },
        )
        .unwrap();
        assert_eq!(load_position(&base, "a").chapter, 3);
        assert_eq!(
            std::fs::read_to_string(state_path(&base, "a")).unwrap(),
            r#"{"version":1,"chapter":3,"block":9}"#
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn every_sample_book_chapter_lays_out_at_narrow_and_wide_widths() {
        let dir = format!("{}/../../../examplePubs", env!("CARGO_MANIFEST_DIR"));
        let Ok(read) = std::fs::read_dir(dir) else {
            return;
        }; // samples are local, not in git
        let mut books = 0;
        for entry in read.flatten() {
            if !entry.file_name().to_string_lossy().ends_with(".epub") {
                continue;
            }
            let converted =
                kata_core::convert::convert(std::fs::read(entry.path()).unwrap(), "x.epub")
                    .unwrap();
            for chapter in &converted.doc.chapters {
                for width in [20, 37, 64, 100] {
                    for line in render(chapter, width) {
                        assert!(
                            layout::display_width(&line.text) <= width + 2,
                            "{} @ {width}",
                            chapter.title
                        );
                    }
                }
            }
            books += 1;
        }
        assert!(books == 0 || books >= 2);
    }
}
