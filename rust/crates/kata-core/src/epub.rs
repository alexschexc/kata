//! EPUB container reading (ingest stage 2): in-memory zip, container.xml, OPF
//! metadata/manifest/spine and a DRM check. Ported from `src/epub.zig`; the zip container is
//! read with the `zip` crate instead of the hand-written central-directory parser.

use crate::xhtml::{entity_at, is_space};
use std::io::{Cursor, Read};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EpubError {
    NotZip,
    CorruptZip,
    MissingMember,
    MemberTooLarge,
    UnsupportedCompression,
    EncryptedEpub,
    NoRootfile,
    MissingSpineMember,
    EmptySpine,
}

impl std::fmt::Display for EpubError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for EpubError {}

const MAX_MEMBER: u64 = 64 << 20;

/// Zip archive held in memory (EPUBs are small enough).
pub struct Archive {
    zip: std::cell::RefCell<zip::ZipArchive<Cursor<Vec<u8>>>>,
    names: Vec<String>,
}

impl Archive {
    pub fn new(bytes: Vec<u8>) -> Result<Archive, EpubError> {
        if bytes.len() < 22 {
            return Err(EpubError::NotZip);
        }
        let zip = zip::ZipArchive::new(Cursor::new(bytes)).map_err(|e| match e {
            zip::result::ZipError::InvalidArchive(_) => EpubError::CorruptZip,
            _ => EpubError::NotZip,
        })?;
        let names = zip
            .file_names()
            .filter_map(Result::ok)
            .map(|name| name.into_owned())
            .collect();
        Ok(Archive {
            zip: std::cell::RefCell::new(zip),
            names,
        })
    }

    pub fn find(&self, name: &str) -> bool {
        self.names.iter().any(|n| n == name)
    }

    pub fn names(&self) -> &[String] {
        &self.names
    }

    /// Decompressed member contents (stored or deflate).
    pub fn read(&self, name: &str) -> Result<Vec<u8>, EpubError> {
        let mut zip = self.zip.borrow_mut();
        let mut file = zip.by_name(name).map_err(|e| match e {
            zip::result::ZipError::FileNotFound => EpubError::MissingMember,
            zip::result::ZipError::UnsupportedArchive(_) => EpubError::UnsupportedCompression,
            _ => EpubError::CorruptZip,
        })?;
        if file.size() > MAX_MEMBER {
            return Err(EpubError::MemberTooLarge);
        }
        let expected = file.size();
        let mut out = Vec::with_capacity(expected as usize);
        file.by_ref()
            .take(MAX_MEMBER + 1)
            .read_to_end(&mut out)
            .map_err(|_| EpubError::CorruptZip)?;
        if out.len() as u64 != expected {
            return Err(EpubError::CorruptZip);
        }
        Ok(out)
    }
}

/// Value of attribute `name` inside one start tag's text, if present.
pub fn attribute<'a>(tag: &'a [u8], name: &str) -> Option<&'a [u8]> {
    crate::xhtml::attribute(tag, name.as_bytes())
}

/// Iterates start tags `<prefix…>` (any namespace prefix) in XML text.
pub struct Tags<'a> {
    text: &'a [u8],
    local: &'a [u8],
    pub at: usize,
}

impl<'a> Tags<'a> {
    pub fn new(text: &'a [u8], local: &'a str) -> Self {
        Tags {
            text,
            local: local.as_bytes(),
            at: 0,
        }
    }
}

impl<'a> Iterator for Tags<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<&'a [u8]> {
        while let Some(open) = self
            .text
            .get(self.at..)?
            .iter()
            .position(|&c| c == b'<')
            .map(|i| i + self.at)
        {
            let close = self.text[open..]
                .iter()
                .position(|&c| c == b'>')
                .map(|i| i + open)?;
            self.at = close + 1;
            let mut name_end = open + 1;
            while name_end < close && !is_space(self.text[name_end]) && self.text[name_end] != b'/'
            {
                name_end += 1;
            }
            let name = &self.text[open + 1..name_end];
            if crate::xhtml::local_name(name) == self.local {
                return Some(&self.text[open..close]);
            }
        }
        None
    }
}

