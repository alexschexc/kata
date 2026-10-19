//! EPUB content → Kata document blocks (ingest stage 3), ported from `src/convert.zig`. Rules
//! are generic (semantic tags, EPUB/O'Reilly data-type, common class names) rather than
//! per-book: see docs/ingest-plan.md for the samples they were designed on.

use crate::document::{Block, Chapter, Document, Kind};
use crate::epub::{self, Archive, EpubError};
use crate::xhtml::{self, Tag, Token, Tokenizer};

pub const VERSION: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    pub chapters: usize,
    pub blocks: usize,
    pub verses: usize,
    /// Explicit verse numbers that disagreed with the inferred count.
    pub verse_mismatches: usize,
    pub pages: usize,
    pub figures: usize,
    pub code: usize,
    pub footnotes: usize,
    pub images: usize,
    /// Spine documents with no readable text (cover, TOC, index).
    pub skipped: usize,
}

/// An image copied out of the EPUB, stored as `<document>.assets/<name>`.
#[derive(Debug, Clone)]
pub struct Asset {
    pub name: String,
    pub bytes: Vec<u8>,
}

#[derive(Debug)]
pub struct Converted {
    pub doc: Document,
    pub stats: Stats,
    pub assets: Vec<Asset>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConvertError {
    Epub(EpubError),
    NoReadableText,
}

impl std::fmt::Display for ConvertError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConvertError::Epub(e) => write!(f, "{e}"),
            ConvertError::NoReadableText => write!(f, "NoReadableText"),
        }
    }
}

impl std::error::Error for ConvertError {}

impl From<EpubError> for ConvertError {
    fn from(e: EpubError) -> Self {
        ConvertError::Epub(e)
    }
}

/// Zip member path for `href` relative to content document `member`.
fn resolve_member(member: &str, href: &str) -> Option<String> {
    if href.contains("://") || href.starts_with("data:") {
        return None;
    }
    let clean = &href[..href.find(['#', '?']).unwrap_or(href.len())];
    let mut parts: Vec<&str> = Vec::new();
    if !clean.starts_with('/')
        && let Some(slash) = member.rfind('/')
    {
        parts.extend(member[..slash].split('/').filter(|p| !p.is_empty()));
    }
    for part in clean.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

fn eq(a: &[u8], b: &str) -> bool {
    a.eq_ignore_ascii_case(b.as_bytes())
}

/// Whole subtrees that carry no reading text in a terminal.
fn skipped(tag: &Tag) -> bool {
    let name = tag.local();
    if [
        "head", "script", "style", "svg", "math", "nav", "template", "noscript",
    ]
    .iter()
    .any(|s| eq(name, s))
    {
        return true;
    }
    for attr in ["data-type", "epub:type"] {
        for value in ["index", "toc", "landmarks", "page-list"] {
            if tag.has_token(attr, value) {
                return true;
            }
        }
    }
    tag.has_token("class", "ornament")
        || tag.has_token("class", "tailpiece-art")
        || tag.has_token("data-type", "indexterm")
}

fn is_page_break(tag: &Tag) -> bool {
    tag.has_token("epub:type", "pagebreak") || tag.has_token("role", "doc-pagebreak")
}

const CONTAINERS: [&str; 18] = [
    "body",
    "div",
    "section",
    "article",
    "main",
    "header",
    "footer",
    "aside",
    "blockquote",
    "ul",
    "ol",
    "dl",
    "table",
    "thead",
    "tbody",
    "tfoot",
    "figure",
    "hgroup",
];

fn is_container(name: &[u8]) -> bool {
    CONTAINERS.iter().any(|c| eq(name, c))
}

fn quote_container(tag: &Tag) -> bool {
    let name = tag.local();
    if eq(name, "blockquote") {
        return true;
    }
    if eq(name, "aside") && !tag.has_token("epub:type", "footnote") {
        return true;
    }
    [
        "sidebar",
        "note",
        "tip",
        "warning",
        "caution",
        "important",
        "epigraph",
    ]
    .iter()
    .any(|kind| tag.has_token("data-type", kind))
}

const LITURGICAL: [&str; 6] = [
    "rubric",
    "stasis",
    "refrain",
    "response",
    "chant",
    "liturgical",
];

#[derive(Clone)]
struct Frame {
    name: Vec<u8>,
    /// This element opened a block.
    block: bool,
    kind: Kind,
    level: u8,
    verse: u32,
    container: bool,
    quote: bool,
    pre: bool,
    hide: bool,
    number: bool,
    sup: bool,
}

impl Frame {
    fn new(name: &[u8]) -> Frame {
        Frame {
            name: name.to_vec(),
            block: false,
            kind: Kind::Paragraph,
            level: 0,
            verse: 0,
            container: false,
            quote: false,
            pre: false,
            hide: false,
            number: false,
            sup: false,
        }
    }
}

#[derive(Default)]
pub struct Context<'a> {
    pub member: &'a str,
    pub archive: Option<&'a Archive>,
    pub images: Option<&'a mut Vec<String>>,
}

