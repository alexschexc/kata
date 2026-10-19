//! Ingest pipeline: convert new or changed EPUBs from the ingest folder into library documents,
//! keep a manifest of what was converted, and index the library. Ported from `src/ingest.zig`;
//! manifest, index and folders files keep the Zig JSON layout.

use crate::convert;
use crate::document;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};
use std::time::UNIX_EPOCH;

pub const CONVERTER_VERSION: u32 = convert::VERSION;
pub const MANIFEST_NAME: &str = ".kata-manifest.json";
pub const INDEX_NAME: &str = ".kata-index.json";
pub const INGEST_FOLDER: &str = "kataIngest";
pub const LIBRARY_FOLDER: &str = "kataLibrary";

#[derive(Debug)]
pub enum IngestError {
    Io(io::Error),
    Json(String),
    InvalidFolders,
    HomeUnavailable,
    FolderMustBeAbsolute,
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::Io(e) => write!(f, "{e}"),
            IngestError::Json(m) => write!(f, "{m}"),
            IngestError::InvalidFolders => write!(f, "InvalidFolders"),
            IngestError::HomeUnavailable => write!(f, "HomeUnavailable"),
            IngestError::FolderMustBeAbsolute => write!(f, "FolderMustBeAbsolute"),
        }
    }
}

impl std::error::Error for IngestError {}

impl From<io::Error> for IngestError {
    fn from(e: io::Error) -> Self {
        IngestError::Io(e)
    }
}

/// User-chosen folder locations (config file `kata/folders.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, default)]
pub struct Folders {
    pub version: u8,
    pub ingest: String,
    pub library: String,
}

impl Default for Folders {
    fn default() -> Self {
        Folders {
            version: 1,
            ingest: String::new(),
            library: String::new(),
        }
    }
}

impl Folders {
    pub fn configured(&self) -> bool {
        !self.ingest.is_empty() && !self.library.is_empty()
    }
}

/// Identity of a source EPUB. Cheap fields first; hash only when they change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Fingerprint {
    pub size: u64,
    pub mtime_ns: i128,
    pub sha256: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEntry {
    /// EPUB file name inside the ingest folder, e.g. "foo.epub".
    pub source: String,
    /// Output document name inside the library folder, e.g. "foo".
    pub output: String,
    pub fingerprint: Fingerprint,
    pub converter_version: u32,
    #[serde(default)]
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Manifest {
    pub version: u8,
    pub entries: Vec<ManifestEntry>,
}

impl Default for Manifest {
    fn default() -> Self {
        Manifest {
            version: 1,
            entries: Vec::new(),
        }
    }
}

impl Manifest {
    pub fn find(&self, source: &str) -> Option<&ManifestEntry> {
        self.entries.iter().find(|e| e.source == source)
    }
}

/// One readable document in the library folder (`.kata-index.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IndexEntry {
    /// File name in the library folder.
    pub file: String,
    pub title: String,
    #[serde(default)]
    pub author: String,
    pub chapters: u32,
    pub blocks: u32,
    pub size: u64,
    pub mtime_ns: i128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Index {
    pub version: u8,
    pub entries: Vec<IndexEntry>,
}

impl Default for Index {
    fn default() -> Self {
        Index {
            version: 1,
            entries: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    Converted,
    Unchanged,
    Failed,
    Orphaned,
}

impl Outcome {
    pub fn name(self) -> &'static str {
        match self {
            Outcome::Converted => "converted",
            Outcome::Unchanged => "unchanged",
            Outcome::Failed => "failed",
            Outcome::Orphaned => "orphaned",
        }
    }
}

#[derive(Debug, Clone)]
pub struct Report {
    pub name: String,
    pub outcome: Outcome,
    pub detail: String,
}

#[derive(Debug, Default)]
pub struct Summary {
    pub reports: Vec<Report>,
    pub converted: usize,
    pub unchanged: usize,
    pub failed: usize,
    pub orphaned: usize,
    pub indexed: usize,
    pub index_rebuilt: bool,
}

/// Output name for a source: the file name without its ".epub" extension.
pub fn output_name(source_name: &str) -> &str {
    let bytes = source_name.as_bytes();
    if bytes.len() >= 5 && bytes[bytes.len() - 5..].eq_ignore_ascii_case(b".epub") {
        &source_name[..source_name.len() - 5]
    } else {
        source_name
    }
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<Option<T>, IngestError> {
    match fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|e| IngestError::Json(format!("{}: {e}", path.display()))),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.into()),
    }
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        fs::create_dir_all(parent)?;
    }
    let mut temporary = path.as_os_str().to_owned();
    temporary.push(".tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

fn write_json<T: Serialize>(path: &Path, value: &T) -> Result<(), IngestError> {
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| IngestError::Json(e.to_string()))?;
    Ok(write_atomic(path, &bytes)?)
}