fn trim_ws(s: &[u8]) -> &[u8] {
    let start = s
        .iter()
        .position(|&c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
        .unwrap_or(s.len());
    let end = s
        .iter()
        .rposition(|&c| !matches!(c, b' ' | b'\t' | b'\r' | b'\n'))
        .map_or(start, |e| e + 1);
    &s[start..end.max(start)]
}

/// Text content of the first `<…local>` element (namespace-agnostic).
fn element_text<'a>(xml: &'a [u8], local: &'a str) -> Option<&'a [u8]> {
    let mut tags = Tags::new(xml, local);
    tags.next()?;
    let start = tags.at;
    let end = xml.get(start..)?.iter().position(|&c| c == b'<')? + start;
    Some(trim_ws(&xml[start..end]))
}

/// Metadata text with entities decoded and whitespace collapsed.
fn decode_text(raw: &[u8]) -> String {
    let mut out: Vec<u8> = Vec::new();
    let mut i = 0;
    while i < raw.len() {
        if raw[i] == b'&'
            && let Some((cp, len)) = entity_at(raw, i)
        {
            let c = char::from_u32(if cp == 0xA0 { ' ' as u32 } else { cp }).unwrap_or('\u{fffd}');
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
            i += len;
            continue;
        }
        let c = raw[i];
        if is_space(c) {
            if !out.is_empty() && *out.last().unwrap() != b' ' {
                out.push(b' ');
            }
        } else {
            out.push(c);
        }
        i += 1;
    }
    String::from_utf8_lossy(out.strip_suffix(b" ").unwrap_or(&out)).into_owned()
}

#[derive(Debug, Clone)]
struct Item {
    id: Vec<u8>,
    href: Vec<u8>,
    properties: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Package {
    pub title: String,
    pub author: String,
    /// Spine, in reading order, as zip member paths.
    pub spine: Vec<String>,
    /// EPUB 3 nav document or EPUB 2 NCX (zip member path), if any.
    pub toc: Option<String>,
}

fn join(base_dir: &str, href: &[u8]) -> String {
    let href = String::from_utf8_lossy(href);
    let clean = href.split('#').next().unwrap_or("");
    if base_dir.is_empty() {
        clean.to_string()
    } else {
        format!("{base_dir}/{clean}")
    }
}

/// Refuses DRM: encryption.xml listing anything other than font obfuscation.
pub fn check_drm(archive: &Archive) -> Result<(), EpubError> {
    if archive.find("META-INF/rights.xml") {
        return Err(EpubError::EncryptedEpub);
    }
    if !archive.find("META-INF/encryption.xml") {
        return Ok(());
    }
    // The Zig build reads this through a 64 KiB scratch buffer; anything larger is refused.
    let xml = archive
        .read("META-INF/encryption.xml")
        .map_err(|_| EpubError::EncryptedEpub)?;
    if xml.len() > 64 << 10 {
        return Err(EpubError::EncryptedEpub);
    }
    check_encryption(&xml)
}

/// Font obfuscation (IDPF, Adobe) only scrambles embedded fonts; anything else listed in
/// encryption.xml means content is encrypted (DRM): refuse it.
fn check_encryption(xml: &[u8]) -> Result<(), EpubError> {
    const ALLOWED: [&[u8]; 2] = [
        b"http://www.idpf.org/2008/embedding",
        b"http://ns.adobe.com/pdf/enc#RC",
    ];
    for tag in Tags::new(xml, "EncryptionMethod") {
        let algorithm = attribute(tag, "Algorithm").ok_or(EpubError::EncryptedEpub)?;
        if !ALLOWED.contains(&algorithm) {
            return Err(EpubError::EncryptedEpub);
        }
    }
    Ok(())
}

fn dirname_posix(path: &str) -> &str {
    match path.rfind('/') {
        Some(0) => "/",
        Some(i) => &path[..i],
        None => "",
    }
}

/// container.xml → OPF → metadata, manifest, spine, TOC.
pub fn package(archive: &Archive) -> Result<Package, EpubError> {
    check_drm(archive)?;
    let container = archive.read("META-INF/container.xml")?;
    let rootfile = Tags::new(&container, "rootfile")
        .next()
        .ok_or(EpubError::NoRootfile)?;
    let opf_path =
        String::from_utf8_lossy(attribute(rootfile, "full-path").ok_or(EpubError::NoRootfile)?)
            .into_owned();
    let opf = archive.read(&opf_path)?;
    let base_dir = dirname_posix(&opf_path);

    let items: Vec<Item> = Tags::new(&opf, "item")
        .filter_map(|tag| {
            Some(Item {
                id: attribute(tag, "id")?.to_vec(),
                href: attribute(tag, "href")?.to_vec(),
                properties: attribute(tag, "properties").unwrap_or(b"").to_vec(),
            })
        })
        .collect();
    let mut toc: Option<String> = None;
    for item in &items {
        if item.properties.windows(3).any(|w| w == b"nav") {
            toc = Some(join(base_dir, &item.href));
        }
    }
    if toc.is_none()
        && let Some(spine_tag) = Tags::new(&opf, "spine").next()
        && let Some(ncx_id) = attribute(spine_tag, "toc")
    {
        for item in &items {
            if item.id == ncx_id {
                toc = Some(join(base_dir, &item.href));
            }
        }
    }
    let mut spine = Vec::new();
    for tag in Tags::new(&opf, "itemref") {
        let Some(idref) = attribute(tag, "idref") else {
            continue;
        };
        if let Some(item) = items.iter().find(|item| item.id == idref) {
            let path = join(base_dir, &item.href);
            if !archive.find(&path) {
                return Err(EpubError::MissingSpineMember);
            }
            spine.push(path);
        }
    }
    if spine.is_empty() {
        return Err(EpubError::EmptySpine);
    }
    Ok(Package {
        title: decode_text(element_text(&opf, "title").unwrap_or(b"")),
        author: decode_text(element_text(&opf, "creator").unwrap_or(b"")),
        spine,
        toc,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn font_obfuscation_is_allowed_but_encrypted_content_is_refused() {
        assert!(check_encryption(br#"<encryption><EncryptedData><enc:EncryptionMethod Algorithm="http://www.idpf.org/2008/embedding"/></EncryptedData></encryption>"#).is_ok());
        assert_eq!(
            check_encryption(
                br#"<EncryptionMethod Algorithm="http://www.w3.org/2001/04/xmlenc#aes128-cbc"/>"#
            ),
            Err(EpubError::EncryptedEpub)
        );
        assert_eq!(
            check_encryption(b"<EncryptionMethod/>"),
            Err(EpubError::EncryptedEpub)
        );
    }

    #[test]
    fn metadata_entities_decode() {
        assert_eq!(decode_text(b"A &#8212;\n  B &amp; C"), "A \u{2014} B & C");
    }

    #[test]
    fn attribute_and_tag_scanning_are_namespace_and_quote_tolerant() {
        assert_eq!(
            attribute(b"<item id='x' href=\"a b\"", "href"),
            Some(&b"a b"[..])
        );
        assert_eq!(attribute(b"<item data-href=\"no\"", "href"), None);
        let mut tags = Tags::new(
            br#"<opf:item id="1"/><itemref idref="2"/><item id="3">"#,
            "item",
        );
        assert_eq!(attribute(tags.next().unwrap(), "id"), Some(&b"1"[..]));
        assert_eq!(attribute(tags.next().unwrap(), "id"), Some(&b"3"[..]));
        assert!(tags.next().is_none());
    }

    /// One stored member "a.txt" = "hello", written zip64-style (saturated sizes/offset in the
    /// central entry, real values in the 0x0001 extra, saturated end record pointing at the
    /// zip64 end record), as in the Zig test.
    fn zip64_hello() -> Vec<u8> {
        let mut b: Vec<u8> = Vec::new();
        let u16le = |b: &mut Vec<u8>, v: u16| b.extend_from_slice(&v.to_le_bytes());
        let u32le = |b: &mut Vec<u8>, v: u32| b.extend_from_slice(&v.to_le_bytes());
        let u64le = |b: &mut Vec<u8>, v: u64| b.extend_from_slice(&v.to_le_bytes());
        b.extend_from_slice(b"PK\x03\x04");
        u16le(&mut b, 45);
        u16le(&mut b, 0);
        u16le(&mut b, 0);
        u32le(&mut b, 0);
        u32le(&mut b, 0x3610a686);
        u32le(&mut b, 5);
        u32le(&mut b, 5);
        u16le(&mut b, 5);
        u16le(&mut b, 0);
        b.extend_from_slice(b"a.txthello");
        let central = b.len();
        b.extend_from_slice(b"PK\x01\x02");
        u16le(&mut b, 45);
        u16le(&mut b, 45);
        u16le(&mut b, 0);
        u16le(&mut b, 0);
        u32le(&mut b, 0);
        u32le(&mut b, 0x3610a686);
        u32le(&mut b, 0xFFFF_FFFF);
        u32le(&mut b, 0xFFFF_FFFF);
        u16le(&mut b, 5);
        u16le(&mut b, 4 + 24);
        u16le(&mut b, 0);
        u16le(&mut b, 0);
        u16le(&mut b, 0);
        u32le(&mut b, 0);
        u32le(&mut b, 0xFFFF_FFFF);
        b.extend_from_slice(b"a.txt");
        u16le(&mut b, 1);
        u16le(&mut b, 24);
        u64le(&mut b, 5);
        u64le(&mut b, 5);
        u64le(&mut b, 0);
        let record = b.len();
        let central_len = (record - central) as u64;
        b.extend_from_slice(b"PK\x06\x06");
        u64le(&mut b, 44);
        u16le(&mut b, 45);
        u16le(&mut b, 45);
        u32le(&mut b, 0);
        u32le(&mut b, 0);
        u64le(&mut b, 1);
        u64le(&mut b, 1);
        u64le(&mut b, central_len);
        u64le(&mut b, central as u64);
        b.extend_from_slice(b"PK\x06\x07");
        u32le(&mut b, 0);
        u64le(&mut b, record as u64);
        u32le(&mut b, 1);
        b.extend_from_slice(b"PK\x05\x06");
        u16le(&mut b, 0);
        u16le(&mut b, 0);
        u16le(&mut b, 0xFFFF);
        u16le(&mut b, 0xFFFF);
        u32le(&mut b, 0xFFFF_FFFF);
        u32le(&mut b, 0xFFFF_FFFF);
        u16le(&mut b, 0);
        b
    }

    #[test]
    fn zip64_central_directory_and_extra_fields_are_read() {
        let archive = Archive::new(zip64_hello()).unwrap();
        assert_eq!(archive.names(), ["a.txt"]);
        assert_eq!(archive.read("a.txt").unwrap(), b"hello");
        assert_eq!(archive.read("missing"), Err(EpubError::MissingMember));
    }

    #[test]
    fn garbage_is_not_a_zip() {
        assert!(Archive::new(vec![0; 10]).is_err());
        assert!(Archive::new(vec![0x50; 4096]).is_err());
    }

    #[test]
    fn sample_epubs_open_with_metadata_spine_and_toc() {
        let samples = [
            (
                "The Psalter According to the Seventy - Holy Transfiguration Monastery (1).epub",
                "Psalter",
                33,
            ),
            ("buildingmicroservices2ndedition.epub", "Microservices", 29),
        ];
        for (file, title, spine) in samples {
            let path = format!("{}/../../../examplePubs/{file}", env!("CARGO_MANIFEST_DIR"));
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            }; // samples are local, not in git
            let archive = Archive::new(bytes).unwrap();
            let pkg = package(&archive).unwrap();
            assert!(pkg.title.contains(title), "{}", pkg.title);
            assert_eq!(pkg.spine.len(), spine);
            assert!(pkg.toc.is_some());
            for member in &pkg.spine {
                assert!(
                    archive
                        .read(member)
                        .unwrap()
                        .windows(5)
                        .any(|w| w == b"<body")
                );
            }
        }
    }
}