struct Builder<'a, 'b> {
    stats: &'a mut Stats,
    blocks: Vec<Block>,
    frames: Vec<Frame>,
    buf: Vec<u8>,
    number_buf: Vec<u8>,
    figure_alt: String,
    page: &'a mut String,
    buf_page: String,
    quote: usize,
    pre: usize,
    hide: usize,
    number: usize,
    verse_counter: u32,
    member: &'a str,
    archive: Option<&'a Archive>,
    images: Option<&'b mut Vec<String>>,
}

fn trim_set<'a>(s: &'a [u8], set: &[u8]) -> &'a [u8] {
    let start = s.iter().position(|c| !set.contains(c)).unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|c| !set.contains(c))
        .map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

impl Builder<'_, '_> {
    /// Emits an image block and records the zip member to copy out.
    fn image(&mut self, src: &[u8]) {
        let Some(path) = resolve_member(self.member, &String::from_utf8_lossy(src)) else {
            return;
        };
        let Some(archive) = self.archive else { return }; // unit tests: no assets
        if !archive.find(&path) {
            return;
        }
        self.flush();
        let name = path.replace('/', "_");
        self.emit(Kind::Image, 0, 0, name);
        if let Some(list) = self.images.as_mut()
            && !list.contains(&path)
        {
            list.push(path);
        }
    }

    fn innermost_block(&self) -> Option<usize> {
        self.frames.iter().rposition(|f| f.block)
    }

    fn flush(&mut self) {
        let frame = self.innermost_block().map(|i| {
            (
                self.frames[i].kind,
                self.frames[i].level,
                self.frames[i].verse,
            )
        });
        let kind = match frame {
            Some((kind, ..)) => kind,
            None if self.quote > 0 => Kind::Quote,
            None => Kind::Paragraph,
        };
        let raw = std::mem::take(&mut self.buf);
        let body = if kind == Kind::Code {
            trim_set(&raw, b"\n")
        } else {
            trim_set(&raw, b" \n")
        };
        if body.is_empty() {
            if kind == Kind::Figure && frame.is_some() && !self.figure_alt.is_empty() {
                let text = format!("[image: {}]", self.figure_alt);
                self.emit(Kind::Figure, 0, 0, text);
                self.figure_alt.clear();
            }
            self.buf = raw;
            self.buf.clear();
            return;
        }
        let text = String::from_utf8_lossy(body).into_owned();
        let (level, verse) = frame.map_or((0, 0), |(_, level, verse)| (level, verse));
        self.emit(kind, level, verse, text);
        if kind == Kind::Figure {
            self.figure_alt.clear();
        }
        self.buf = raw;
        self.buf.clear();
    }

    fn emit(&mut self, kind: Kind, level: u8, verse: u32, text: String) {
        let page = if self.buf_page.is_empty() {
            self.page.clone()
        } else {
            std::mem::take(&mut self.buf_page)
        };
        self.buf_page.clear();
        self.blocks.push(Block {
            kind,
            level,
            verse,
            page,
            text,
        });
        match kind {
            Kind::Verse => self.stats.verses += 1,
            Kind::Figure => self.stats.figures += 1,
            Kind::Code => self.stats.code += 1,
            Kind::Footnote => self.stats.footnotes += 1,
            Kind::Image => self.stats.images += 1,
            _ => {}
        }
    }

