//! Reading-plan units for ingested books, ported from `src/reading_units.zig`. Front/back
//! matter (cover, title, copyright, dedication, contents, preface, foreword, introduction,
//! index, glossary, colophon, tiny pages) is never part of a plan.
//!
//! * Psalters (10+ "Kathisma" chapters): chapter = kathisma, section = stasis (split after
//!   each "Glory…" rubric).
//! * Books with section headings (technical books): chapter, or section = the chapter split at
//!   its top-level section headings (small sections merge with their neighbour).
//! * Books without them (fiction): chapter, or section = quarter chapter (never smaller than a
//!   quarter; short chapters stay whole).

use crate::document::{Block, Document, Kind as BlockKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Chapter,
    Section,
}

/// Blocks `[start, end)` of chapter `chapter`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unit {
    pub chapter: usize,
    pub start: usize,
    pub end: usize,
    pub label: String,
}

const MIN_BODY: usize = 2000; // chapters with less text are matter/dividers
const MIN_SECTION: usize = 1500; // smaller sections merge into a neighbour
const QUARTER_FROM: usize = 8000; // shorter chapters are not split into quarters

fn text_size(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .filter(|b| !matches!(b.kind, BlockKind::Heading | BlockKind::Image))
        .map(|b| b.text.len())
        .sum()
}

const EXACT_MATTER: [&str; 18] = [
    "cover",
    "titlepage",
    "title page",
    "copyright",
    "dedication",
    "contents",
    "preface",
    "foreword",
    "introduction",
    "acknowledgments",
    "acknowledgements",
    "index",
    "glossary",
    "bibliography",
    "colophon",
    "footnotes",
    "frontispiece",
    "epigraph",
];
const PREFIX_MATTER: [&str; 9] = [
    "praise for",
    "about the author",
    "about the technical",
    "brief contents",
    "contents in detail",
    "table of contents",
    "further resources",
    "online appendix",
    "also by",
];

pub fn is_matter(doc: &Document, index: usize) -> bool {
    let chapter = &doc.chapters[index];
    let title = chapter.title.trim_matches(' ');
    let head = &title.as_bytes()[..title.len().min(96)];
    let lower = head.to_ascii_lowercase();
    if EXACT_MATTER.iter().any(|w| lower == w.as_bytes())
        || PREFIX_MATTER
            .iter()
            .any(|w| lower.starts_with(w.as_bytes()))
    {
        return true;
    }
    // Untitled pages keep their file stem ("copy", "ded01", "bm01").
    let has_upper = title.bytes().any(|c| c.is_ascii_uppercase());
    if !title.contains(' ') && !has_upper {
        return true;
    }
    text_size(&chapter.blocks) < MIN_BODY
}

fn is_psalter(doc: &Document) -> bool {
    doc.chapters
        .iter()
        .filter(|c| c.title.contains("Kathisma"))
        .count()
        >= 10
}

/// Chapters that count for plans, in reading order.
pub fn body_chapters(doc: &Document) -> Vec<usize> {
    let psalter = is_psalter(doc);
    (0..doc.chapters.len())
        .filter(|&i| {
            if psalter {
                doc.chapters[i].title.contains("Kathisma")
            } else {
                !is_matter(doc, i)
            }
        })
        .collect()
}

/// Name of the section unit for this book (shown in the plan builder).
pub fn section_name(doc: &Document) -> &'static str {
    if is_psalter(doc) {
        return "stases";
    }
    for (i, chapter) in doc.chapters.iter().enumerate() {
        if !is_matter(doc, i) && section_level(&chapter.blocks).is_some() {
            return "sections";
        }
    }
    "quarter chapters"
}

pub fn chapter_name(doc: &Document) -> &'static str {
    if is_psalter(doc) {
        "kathismata"
    } else {
        "chapters"
    }
}

/// First block after the chapter's leading title headings.
fn lead_end(blocks: &[Block]) -> usize {
    blocks
        .iter()
        .take_while(|b| b.kind == BlockKind::Heading)
        .count()
}