fn folders_path(config_home: &Path) -> PathBuf {
    config_home.join("kata").join("folders.json")
}

/// Stage 0: the saved folder choice, or None when the user has not chosen.
pub fn load_folders(config_home: &Path) -> Result<Option<Folders>, IngestError> {
    let Some(folders) = read_json::<Folders>(&folders_path(config_home))? else {
        return Ok(None);
    };
    if folders.version != 1 || !folders.configured() {
        return Err(IngestError::InvalidFolders);
    }
    Ok(Some(folders))
}

/// Saves the folder choice and creates both folders.
pub fn save_folders(config_home: &Path, folders: &Folders) -> Result<(), IngestError> {
    fs::create_dir_all(&folders.ingest)?;
    fs::create_dir_all(&folders.library)?;
    write_json(&folders_path(config_home), folders)
}

/// Expands a leading `~/` and requires an absolute result, so the saved folders do not depend on
/// the directory Kata happens to be launched from.
pub fn normalize_folder(raw: &str, home: Option<&str>) -> Result<String, IngestError> {
    let trimmed = raw.trim_matches([' ', '\t']);
    let expanded: PathBuf = if trimmed == "~" || trimmed.starts_with("~/") {
        Path::new(home.ok_or(IngestError::HomeUnavailable)?).join(&trimmed[trimmed.len().min(2)..])
    } else {
        PathBuf::from(trimmed)
    };
    if !expanded.is_absolute() {
        return Err(IngestError::FolderMustBeAbsolute);
    }
    let mut out = PathBuf::new();
    for component in expanded.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out.to_string_lossy().into_owned())
}