    fn append_codepoint(&mut self, cp: u32) {
        let mut c = cp;
        if c == 0xA0 {
            c = ' ' as u32;
        }
        if c == 0xAD || c == 0xFEFF || c == 0x7F || (0x80..=0x9F).contains(&c) {
            return;
        }
        if self.number > 0 {
            if c < 0x80 && (c as u8).is_ascii_digit() {
                self.number_buf.push(c as u8);
            }
            return;
        }
        if self.hide > 0 {
            return;
        }
        if self.pre > 0 {
            if c == '\r' as u32 || (c < 0x20 && c != '\n' as u32 && c != '\t' as u32) {
                return;
            }
        } else {
            if matches!(c, 0x0A | 0x09 | 0x0D | 0x20 | 0x0C) {
                if let Some(&last) = self.buf.last()
                    && last != b' '
                    && last != b'\n'
                {
                    self.buf.push(b' ');
                }
                return;
            }
            if c < 0x20 {
                return;
            }
        }
        if self.buf.is_empty() || trim_set(&self.buf, b" \n").is_empty() {
            self.buf_page = self.page.clone();
        }
        let Some(ch) = char::from_u32(c) else { return };
        let mut bytes = [0u8; 4];
        self.buf
            .extend_from_slice(ch.encode_utf8(&mut bytes).as_bytes());
    }

    fn add_text(&mut self, raw: &[u8], decode: bool) {
        let mut i = 0;
        while i < raw.len() {
            if decode
                && raw[i] == b'&'
                && let Some((cp, len)) = xhtml::entity_at(raw, i)
            {
                self.append_codepoint(cp);
                i += len;
                continue;
            }
            // Zig's utf8ByteSequenceLength: invalid lead bytes are skipped one at a time.
            let len = match raw[i] {
                0x00..=0x7f => 1,
                0xc0..=0xdf => 2,
                0xe0..=0xef => 3,
                0xf0..=0xf7 => 4,
                _ => {
                    i += 1;
                    continue;
                }
            };
            if i + len > raw.len() {
                break;
            }
            if let Ok(s) = std::str::from_utf8(&raw[i..i + len]) {
                self.append_codepoint(s.chars().next().map_or(0xFFFD, |c| c as u32));
            }
            i += len;
        }
    }

    fn line_break(&mut self) {
        if self.hide > 0 || self.number > 0 {
            return;
        }
        // Headings render on one line: a break is just a space there.
        if let Some(i) = self.innermost_block()
            && self.frames[i].kind == Kind::Heading
        {
            return self.append_codepoint(' ' as u32);
        }
        while self.buf.last() == Some(&b' ') {
            self.buf.pop();
        }
        if !self.buf.is_empty() {
            self.buf.push(b'\n');
        }
    }

    /// Block kind (and heading level) for a block-level element, or None if it is not one.
    fn block_kind(&self, tag: &Tag) -> Option<(Kind, u8)> {
        let name = tag.local();
        if name.len() == 2
            && (name[0] == b'h' || name[0] == b'H')
            && (b'1'..=b'6').contains(&name[1])
        {
            return Some((Kind::Heading, name[1] - b'0'));
        }
        if eq(name, "pre") {
            return Some((Kind::Code, 0));
        }
        if eq(name, "li") {
            return Some((Kind::ListItem, 0));
        }
        if eq(name, "dt") {
            return Some((Kind::Term, 0));
        }
        if eq(name, "tr") {
            return Some((Kind::TableRow, 0));
        }
        if eq(name, "figure") {
            return Some((Kind::Figure, 0));
        }
        if tag.has_token("data-type", "footnote") || tag.has_token("epub:type", "footnote") {
            return Some((Kind::Footnote, 0));
        }
        let paragraph_like = ["p", "dd", "caption", "figcaption"]
            .iter()
            .any(|n| eq(name, n));
        if !paragraph_like {
            return None;
        }
        if tag.has_token("class", "verse") {
            return Some((Kind::Verse, 0));
        }
        if tag.has_token("class", "superscription") {
            return Some((Kind::Superscription, 0));
        }
        if LITURGICAL.iter().any(|class| tag.has_token("class", class)) {
            return Some((Kind::Rubric, 0));
        }
        if self.quote > 0 {
            return Some((Kind::Quote, 0));
        }
        Some((Kind::Paragraph, 0))
    }

