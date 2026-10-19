//! Plan catalog and per-plan sidecar files, ported from `src/catalog.zig`: named plans
//! deduplicated by content hash, isolated progress paths, hidden plans and the remembered plan
//! selection.

use crate::plan::{Plan, PlanError};
use crate::wyhash;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

#[derive(Debug)]
pub enum CatalogError {
    Plan(PlanError),
    InvalidPlanName,
    Io(io::Error),
    Parse(String),
}

impl std::fmt::Display for CatalogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CatalogError::Plan(e) => write!(f, "{e}"),
            CatalogError::InvalidPlanName => write!(f, "InvalidPlanName"),
            CatalogError::Io(e) => write!(f, "{e}"),
            CatalogError::Parse(m) => write!(f, "{m}"),
        }
    }
}

impl std::error::Error for CatalogError {}

impl From<PlanError> for CatalogError {
    fn from(e: PlanError) -> Self {
        CatalogError::Plan(e)
    }
}

impl From<io::Error> for CatalogError {
    fn from(e: io::Error) -> Self {
        CatalogError::Io(e)
    }
}

pub struct Entry {
    pub id: String,
    pub plan: Plan,
    pub hash: u64,
}

#[derive(Default)]
pub struct Catalog {
    pub entries: Vec<Entry>,
    /// Files in a discovered folder that were not valid plans.
    pub skipped: usize,
}

/// Lexically resolve `path` against the current directory, like Zig's `std.fs.path.resolve`.
fn resolve(path: &Path) -> io::Result<PathBuf> {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    };
    let mut out = PathBuf::new();
    for component in joined.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other.as_os_str()),
        }
    }
    Ok(out)
}

impl Catalog {
    pub fn new() -> Catalog {
        Catalog::default()
    }

    /// Adds a plan, or returns the index of an identical one already present.
    pub fn add(&mut self, id: &str, bytes: &[u8]) -> Result<usize, CatalogError> {
        let hash = wyhash::hash(0, bytes);
        if let Some(index) = self.entries.iter().position(|e| e.hash == hash) {
            return Ok(index);
        }
        let plan = Plan::new(bytes)?;
        if plan.name().bytes().any(|c| c < 32 || c == 127) {
            return Err(CatalogError::InvalidPlanName);
        }
        self.entries.push(Entry {
            id: id.to_string(),
            plan,
            hash,
        });
        Ok(self.entries.len() - 1)
    }

    pub fn add_file(&mut self, path: &Path) -> Result<usize, CatalogError> {
        let absolute = resolve(path)?;
        let file = fs::File::open(&absolute)?;
        let mut bytes = Vec::new();
        io::Read::read_to_end(&mut io::Read::take(file, 1024 * 1024 + 1), &mut bytes)?;
        if bytes.len() > 1024 * 1024 {
            return Err(CatalogError::Parse("plan file larger than 1 MiB".into()));
        }
        self.add(&absolute.to_string_lossy(), &bytes)
    }

    /// Adds every `*.json` file in `dir` in byte-wise name order; invalid ones are counted in
    /// `skipped`. A missing directory is fine.
    pub fn discover(&mut self, dir: &Path) -> Result<(), CatalogError> {
        let read = match fs::read_dir(dir) {
            Ok(read) => read,
            Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e.into()),
        };
        let mut names: Vec<String> = Vec::new();
        for entry in read {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            if entry.file_type()?.is_file() && name.ends_with(".json") {
                names.push(name);
            }
        }
        names.sort_unstable();
        for name in names {
            if self.add_file(&dir.join(name)).is_err() {
                self.skipped += 1;
            }
        }
        Ok(())
    }

    /// Drops entry `index` from the in-memory list (files are the caller's concern).
    pub fn remove(&mut self, index: usize) {
        self.entries.remove(index);
    }

    /// Removes every entry whose id is in `ids` (never the last remaining plan).
    pub fn drop_hidden(&mut self, ids: &[String]) {
        let mut i = self.entries.len();
        while i > 0 {
            i -= 1;
            if self.entries.len() <= 1 {
                return;
            }
            if ids.iter().any(|id| *id == self.entries[i].id) {
                self.remove(i);
            }
        }
    }
}

/// Progress file for a plan: the base state file when it is the first plan used or the same
/// plan, otherwise an isolated sidecar named after the plan hash.
pub fn progress_path(base: &str, base_hash: u64, hash: u64) -> String {
    if base_hash == 0 || base_hash == hash {
        base.to_string()
    } else {
        format!("{base}.plans/{hash:x}.json")
    }
}

fn hidden_path(base: &str) -> String {
    format!("{base}.hidden-plans")
}

fn write_atomically(path: &str, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let temporary = format!("{path}.tmp");
    fs::write(&temporary, bytes)?;
    fs::rename(&temporary, path)
}

