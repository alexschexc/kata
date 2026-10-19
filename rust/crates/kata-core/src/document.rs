//! Converted-document model and its on-disk format (one file per book in the library folder),
//! ported from `src/document.zig`. Line-oriented and tab-separated like the embedded Bible
//! data, so documents stay greppable and diffable:
//!
//! ```text
//! KATA-DOC<TAB>1
//! title<TAB>…   author<TAB>…   source<TAB>file.epub
//! converter<TAB>N
//! chapter<TAB><title>
//! <kind><TAB><level><TAB><verse><TAB><page><TAB><text>
//! ```
//!
//! Text escapes: `\\`, `\n`, `\t`. `verse` is 0 when absent; `page` is written only when it
//! changes and carries forward when empty.

use std::fmt::Write as _;

pub const MAGIC: &str = "KATA-DOC";
pub const FORMAT_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Heading,
    Paragraph,
    Verse,
    Superscription,
    Rubric,
    Quote,
    Code,
    ListItem,
    Term,
    TableRow,
    Figure,
    Footnote,
    /// Text is an asset file name inside `<document>.assets/`.
    Image,
}

impl Kind {
    const ALL: [(Kind, &'static str); 13] = [
        (Kind::Heading, "heading"),
        (Kind::Paragraph, "paragraph"),
        (Kind::Verse, "verse"),
        (Kind::Superscription, "superscription"),
        (Kind::Rubric, "rubric"),
        (Kind::Quote, "quote"),
        (Kind::Code, "code"),
        (Kind::ListItem, "list_item"),
        (Kind::Term, "term"),
        (Kind::TableRow, "table_row"),
        (Kind::Figure, "figure"),
        (Kind::Footnote, "footnote"),
        (Kind::Image, "image"),
    ];

    pub fn name(self) -> &'static str {
        Kind::ALL
            .iter()
            .find(|(k, _)| *k == self)
            .map(|(_, n)| *n)
            .unwrap()
    }

    pub fn from_name(name: &str) -> Option<Kind> {
        Kind::ALL.iter().find(|(_, n)| *n == name).map(|(k, _)| *k)
    }

    /// Separated from the previous block by a blank line when rendered.
    pub fn spaced(self) -> bool {
        !matches!(
            self,
            Kind::Verse | Kind::ListItem | Kind::TableRow | Kind::Footnote
        )
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub kind: Kind,
    /// Heading level 1–6; 0 otherwise.
    pub level: u8,
    /// Verse number (explicit or inferred); 0 when absent.
    pub verse: u32,
    /// Print page label in effect where the block starts ("" if unknown).
    pub page: String,
    pub text: String,
}

impl Block {
    pub fn new(kind: Kind, text: impl Into<String>) -> Block {
        Block {
            kind,
            level: 0,
            verse: 0,
            page: String::new(),
            text: text.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chapter {
    pub title: String,
    pub blocks: Vec<Block>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    pub title: String,
    pub author: String,
    pub source: String,
    pub converter: u32,
    pub chapters: Vec<Chapter>,
}

impl Document {
    pub fn block_count(&self) -> usize {
        self.chapters.iter().map(|c| c.blocks.len()).sum()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidDocument;

impl std::fmt::Display for InvalidDocument {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "InvalidDocument")
    }
}

impl std::error::Error for InvalidDocument {}

fn escape(out: &mut String, text: &str) {
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\t' => out.push_str("\\t"),
            '\r' => {}
            c => out.push(c),
        }
    }
}

fn unescape(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_string();
    }
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        match chars.next() {
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some(other) => out.push(other),
            None => out.push('\\'), // a trailing lone backslash is kept
        }
    }
    out
}

pub fn write(doc: &Document) -> String {
    let mut out = String::new();
    let _ = writeln!(out, "{MAGIC}\t{FORMAT_VERSION}");
    for (field, value) in [
        ("title", &doc.title),
        ("author", &doc.author),
        ("source", &doc.source),
    ] {
        out.push_str(field);
        out.push('\t');
        escape(&mut out, value);
        out.push('\n');
    }
    let _ = writeln!(out, "converter\t{}", doc.converter);
    let mut page = "";
    for chapter in &doc.chapters {
        out.push_str("chapter\t");
        escape(&mut out, &chapter.title);
        out.push('\n');
        for block in &chapter.blocks {
            let _ = write!(
                out,
                "{}\t{}\t{}\t",
                block.kind.name(),
                block.level,
                block.verse
            );
            if block.page != page {
                escape(&mut out, &block.page);
                page = &block.page;
            }
            out.push('\t');
            escape(&mut out, &block.text);
            out.push('\n');
        }
    }
    out
}