    fn start(&mut self, tag: &Tag) {
        let name = tag.local();
        if eq(name, "br") {
            return self.line_break();
        }
        if eq(name, "img") {
            let outer = self.innermost_block().map(|i| self.frames[i].kind);
            // Figures and free-standing images; inline icons stay out.
            if (outer.is_none() || outer == Some(Kind::Figure))
                && let Some(src) = tag.attr("src")
            {
                self.image(src);
            }
            if outer == Some(Kind::Figure) {
                self.figure_alt = tag
                    .attr("alt")
                    .map(|a| String::from_utf8_lossy(a).into_owned())
                    .unwrap_or_default();
            }
            return;
        }
        if eq(name, "hr") {
            return;
        }
        let mut frame = Frame::new(name);
        if is_page_break(tag) {
            if let Some(label) = tag.attr("aria-label").or_else(|| tag.attr("title")) {
                *self.page = String::from_utf8_lossy(label).into_owned();
                self.stats.pages += 1;
            }
            frame.hide = true;
        } else if tag.has_token("class", "verse-number") {
            frame.number = true;
            self.number_buf.clear();
        } else if eq(name, "sup") {
            frame.sup = true;
            self.append_codepoint('[' as u32);
        } else if eq(name, "td") || eq(name, "th") {
            if !trim_set(&self.buf, b" ").is_empty() {
                self.add_text(" │ ".as_bytes(), false);
            }
        } else if let Some((kind, level)) = self.block_kind(tag) {
            let outer = self.innermost_block().map(|i| self.frames[i].kind);
            let inline_in_outer = match outer {
                Some(
                    Kind::ListItem | Kind::TableRow | Kind::Term | Kind::Footnote | Kind::Figure,
                ) => !matches!(
                    kind,
                    Kind::ListItem | Kind::TableRow | Kind::Code | Kind::Figure
                ),
                _ => false,
            };
            if inline_in_outer {
                self.append_codepoint(' ' as u32);
            } else {
                self.flush();
                frame.block = true;
                frame.kind = kind;
                frame.level = level;
                if kind == Kind::Heading {
                    self.verse_counter = 0;
                }
                if kind == Kind::Verse {
                    self.verse_counter += 1;
                    frame.verse = self.verse_counter;
                }
                if kind == Kind::Code {
                    frame.pre = true;
                }
                if kind == Kind::Figure {
                    self.figure_alt.clear();
                    frame.container = true;
                }
            }
        } else if is_container(name) {
            self.flush();
            frame.container = true;
            frame.quote = quote_container(tag);
        }
        if tag.self_closing {
            return;
        }
        self.quote += usize::from(frame.quote);
        self.pre += usize::from(frame.pre);
        self.hide += usize::from(frame.hide);
        self.number += usize::from(frame.number);
        self.frames.push(frame);
    }

    fn end(&mut self, raw_name: &[u8]) {
        let name = xhtml::local_name(raw_name);
        // Tolerate unclosed inner elements: pop to the nearest match.
        let Some(matched) = self
            .frames
            .iter()
            .rposition(|f| f.name.eq_ignore_ascii_case(name))
        else {
            return;
        };
        while self.frames.len() > matched {
            let frame = self.frames.last().unwrap().clone();
            if frame.block || frame.container {
                self.flush();
            }
            if frame.sup {
                if self.buf.last() == Some(&b'[') {
                    self.buf.pop();
                } else {
                    self.append_codepoint(']' as u32);
                }
            }
            if frame.number {
                self.number -= 1;
                if let Some(explicit) = std::str::from_utf8(&self.number_buf)
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok())
                    && let Some(i) = self.innermost_block()
                    && self.frames[i].kind == Kind::Verse
                    && self.frames[i].verse != explicit
                {
                    self.stats.verse_mismatches += 1;
                    self.frames[i].verse = explicit;
                    self.verse_counter = explicit;
                }
            }
            self.quote -= usize::from(frame.quote);
            self.pre -= usize::from(frame.pre);
            self.hide -= usize::from(frame.hide);
            self.frames.pop();
        }
    }
}

/// Converts one XHTML content document. Returns None when it has no text.
pub fn chapter(source: &[u8], page: &mut String, stats: &mut Stats) -> Option<Chapter> {
    chapter_in(source, page, stats, Context::default())
}