fn read_limited(path: &str, limit: u64) -> Result<Option<Vec<u8>>, CatalogError> {
    let file = match fs::File::open(path) {
        Ok(file) => file,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let mut bytes = Vec::new();
    io::Read::read_to_end(&mut io::Read::take(file, limit + 1), &mut bytes)?;
    if bytes.len() as u64 > limit {
        return Err(CatalogError::Parse(format!("{path}: file too large")));
    }
    Ok(Some(bytes))
}

pub fn load_hidden(base: &str) -> Result<Vec<String>, CatalogError> {
    let Some(bytes) = read_limited(&hidden_path(base), 65536)? else {
        return Ok(Vec::new());
    };
    Ok(String::from_utf8_lossy(&bytes)
        .split('\n')
        .filter(|l| !l.is_empty())
        .map(str::to_string)
        .collect())
}

pub fn hide(base: &str, id: &str) -> Result<(), CatalogError> {
    let path = hidden_path(base);
    let mut contents = read_limited(&path, 65536)?.unwrap_or_default();
    contents.extend_from_slice(id.as_bytes());
    contents.push(b'\n');
    write_atomically(&path, &contents)?;
    Ok(())
}

#[derive(Serialize, Deserialize)]
struct Selection {
    id: String,
}

pub fn load_selected(base: &str) -> Result<Option<String>, CatalogError> {
    let Some(bytes) = read_limited(&format!("{base}.selection.json"), 65536)? else {
        return Ok(None);
    };
    let selection: Selection =
        serde_json::from_slice(&bytes).map_err(|e| CatalogError::Parse(e.to_string()))?;
    Ok(Some(selection.id))
}

pub fn save_selected(base: &str, id: &str) -> Result<(), CatalogError> {
    let bytes = serde_json::to_vec(&Selection { id: id.to_string() })
        .map_err(|e| CatalogError::Parse(e.to_string()))?;
    write_atomically(&format!("{base}.selection.json"), &bytes)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHORT: &str = r#"{"name":"Short","repeat":true,"streams":[{"books":[{"name":"John","chapters":1}]}],"phases":[{"days":1,"rates":[1]}]}"#;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kata-catalog-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn catalog_lists_named_plans_and_deduplicates_identical_config_content() {
        let mut catalog = Catalog::new();
        assert_eq!(catalog.add("builtin:test", SHORT.as_bytes()).unwrap(), 0);
        assert_eq!(catalog.add("copy.json", SHORT.as_bytes()).unwrap(), 0);
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].plan.name(), "Short");
    }

    #[test]
    fn plan_progress_uses_existing_base_state_or_an_isolated_sidecar() {
        assert_eq!(progress_path("/tmp/state.json", 10, 10), "/tmp/state.json");
        assert_eq!(
            progress_path("/tmp/state.json", 10, 11),
            "/tmp/state.json.plans/b.json"
        );
        assert_eq!(progress_path("/tmp/state.json", 0, 10), "/tmp/state.json");
    }

    #[test]
    fn plan_names_with_control_characters_are_rejected() {
        let bad = SHORT.replace("Short", "Sh\\u0007ort");
        assert!(matches!(
            Catalog::new().add("x", bad.as_bytes()),
            Err(CatalogError::InvalidPlanName)
        ));
    }

    #[test]
    fn discover_adds_json_plans_in_name_order_and_counts_invalid_ones() {
        let dir = scratch("discover");
        fs::write(dir.join("b.json"), SHORT.replace("Short", "Bee")).unwrap();
        fs::write(dir.join("a.json"), SHORT.replace("Short", "Aye")).unwrap();
        fs::write(dir.join("bad.json"), "{").unwrap();
        fs::write(dir.join("notes.txt"), "ignored").unwrap();
        let mut catalog = Catalog::new();
        catalog.discover(&dir).unwrap();
        assert_eq!(
            catalog
                .entries
                .iter()
                .map(|e| e.plan.name())
                .collect::<Vec<_>>(),
            ["Aye", "Bee"]
        );
        assert_eq!(catalog.skipped, 1);
        Catalog::new().discover(&dir.join("missing")).unwrap();
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn hidden_plans_and_the_remembered_selection_round_trip() {
        let dir = scratch("sidecars");
        let base = dir.join("state.json").to_string_lossy().into_owned();
        assert!(load_hidden(&base).unwrap().is_empty());
        hide(&base, "one.json").unwrap();
        hide(&base, "two.json").unwrap();
        assert_eq!(load_hidden(&base).unwrap(), ["one.json", "two.json"]);
        assert_eq!(load_selected(&base).unwrap(), None);
        save_selected(&base, "/plans/x.json").unwrap();
        assert_eq!(
            load_selected(&base).unwrap().as_deref(),
            Some("/plans/x.json")
        );
        assert_eq!(
            fs::read_to_string(format!("{base}.selection.json")).unwrap(),
            r#"{"id":"/plans/x.json"}"#
        );
        fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn dropping_hidden_plans_never_removes_the_last_one() {
        let mut catalog = Catalog::new();
        for name in ["A", "B", "C"] {
            catalog
                .add(
                    &format!("{name}.json"),
                    SHORT.replace("Short", name).as_bytes(),
                )
                .unwrap();
        }
        catalog.drop_hidden(&["A.json".into(), "B.json".into(), "C.json".into()]);
        assert_eq!(catalog.entries.len(), 1);
        assert_eq!(catalog.entries[0].id, "A.json");
    }
}