/// Parses a document file.
pub fn parse(bytes: &[u8]) -> Result<Document, InvalidDocument> {
    let text = std::str::from_utf8(bytes).map_err(|_| InvalidDocument)?;
    let mut lines = text.split('\n');
    if lines.next() != Some("KATA-DOC\t1") {
        return Err(InvalidDocument);
    }
    let mut doc = Document::default();
    let mut blocks: Vec<Block> = Vec::new();
    let mut chapter_title: Option<String> = None;
    let mut page = String::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        let key = line.split('\t').next().unwrap_or("");
        let rest = line.split_once('\t').map_or("", |(_, rest)| rest);
        match key {
            "chapter" => {
                if let Some(title) = chapter_title.take() {
                    doc.chapters.push(Chapter {
                        title,
                        blocks: std::mem::take(&mut blocks),
                    });
                }
                chapter_title = Some(unescape(rest));
            }
            "title" => doc.title = unescape(rest),
            "author" => doc.author = unescape(rest),
            "source" => doc.source = unescape(rest),
            "converter" => doc.converter = rest.parse().map_err(|_| InvalidDocument)?,
            _ => {
                let kind = Kind::from_name(key).ok_or(InvalidDocument)?;
                if chapter_title.is_none() {
                    return Err(InvalidDocument);
                }
                let mut fields = rest.splitn(4, '\t');
                let level = fields
                    .next()
                    .and_then(|f| f.parse::<u8>().ok())
                    .ok_or(InvalidDocument)?;
                let verse = fields
                    .next()
                    .and_then(|f| f.parse::<u32>().ok())
                    .ok_or(InvalidDocument)?;
                let page_field = fields.next().ok_or(InvalidDocument)?;
                if !page_field.is_empty() {
                    page = unescape(page_field);
                }
                let text = unescape(fields.next().unwrap_or(""));
                blocks.push(Block {
                    kind,
                    level,
                    verse,
                    page: page.clone(),
                    text,
                });
            }
        }
    }
    if let Some(title) = chapter_title {
        doc.chapters.push(Chapter { title, blocks });
    }
    if doc.chapters.is_empty() {
        return Err(InvalidDocument);
    }
    Ok(doc)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(kind: Kind, level: u8, verse: u32, page: &str, text: &str) -> Block {
        Block {
            kind,
            level,
            verse,
            page: page.into(),
            text: text.into(),
        }
    }

    #[test]
    fn documents_round_trip_with_escapes_and_carried_page_labels() {
        let doc = Document {
            title: "T\tx".into(),
            author: "A".into(),
            source: "t.epub".into(),
            converter: 3,
            chapters: vec![
                Chapter {
                    title: "One".into(),
                    blocks: vec![
                        block(Kind::Heading, 1, 0, "", "One"),
                        block(Kind::Verse, 0, 1, "26", "a \\ b"),
                        block(Kind::Code, 0, 0, "26", "fn x() {\n\treturn;\n}"),
                    ],
                },
                Chapter {
                    title: "Two".into(),
                    blocks: vec![block(Kind::Paragraph, 0, 0, "27", "p")],
                },
            ],
        };
        let written = write(&doc);
        assert_eq!(written.matches("\t26\t").count(), 1);
        let back = parse(written.as_bytes()).unwrap();
        assert_eq!(back, doc);
        assert_eq!(back.chapters[0].blocks[2].page, "26");
        assert_eq!(back.chapters[1].blocks[0].page, "27");
        assert_eq!(parse(b"nope\n"), Err(InvalidDocument));
    }

    #[test]
    fn rejects_malformed_documents() {
        let header = "KATA-DOC\t1\n";
        assert_eq!(parse(header.as_bytes()), Err(InvalidDocument)); // no chapters
        assert_eq!(
            parse(format!("{header}paragraph\t0\t0\t\tx\n").as_bytes()),
            Err(InvalidDocument)
        ); // before a chapter
        assert_eq!(
            parse(format!("{header}chapter\tA\nbogus\t0\t0\t\tx\n").as_bytes()),
            Err(InvalidDocument)
        );
        assert_eq!(
            parse(format!("{header}chapter\tA\nparagraph\t0\n").as_bytes()),
            Err(InvalidDocument)
        );
        assert_eq!(parse(&[0xff, 0xfe]), Err(InvalidDocument));
    }

    #[test]
    fn spacing_rules_and_kind_names() {
        for (kind, name) in Kind::ALL {
            assert_eq!(Kind::from_name(name), Some(kind));
            assert_eq!(kind.name(), name);
        }
        assert!(!Kind::Verse.spaced() && !Kind::Footnote.spaced() && Kind::Paragraph.spaced());
    }
}