pub fn chapter_in(
    source: &[u8],
    page: &mut String,
    stats: &mut Stats,
    context: Context<'_>,
) -> Option<Chapter> {
    let mut builder = Builder {
        stats,
        blocks: Vec::new(),
        frames: Vec::new(),
        buf: Vec::new(),
        number_buf: Vec::new(),
        figure_alt: String::new(),
        page,
        buf_page: String::new(),
        quote: 0,
        pre: 0,
        hide: 0,
        number: 0,
        verse_counter: 0,
        member: context.member,
        archive: context.archive,
        images: context.images,
    };
    let mut skip_name: Option<Vec<u8>> = None;
    let mut skip_depth = 0usize;
    for token in Tokenizer::new(source) {
        if let Some(skip) = &skip_name {
            match &token {
                Token::Start(tag)
                    if !tag.self_closing && tag.local().eq_ignore_ascii_case(skip) =>
                {
                    skip_depth += 1
                }
                Token::End(name) if xhtml::local_name(name).eq_ignore_ascii_case(skip) => {
                    skip_depth -= 1;
                    if skip_depth == 0 {
                        skip_name = None;
                    }
                }
                _ => {}
            }
            continue;
        }
        match token {
            Token::Text(raw) => builder.add_text(raw, true),
            Token::Cdata(raw) => builder.add_text(raw, false),
            Token::Start(tag) => {
                if skipped(&tag) {
                    if !tag.self_closing {
                        skip_name = Some(tag.local().to_vec());
                        skip_depth = 1;
                    }
                    continue;
                }
                builder.start(&tag);
            }
            Token::End(name) => builder.end(name),
        }
    }
    builder.flush();
    if builder.blocks.is_empty() {
        return None;
    }
    let title = builder
        .blocks
        .iter()
        .find(|b| b.kind == Kind::Heading)
        .map(|b| b.text.clone())
        .unwrap_or_default();
    Some(Chapter {
        title,
        blocks: builder.blocks,
    })
}

