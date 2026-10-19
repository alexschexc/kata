//! Verse alignment across panes, text wrapping and line rendering, ported from `src/layout.zig`.
//!
//! Width comes from [`crate::width`] on every platform. The Zig build used libc `wcwidth` on
//! POSIX and its own table on Windows; the Rust port is deliberately uniform.

use crate::source::{Label, Verse};
use crate::width::width;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub book: String,
    pub chapter: u16,
    pub number: u16,
    pub texts: [Option<String>; 3],
    /// A pane's own chapter:verse when it differs from the row position (KJV Psalms follow the
    /// Greek/Latin psalm order but keep KJV labels).
    pub labels: [Option<Label>; 3],
}

impl Row {
    pub fn new(book: &str, chapter: u16, number: u16) -> Row {
        Row {
            book: book.to_string(),
            chapter,
            number,
            texts: [None, None, None],
            labels: [None; 3],
        }
    }

    pub fn label(&self, pane: usize) -> Label {
        self.labels[pane].unwrap_or(Label {
            chapter: self.chapter,
            number: self.number,
        })
    }
}

/// Shown in a pane that has no text under this verse label.
pub const ABSENT: &str = "[not present under this verse label]";

/// `cells` are the wrapped lines of `texts`, the full "c:v text" per pane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub cells: [String; 3],
    pub row: usize,
    pub texts: [String; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayoutError {
    DuplicateAlignedVerse,
    TooNarrow,
}

impl std::fmt::Display for LayoutError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for LayoutError {}

pub fn align_verses(streams: &[Vec<Verse>; 3]) -> Result<Vec<Row>, LayoutError> {
    let mut rows: Vec<Row> = Vec::new();
    for (pane, verses) in streams.iter().enumerate() {
        for verse in verses {
            let found = rows.iter().position(|r| {
                r.book == verse.book && r.chapter == verse.chapter && r.number == verse.number
            });
            let index = found.unwrap_or_else(|| {
                rows.push(Row::new(&verse.book, verse.chapter, verse.number));
                rows.len() - 1
            });
            if rows[index].texts[pane].is_some() {
                return Err(LayoutError::DuplicateAlignedVerse);
            }
            rows[index].texts[pane] = Some(verse.text.clone());
            rows[index].labels[pane] = verse.label;
        }
    }
    // Each call assembles a single book/passage. Session assembly preserves passage order.
    rows.sort_by_key(|row| (row.chapter, row.number));
    Ok(rows)
}

pub fn display_width(text: &str) -> usize {
    crate::width::str_width(text)
}

/// Greedy word wrap into at most `max_width` cells per line; breaks on spaces when possible.
pub fn wrap(text: &str, max_width: usize) -> Result<Vec<&str>, LayoutError> {
    if max_width < 2 {
        return Err(LayoutError::TooNarrow);
    }
    let mut lines: Vec<&str> = Vec::new();
    let mut start = 0;
    while start < text.len() {
        let mut end = start;
        let mut used = 0;
        let mut last_space: Option<usize> = None;
        for (offset, ch) in text[start..].char_indices() {
            let cells = width(ch as u32);
            if used + cells > max_width && start + offset > start {
                break;
            }
            if ch == ' ' {
                last_space = Some(start + offset);
            }
            used += cells;
            end = start + offset + ch.len_utf8();
        }
        let mut next = end;
        if end < text.len()
            && let Some(space) = last_space
            && space > start
        {
            end = space;
            next = space + 1;
        }
        lines.push(&text[start..end]);
        start = next;
        while text.as_bytes().get(start) == Some(&b' ') {
            start += 1;
        }
    }
    if lines.is_empty() {
        lines.push("");
    }
    Ok(lines)
}

pub fn render_lines(
    rows: &[Row],
    max_width: usize,
    enabled: [bool; 3],
) -> Result<Vec<Line>, LayoutError> {
    let mut output = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let mut texts: [String; 3] = Default::default();
        let mut wrapped: Vec<Vec<String>> = Vec::with_capacity(3);
        let mut height = 1;
        for pane in 0..3 {
            let own = row.label(pane);
            texts[pane] = format!(
                "{}:{} {}",
                own.chapter,
                own.number,
                row.texts[pane].as_deref().unwrap_or(ABSENT)
            );
            let lines: Vec<String> = wrap(&texts[pane], max_width)?
                .into_iter()
                .map(str::to_string)
                .collect();
            if enabled[pane] {
                height = height.max(lines.len());
            }
            wrapped.push(lines);
        }
        for line_index in 0..height {
            let mut cells: [String; 3] = Default::default();
            for pane in 0..3 {
                if let Some(cell) = wrapped[pane].get(line_index) {
                    cells[pane] = cell.clone();
                }
            }
            output.push(Line {
                cells,
                row: index,
                texts: texts.clone(),
            });
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn verse(number: u16, text: &str) -> Verse {
        Verse {
            book: "John".into(),
            chapter: 1,
            number,
            text: text.into(),
            label: None,
        }
    }

    #[test]
    fn alignment_uses_verse_labels_and_leaves_a_missing_cell() {
        let a = vec![verse(1, "first"), verse(3, "third")];
        let b = vec![verse(3, "τρίτος")];
        let rows = align_verses(&[a.clone(), b, a]).unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows[0].texts[1].is_none());
        assert_eq!(rows[1].texts[1].as_deref(), Some("τρίτος"));
    }

    #[test]
    fn duplicate_aligned_verses_are_rejected() {
        let a = vec![verse(1, "first"), verse(1, "again")];
        assert_eq!(
            align_verses(&[a, vec![], vec![]]),
            Err(LayoutError::DuplicateAlignedVerse)
        );
    }

    #[test]
    fn panes_print_their_own_verse_labels() {
        let row = Row {
            texts: [Some("kjv".into()), Some("grb".into()), Some("vul".into())],
            labels: [
                Some(Label {
                    chapter: 23,
                    number: 1,
                }),
                None,
                None,
            ],
            ..Row::new("Psalms", 22, 1)
        };
        let lines = render_lines(&[row], 40, [true; 3]).unwrap();
        assert_eq!(lines[0].cells[0], "23:1 kjv");
        assert_eq!(lines[0].cells[1], "22:1 grb");
    }

    #[test]
    fn wrapping_respects_utf8_boundaries_and_combining_marks() {
        let lines = wrap("Ἐν ἀρχῇ λόγος", 5).unwrap();
        assert!(lines.len() > 1);
        assert!(lines.iter().all(|l| display_width(l) <= 5));
        assert_eq!(display_width("a\u{0301}"), 1);
    }

    #[test]
    fn row_heights_are_shared_across_panes() {
        let row = Row {
            texts: [
                Some("short".into()),
                Some("a much longer translation".into()),
                Some("short".into()),
            ],
            ..Row::new("John", 1, 1)
        };
        let lines = render_lines(&[row], 10, [true; 3]).unwrap();
        assert!(lines.len() > 1);
        assert_eq!(lines[lines.len() - 1].cells[0], "");
        assert_eq!(lines[lines.len() - 1].cells[2], "");
    }

    #[test]
    fn disabled_panes_do_not_add_height() {
        let row = Row {
            texts: [
                Some("short".into()),
                Some("a much longer translation".into()),
                None,
            ],
            ..Row::new("John", 1, 1)
        };
        assert_eq!(
            render_lines(std::slice::from_ref(&row), 10, [true, false, false])
                .unwrap()
                .len(),
            1
        );
        assert!(render_lines(&[row], 10, [true, true, false]).unwrap().len() > 1);
    }

    #[test]
    fn wrap_edge_cases() {
        assert_eq!(wrap("", 10).unwrap(), [""]);
        assert_eq!(wrap("abc", 1), Err(LayoutError::TooNarrow));
        assert_eq!(wrap("aaaa bbbb", 4).unwrap(), ["aaaa", "bbbb"]);
        assert_eq!(wrap("abcdefgh", 3).unwrap(), ["abc", "def", "gh"]);
        assert_eq!(wrap("界界界", 4).unwrap(), ["界界", "界"]);
    }
}
