//! Local history: every save of a text file also keeps a copy under
//! `<root>/.tectec/history/`, so a bad edit, a bad paste or a bad
//! "replace all" can be undone long after the editor's own undo is gone.
//!
//! Saves come every second or so while typing; keeping each would bury the
//! useful versions. A save within `WINDOW` of the newest snapshot replaces
//! it, so there is at most one version per couple of minutes of work, and
//! only the newest `KEEP` are kept per file.

use super::project::{self, Res};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

pub const HISTORY_DIR: &str = ".tectec/history";
const WINDOW: Duration = Duration::from_secs(120);
const KEEP: usize = 60;

#[derive(Clone, Debug, PartialEq)]
pub struct Version {
    pub path: PathBuf,
    /// Milliseconds since the epoch.
    pub time: u64,
    pub size: u64,
}

/// The folder holding one file's versions: its project-relative path with
/// the separators flattened, so every file gets its own.
fn folder(root: &Path, file: &Path) -> PathBuf {
    let rel = file.strip_prefix(root).unwrap_or(file).to_string_lossy().replace(['/', '\\'], "__");
    root.join(HISTORY_DIR).join(rel)
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// Every version of a file, newest first.
pub fn list(root: &Path, file: &Path) -> Vec<Version> {
    let Ok(entries) = fs::read_dir(folder(root, file)) else { return vec![] };
    let mut out: Vec<Version> = entries
        .flatten()
        .filter_map(|e| {
            let name = e.file_name().to_string_lossy().into_owned();
            let time = name.split('.').next()?.parse().ok()?;
            Some(Version { path: e.path(), time, size: e.metadata().map(|m| m.len()).unwrap_or(0) })
        })
        .collect();
    out.sort_by(|a, b| b.time.cmp(&a.time));
    out
}

/// Records `content` as the newest version of `file`.
pub fn snapshot(root: &Path, file: &Path, content: &str) -> Res<()> {
    snapshot_at(root, file, content, now_ms())
}

fn snapshot_at(root: &Path, file: &Path, content: &str, time: u64) -> Res<()> {
    let dir = folder(root, file);
    let versions = list(root, file);
    if let Some(newest) = versions.first() {
        if fs::read_to_string(&newest.path).map(|s| s == content).unwrap_or(false) {
            return Ok(());
        }
        // Still inside the window: this save replaces the newest version.
        if time.saturating_sub(newest.time) < WINDOW.as_millis() as u64 {
            let _ = fs::remove_file(&newest.path);
        }
    }
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let ext = project::ext_lower(file);
    let name = if ext.is_empty() { format!("{time}") } else { format!("{time}.{ext}") };
    project::atomic_write(&dir.join(name), content.as_bytes())?;
    for old in list(root, file).into_iter().skip(KEEP) {
        let _ = fs::remove_file(old.path);
    }
    Ok(())
}

pub fn read(version: &Version) -> Res<String> {
    fs::read_to_string(&version.path).map_err(|e| e.to_string())
}

/// Lines added and removed going from `old` to `new`, a cheap summary for
/// the version list (a multiset difference, not a real diff).
pub fn change_summary(old: &str, new: &str) -> (usize, usize) {
    use std::collections::HashMap;
    let mut count: HashMap<&str, i64> = HashMap::new();
    for l in old.lines() {
        *count.entry(l).or_default() -= 1;
    }
    for l in new.lines() {
        *count.entry(l).or_default() += 1;
    }
    let added = count.values().filter(|v| **v > 0).map(|v| *v as usize).sum();
    let removed = count.values().filter(|v| **v < 0).map(|v| (-*v) as usize).sum();
    (added, removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_one_version_per_window_and_skips_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let (root, file) = (dir.path(), dir.path().join("cap/um.tex"));
        let t0 = 1_000_000;
        snapshot_at(root, &file, "a", t0).unwrap();
        snapshot_at(root, &file, "ab", t0 + 5_000).unwrap(); // same window: replaces
        assert_eq!(list(root, &file).len(), 1);
        assert_eq!(read(&list(root, &file)[0]).unwrap(), "ab");
        snapshot_at(root, &file, "ab", t0 + 500_000).unwrap(); // unchanged: nothing
        assert_eq!(list(root, &file).len(), 1);
        snapshot_at(root, &file, "abc", t0 + 500_000).unwrap(); // new window
        let v = list(root, &file);
        assert_eq!(v.len(), 2);
        assert_eq!(read(&v[0]).unwrap(), "abc", "newest first");
        // Hidden from the project tree and from other files' history.
        assert!(v[0].path.to_string_lossy().contains(".tectec"));
        assert!(list(root, &dir.path().join("outro.tex")).is_empty());
    }

    #[test]
    fn keeps_only_the_newest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.tex");
        for i in 0..(KEEP as u64 + 5) {
            snapshot_at(dir.path(), &file, &format!("v{i}"), i * 1_000_000).unwrap();
        }
        let v = list(dir.path(), &file);
        assert_eq!(v.len(), KEEP);
        assert_eq!(read(&v[0]).unwrap(), format!("v{}", KEEP + 4));
    }

    #[test]
    fn summarizes_changes() {
        assert_eq!(change_summary("a\nb\nc", "a\nc\nd\ne"), (2, 1));
    }
}