/// Converts a whole EPUB held in memory.
pub fn convert(bytes: Vec<u8>, source_name: &str) -> Result<Converted, ConvertError> {
    let archive = Archive::new(bytes)?;
    let pkg = epub::package(&archive)?;
    let mut stats = Stats::default();
    let mut chapters: Vec<Chapter> = Vec::new();
    let mut page = String::new();
    let mut images: Vec<String> = Vec::new();
    for member in &pkg.spine {
        let content = archive.read(member)?;
        let context = Context {
            member,
            archive: Some(&archive),
            images: Some(&mut images),
        };
        if let Some(mut c) = chapter_in(&content, &mut page, &mut stats, context) {
            if c.title.is_empty() {
                let base = member.rsplit('/').next().unwrap_or(member);
                c.title = base[..base.rfind('.').unwrap_or(base.len())].to_string();
            }
            stats.blocks += c.blocks.len();
            chapters.push(c);
        } else {
            stats.skipped += 1;
        }
    }
    stats.chapters = chapters.len();
    if chapters.is_empty() {
        return Err(ConvertError::NoReadableText);
    }
    let assets = images
        .iter()
        .filter_map(|path| {
            archive.read(path).ok().map(|bytes| Asset {
                name: path.replace('/', "_"),
                bytes,
            })
        })
        .collect();
    Ok(Converted {
        assets,
        doc: Document {
            title: if pkg.title.is_empty() {
                source_name.to_string()
            } else {
                pkg.title
            },
            author: pkg.author,
            source: source_name.to_string(),
            converter: VERSION,
            chapters,
        },
        stats,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_chapter(source: &str, stats: &mut Stats) -> Chapter {
        let mut page = String::new();
        chapter(source.as_bytes(), &mut page, stats).unwrap()
    }

    #[test]
    fn verses_are_inferred_checked_against_explicit_numbers_and_reset_at_headings() {
        let mut stats = Stats::default();
        let c = test_chapter(
            r#"<body><h2>Psalm I. 1</h2><p class="superscription">David's.</p>
<p class="verse opening"><span class="dropcap">B</span>lessed is the man</p>
<p class="verse">second</p><p class="verse"><span class="verse-number">3</span>Third &amp; last</p>
<span epub:type="pagebreak" role="doc-pagebreak" aria-label="26"></span>
<h2>Psalm II. 2</h2><p class="verse">Why</p><p class="stasis">Glory.</p></body>"#,
            &mut stats,
        );
        let b = &c.blocks;
        assert_eq!(c.title, "Psalm I. 1");
        assert_eq!(b[1].kind, Kind::Superscription);
        assert_eq!(b[2].text, "Blessed is the man");
        assert_eq!(b[2].verse, 1);
        assert_eq!(b[4].text, "Third & last");
        assert_eq!(b[4].verse, 3);
        assert_eq!(stats.verse_mismatches, 0);
        assert_eq!(b[5].page, "26");
        assert_eq!(b[6].verse, 1);
        assert_eq!(b[7].kind, Kind::Rubric);
    }

    #[test]
    fn code_keeps_whitespace_figures_notes_lists_tables_and_noise_are_handled() {
        let mut stats = Stats::default();
        let c = test_chapter(
            r##"<html><head><title>x</title></head><body><section data-type="chapter"><h1><span class="label">Chapter 4. </span>Styles</h1>
<p>Fire.<sup><a data-type="noteref" href="#n1">1</a></sup><a data-type="indexterm" id="x"/> More
 text.</p><pre data-type="programlisting"><code class="k">if</code><code> x {
    y();
}</code></pre>
<figure><div class="figure"><img src="a.png" alt="bms2"/><h6><span class="label">Figure 4-9. </span>Caption</h6></div></figure>
<figure><img src="b.png" alt="diagram"/></figure>
<aside data-type="sidebar"><h5>Side</h5><p>Aside text</p></aside>
<ul><li><p>one</p></li><li>two</li></ul><table><tr><td><p>a</p></td><td>b</td></tr></table>
<p class="liturgical">Line one<br/>Line two</p><nav epub:type="toc"><p>skip</p></nav><p class="ornament">✠</p>
</section><div data-type="footnotes"><p data-type="footnote"><sup><a>1</a></sup> True story.</p></div></body></html>"##,
            &mut stats,
        );
        let b = &c.blocks;
        assert_eq!(c.title, "Chapter 4. Styles");
        assert_eq!(b[1].text, "Fire.[1] More text.");
        assert_eq!(b[2].kind, Kind::Code);
        assert_eq!(b[2].text, "if x {\n    y();\n}");
        assert_eq!(b[3].kind, Kind::Figure);
        assert_eq!(b[3].text, "Figure 4-9. Caption");
        assert_eq!(b[4].text, "[image: diagram]");
        assert_eq!(b[5].kind, Kind::Heading);
        assert_eq!(b[6].kind, Kind::Quote);
        assert_eq!(b[7].text, "one");
        assert_eq!(b[8].kind, Kind::ListItem);
        assert_eq!(b[9].text, "a │ b");
        assert_eq!(b[10].text, "Line one\nLine two");
        assert_eq!(b[11].kind, Kind::Footnote);
        assert_eq!(b[11].text, "[1] True story.");
        assert_eq!(b.len(), 12);
    }

    #[test]
    fn image_paths_resolve_relative_to_their_content_document() {
        assert_eq!(
            resolve_member("OEBPS/text/x.xhtml", "../images/c.jpg").unwrap(),
            "OEBPS/images/c.jpg"
        );
        assert_eq!(
            resolve_member("OEBPS/ch4.html", "assets/b.png#x").unwrap(),
            "OEBPS/assets/b.png"
        );
        assert_eq!(resolve_member("x.html", "https://e.com/a.png"), None);
        assert_eq!(
            resolve_member("a/b.html", "/img/z.png").unwrap(),
            "img/z.png"
        );
        assert_eq!(resolve_member("a/b.html", "../../.."), None);
    }

    #[test]
    fn malformed_input_never_panics() {
        for source in [
            &b"<p>\xff\xfe&#x110000;&amp"[..],
            b"</p></div>text",
            b"<p><b>x</p>",
            b"<pre>\x00\x01",
            b"<sup>",
            b"<span class=\"verse-number\">99999999999</span>",
        ] {
            let mut page = String::new();
            let _ = chapter(source, &mut page, &mut Stats::default());
        }
    }
}