/// Defaults offered by the first-run prompt: `kataIngest`/`kataLibrary` inside a parent directory
/// the user confirms or edits.
pub fn default_folders(parent: &Path) -> Folders {
    Folders {
        version: 1,
        ingest: parent.join(INGEST_FOLDER).to_string_lossy().into_owned(),
        library: parent.join(LIBRARY_FOLDER).to_string_lossy().into_owned(),
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn mtime_ns(meta: &fs::Metadata) -> i128 {
    match meta.modified().ok().map(|t| t.duration_since(UNIX_EPOCH)) {
        Some(Ok(d)) => d.as_nanos() as i128,
        Some(Err(e)) => -(e.duration().as_nanos() as i128),
        None => 0,
    }
}

/// Regular files and symlinks in `path`, byte-wise sorted; a missing folder is empty.
fn list_files(path: &Path) -> Result<Vec<String>, IngestError> {
    let read = match fs::read_dir(path) {
        Ok(read) => read,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(e.into()),
    };
    let mut names = Vec::new();
    for entry in read {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_file() || kind.is_symlink() {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort_unstable();
    Ok(names)
}

fn ends_with_epub(name: &str) -> bool {
    name.len() >= 5 && name.as_bytes()[name.len() - 5..].eq_ignore_ascii_case(b".epub")
}

/// Runs the whole pipeline: convert new/changed EPUBs, report orphans, and rebuild the index only
/// when the library changed. Never deletes anything. `force` reconverts every EPUB regardless of
/// the manifest.
pub fn run(folders: &Folders, force: bool) -> Result<Summary, IngestError> {
    fs::create_dir_all(&folders.ingest)?;
    fs::create_dir_all(&folders.library)?;
    let library = Path::new(&folders.library);
    let manifest_path = library.join(MANIFEST_NAME);
    let old = read_json::<Manifest>(&manifest_path)?.unwrap_or_default();
    let mut entries: Vec<ManifestEntry> = Vec::new();
    let mut summary = Summary::default();
    let mut changed = false;

    let sources = list_files(Path::new(&folders.ingest))?;
    for name in &sources {
        if !ends_with_epub(name) {
            continue;
        }
        let path = Path::new(&folders.ingest).join(name);
        let output = output_name(name);
        let output_path = library.join(output);
        let meta = fs::metadata(&path)?;
        let mut fingerprint = Fingerprint {
            size: meta.len(),
            mtime_ns: mtime_ns(&meta),
            sha256: String::new(),
        };
        let previous = old.find(name);
        let reusable = !force
            && output_path.exists()
            && previous.is_some_and(|p| p.converter_version == CONVERTER_VERSION);
        if reusable
            && let Some(p) = previous
            && p.fingerprint.size == fingerprint.size
            && p.fingerprint.mtime_ns == fingerprint.mtime_ns
        {
            entries.push(p.clone());
            summary.reports.push(Report {
                name: name.clone(),
                outcome: Outcome::Unchanged,
                detail: String::new(),
            });
            summary.unchanged += 1;
            continue;
        }
        let fail = |summary: &mut Summary, entries: &mut Vec<ManifestEntry>, detail: String| {
            summary.reports.push(Report {
                name: name.clone(),
                outcome: Outcome::Failed,
                detail,
            });
            summary.failed += 1;
            if let Some(p) = previous {
                entries.push(p.clone());
            }
        };
        let bytes = match fs::read(&path) {
            Ok(bytes) if bytes.len() <= 256 << 20 => bytes,
            Ok(_) => {
                fail(&mut summary, &mut entries, "StreamTooLong".into());
                continue;
            }
            Err(e) => {
                fail(&mut summary, &mut entries, e.kind().to_string());
                continue;
            }
        };
        fingerprint.sha256 = sha256_hex(&bytes);
        // Touched but byte-identical: refresh the fingerprint, keep the output.
        if reusable
            && let Some(p) = previous
            && p.fingerprint.sha256 == fingerprint.sha256
        {
            let mut refreshed = p.clone();
            refreshed.fingerprint = fingerprint;
            entries.push(refreshed);
            summary.reports.push(Report {
                name: name.clone(),
                outcome: Outcome::Unchanged,
                detail: String::new(),
            });
            summary.unchanged += 1;
            changed = true;
            continue;
        }
        let result = match convert::convert(bytes, name) {
            Ok(result) => result,
            Err(e) => {
                fail(&mut summary, &mut entries, e.to_string());
                continue;
            }
        };
        let text = document::write(&result.doc);
        // Images first, so a document never references missing assets.
        if !result.assets.is_empty() {
            let assets_dir = PathBuf::from(format!("{}.assets", output_path.display()));
            fs::create_dir_all(&assets_dir)?;
            for asset in &result.assets {
                write_atomic(&assets_dir.join(&asset.name), &asset.bytes)?;
            }
        }
        write_atomic(&output_path, text.as_bytes())?;
        let s = result.stats;
        entries.push(ManifestEntry {
            source: name.clone(),
            output: output.to_string(),
            fingerprint,
            converter_version: CONVERTER_VERSION,
            title: result.doc.title.clone(),
        });
        let corrected = if s.verse_mismatches > 0 {
            " (verse numbering corrected)"
        } else {
            ""
        };
        summary.reports.push(Report {
            name: name.clone(),
            outcome: Outcome::Converted,
            detail: format!(
                "{} chapters, {} blocks, {} verses, {} pages, {} code, {} figures, {} images, {} notes{corrected}",
                s.chapters, s.blocks, s.verses, s.pages, s.code, s.figures, s.images, s.footnotes
            ),
        });
        summary.converted += 1;
        changed = true;
    }
    for entry in &old.entries {
        if sources.contains(&entry.source) {
            continue;
        }
        summary.reports.push(Report {
            name: entry.output.clone(),
            outcome: Outcome::Orphaned,
            detail: "source EPUB missing; document kept".into(),
        });
        summary.orphaned += 1;
        entries.push(entry.clone());
    }
    if changed || old.entries.len() != entries.len() {
        write_json(
            &manifest_path,
            &Manifest {
                version: 1,
                entries,
            },
        )?;
    }
    let (indexed, rebuilt) = index(library, changed)?;
    summary.indexed = indexed.len();
    summary.index_rebuilt = rebuilt;
    Ok(summary)
}

/// Stage 4: index every document in the library folder. Unchanged files (same size and mtime as
/// indexed) keep their entries without reparsing. Returns the entries and whether it rewrote them.
pub fn index(library_dir: &Path, force: bool) -> Result<(Vec<IndexEntry>, bool), IngestError> {
    let index_path = library_dir.join(INDEX_NAME);
    let old = read_json::<Index>(&index_path)?.unwrap_or_default();
    let mut entries: Vec<IndexEntry> = Vec::new();
    let mut rebuilt = force;
    for name in list_files(library_dir)? {
        if name.is_empty() || name.starts_with('.') || name.ends_with(".tmp") {
            continue;
        }
        let path = library_dir.join(&name);
        let meta = fs::metadata(&path)?;
        let (size, mtime) = (meta.len(), mtime_ns(&meta));
        if let Some(entry) = old
            .entries
            .iter()
            .find(|e| e.file == name && e.size == size && e.mtime_ns == mtime)
        {
            entries.push(entry.clone());
            continue;
        }
        rebuilt = true;
        let Ok(bytes) = fs::read(&path) else { continue };
        let Ok(doc) = document::parse(&bytes) else {
            continue;
        }; // not a Kata document
        entries.push(IndexEntry {
            file: name,
            title: doc.title.clone(),
            author: doc.author.clone(),
            chapters: doc.chapters.len() as u32,
            blocks: doc.block_count() as u32,
            size,
            mtime_ns: mtime,
        });
    }
    if entries.len() != old.entries.len() {
        rebuilt = true;
    }
    if rebuilt {
        write_json(
            &index_path,
            &Index {
                version: 1,
                entries: entries.clone(),
            },
        )?;
    }
    Ok((entries, rebuilt))
}

/// Reads the current index without touching the library (fast startup path).
pub fn load_index(library_dir: &Path) -> Result<Vec<IndexEntry>, IngestError> {
    Ok(read_json::<Index>(&library_dir.join(INDEX_NAME))?
        .map(|i| i.entries)
        .unwrap_or_default())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_name_drops_the_epub_extension_only() {
        assert_eq!(output_name("foo.epub"), "foo");
        assert_eq!(output_name("Foo.EPUB"), "Foo");
        assert_eq!(output_name("foo.epub.bak"), "foo.epub.bak");
        assert_eq!(output_name("epub"), "epub");
    }

    // Uses POSIX paths; Windows absolute paths need drive letters.
    #[cfg(unix)]
    #[test]
    fn folder_paths_expand_home_and_must_be_absolute() {
        assert_eq!(
            normalize_folder(" ~/kataLibrary/ ", Some("/home/r")).unwrap(),
            "/home/r/kataLibrary"
        );
        assert_eq!(
            normalize_folder("/srv/x/../books", None).unwrap(),
            "/srv/books"
        );
        assert!(matches!(
            normalize_folder("books", Some("/home/r")),
            Err(IngestError::FolderMustBeAbsolute)
        ));
        assert!(matches!(
            normalize_folder("~/x", None),
            Err(IngestError::HomeUnavailable)
        ));
    }

    #[test]
    fn sha256_is_lowercase_hex() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
    }

    #[test]
    fn missing_folders_file_means_not_chosen_and_invalid_ones_are_refused() {
        let dir = std::env::temp_dir().join(format!("kata-folders-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        assert_eq!(load_folders(&dir).unwrap(), None);
        let folders = Folders {
            version: 1,
            ingest: dir.join("in").to_string_lossy().into(),
            library: dir.join("lib").to_string_lossy().into(),
        };
        save_folders(&dir, &folders).unwrap();
        assert_eq!(load_folders(&dir).unwrap(), Some(folders));
        write_atomic(
            &folders_path(&dir),
            br#"{"version":1,"ingest":"","library":""}"#,
        )
        .unwrap();
        assert!(matches!(
            load_folders(&dir),
            Err(IngestError::InvalidFolders)
        ));
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn pipeline_converts_once_skips_unchanged_indexes_and_reports_orphans() {
        let samples = format!(
            "{}/../../../examplePubs/buildingmicroservices2ndedition.epub",
            env!("CARGO_MANIFEST_DIR")
        );
        let Ok(bytes) = fs::read(&samples) else {
            return;
        }; // samples are local, not in git
        let root = std::env::temp_dir().join(format!("kata-ingest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let folders = Folders {
            version: 1,
            ingest: root.join("in").to_string_lossy().into(),
            library: root.join("lib").to_string_lossy().into(),
        };
        fs::create_dir_all(&folders.ingest).unwrap();
        fs::write(Path::new(&folders.ingest).join("book.epub"), &bytes).unwrap();
        fs::write(
            Path::new(&folders.ingest).join("bad.epub"),
            b"not a zip at all",
        )
        .unwrap();
        let first = run(&folders, false).unwrap();
        assert_eq!((first.converted, first.failed, first.indexed), (1, 1, 1));
        assert!(first.index_rebuilt);
        let second = run(&folders, false).unwrap();
        assert_eq!(
            (second.converted, second.unchanged, second.failed),
            (0, 1, 1)
        );
        assert!(!second.index_rebuilt);
        fs::remove_file(Path::new(&folders.ingest).join("book.epub")).unwrap();
        let third = run(&folders, false).unwrap();
        assert_eq!(third.orphaned, 1);
        assert!(Path::new(&folders.library).join("book").exists());
        fs::remove_dir_all(root).unwrap();
    }
}