/// Level of the chapter's top-level section headings, if it has any.
fn section_level(blocks: &[Block]) -> Option<u8> {
    blocks[lead_end(blocks)..]
        .iter()
        .filter(|b| b.kind == BlockKind::Heading && b.level > 0)
        .map(|b| b.level)
        .min()
}

pub fn units(doc: &Document, kind: Kind) -> Vec<Unit> {
    let mut out: Vec<Unit> = Vec::new();
    let psalter = is_psalter(doc);
    for ci in body_chapters(doc) {
        let chapter = &doc.chapters[ci];
        let blocks = &chapter.blocks[..];
        if kind == Kind::Chapter {
            out.push(Unit {
                chapter: ci,
                start: 0,
                end: blocks.len(),
                label: chapter.title.clone(),
            });
            continue;
        }
        // Cut points (block indices where a new unit starts) and names.
        let mut cuts = vec![0usize];
        let mut names = vec![chapter.title.clone()];
        if psalter {
            let mut stasis = 1;
            for (i, b) in blocks.iter().enumerate() {
                if b.kind == BlockKind::Rubric
                    && b.text.starts_with("Glory")
                    && i + 1 < blocks.len()
                {
                    stasis += 1;
                    cuts.push(i + 1);
                    names.push(format!("{} · Stasis {stasis}", chapter.title));
                }
            }
            if names.len() > 1 {
                names[0] = format!("{} · Stasis 1", chapter.title);
            }
        } else if let Some(level) = section_level(blocks) {
            let lead = lead_end(blocks);
            for (i, b) in blocks.iter().enumerate().skip(lead) {
                if b.kind == BlockKind::Heading && b.level == level {
                    cuts.push(i);
                    names.push(format!("{} › {}", chapter.title, b.text));
                }
            }
        } else if text_size(blocks) >= QUARTER_FROM {
            let total = text_size(blocks);
            let (mut acc, mut part) = (0usize, 1usize);
            for (i, b) in blocks.iter().enumerate() {
                if part < 4 && acc >= total * part / 4 && b.kind != BlockKind::Heading {
                    part += 1;
                    cuts.push(i);
                    names.push(format!("{} · part {part}/4", chapter.title));
                }
                if !matches!(b.kind, BlockKind::Heading | BlockKind::Image) {
                    acc += b.text.len();
                }
            }
            if names.len() > 1 {
                names[0] = format!("{} · part 1/4", chapter.title);
            }
        }
        // Emit, merging small sections forward (and a small tail backward).
        let first = out.len();
        let mut start = 0usize;
        let mut name = names[0].clone();
        for k in 0..cuts.len() {
            let more = k + 1 < cuts.len();
            let end = if more { cuts[k + 1] } else { blocks.len() };
            if end <= start {
                continue;
            }
            if !psalter && text_size(&blocks[start..end]) < MIN_SECTION && more {
                continue;
            }
            out.push(Unit {
                chapter: ci,
                start,
                end,
                label: name.clone(),
            });
            start = end;
            if more {
                name = names[k + 1].clone();
            }
        }
        if start < blocks.len() {
            if out.len() > first {
                out.last_mut().unwrap().end = blocks.len();
            } else {
                out.push(Unit {
                    chapter: ci,
                    start,
                    end: blocks.len(),
                    label: name,
                });
            }
        }
        if !psalter && out.len() > first + 1 {
            let last = out.last().unwrap();
            if text_size(&blocks[last.start..last.end]) < MIN_SECTION {
                let tail = out.pop().unwrap();
                out.last_mut().unwrap().end = tail.end;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::document::Chapter;

    fn paragraph(len: usize) -> Block {
        Block::new(BlockKind::Paragraph, "x".repeat(len))
    }

    #[test]
    fn chapters_without_section_headings_split_into_quarters() {
        let mut blocks = vec![Block {
            level: 1,
            ..Block::new(BlockKind::Heading, "Chapter One")
        }];
        blocks.extend((0..40).map(|_| paragraph(300)));
        let doc = Document {
            title: "Novel".into(),
            chapters: vec![
                Chapter {
                    title: "Copyright".into(),
                    blocks: vec![paragraph(3000)],
                },
                Chapter {
                    title: "Chapter One".into(),
                    blocks,
                },
            ],
            ..Document::default()
        };
        let parts = units(&doc, Kind::Section);
        assert_eq!(parts.len(), 4);
        assert_eq!(parts[0].label, "Chapter One · part 1/4");
        assert_eq!(section_name(&doc), "quarter chapters");
        assert_eq!(units(&doc, Kind::Chapter).len(), 1);
        // The quarters tile the chapter exactly.
        assert_eq!(parts[0].start, 0);
        assert!(parts.windows(2).all(|w| w[0].end == w[1].start));
        assert_eq!(parts[3].end, 41);
    }

    #[test]
    fn matter_is_recognised_by_title_and_size() {
        let doc = Document {
            chapters: vec![
                Chapter {
                    title: "Cover".into(),
                    blocks: vec![paragraph(5000)],
                },
                Chapter {
                    title: "Praise for This Book".into(),
                    blocks: vec![paragraph(5000)],
                },
                Chapter {
                    title: "ded01".into(),
                    blocks: vec![paragraph(5000)],
                },
                Chapter {
                    title: "Tiny Chapter".into(),
                    blocks: vec![paragraph(100)],
                },
                Chapter {
                    title: "A Real Chapter".into(),
                    blocks: vec![paragraph(5000)],
                },
            ],
            ..Document::default()
        };
        assert_eq!(body_chapters(&doc), [4]);
    }

    #[test]
    fn sections_split_at_top_level_headings_and_merge_small_ones() {
        let heading = |level, text: &str| Block {
            level,
            ..Block::new(BlockKind::Heading, text)
        };
        let blocks = vec![
            heading(1, "Chapter 1"),
            paragraph(2000),
            heading(2, "Big A"),
            paragraph(2000),
            heading(2, "Tiny"),
            paragraph(100),
            heading(2, "Big B"),
            paragraph(2000),
        ];
        let doc = Document {
            chapters: vec![Chapter {
                title: "Chapter 1 Title".into(),
                blocks,
            }],
            ..Document::default()
        };
        assert_eq!(section_name(&doc), "sections");
        let parts = units(&doc, Kind::Section);
        // "Tiny" is under the minimum, so it merges forward into "Big B".
        assert_eq!(
            parts.iter().map(|u| (u.start, u.end)).collect::<Vec<_>>(),
            [(0, 2), (2, 4), (4, 8)]
        );
        assert_eq!(parts[1].label, "Chapter 1 Title › Big A");
        assert_eq!(parts[2].label, "Chapter 1 Title › Tiny");
    }

    #[test]
    fn psalter_splits_into_stases_after_glory_rubrics() {
        let mut chapters = Vec::new();
        for n in 1..=10 {
            let blocks = vec![
                paragraph(50),
                Block::new(BlockKind::Rubric, "Glory to the Father"),
                paragraph(50),
                Block::new(BlockKind::Rubric, "Glory. Now and ever"),
                paragraph(50),
            ];
            chapters.push(Chapter {
                title: format!("The {n}th Kathisma"),
                blocks,
            });
        }
        chapters.insert(
            0,
            Chapter {
                title: "Preface".into(),
                blocks: vec![paragraph(5000)],
            },
        );
        let doc = Document {
            chapters,
            ..Document::default()
        };
        assert_eq!(
            (section_name(&doc), chapter_name(&doc)),
            ("stases", "kathismata")
        );
        assert_eq!(body_chapters(&doc).len(), 10);
        let stases = units(&doc, Kind::Section);
        assert_eq!(stases.len(), 30);
        assert_eq!(stases[1].label, "The 1th Kathisma · Stasis 2");
        assert_eq!((stases[0].start, stases[0].end, stases[1].start), (0, 2, 2));
    }
}
