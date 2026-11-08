//! Watches the project folder and reports the paths that changed, debounced
//! and with the app's own saves filtered out, so an external edit (another
//! editor, `git checkout`, a sync client) shows up instead of going stale.

use super::project::{is_build_artifact, mtime_millis};
use notify::{RecommendedWatcher, RecursiveMode, Watcher as _};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// Coalesces a burst of events into one callback.
const DEBOUNCE: Duration = Duration::from_millis(400);
/// How long a self-write's mtime is remembered.
const SUPPRESS_WINDOW: Duration = Duration::from_secs(5);

pub struct Watcher {
    _inner: RecommendedWatcher,
    suppress: Arc<Mutex<HashMap<PathBuf, (i64, Instant)>>>,
}

impl Watcher {
    /// Starts watching `root` recursively, calling `on_change` from a
    /// background thread once each burst of events settles.
    pub fn new(root: &Path, on_change: impl Fn(Vec<PathBuf>) + Send + 'static) -> notify::Result<Self> {
        let (tx, rx) = mpsc::channel::<PathBuf>();
        let root_buf = root.to_path_buf();
        let mut inner = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            let Ok(event) = res else { return };
            if matches!(event.kind, notify::EventKind::Access(_)) {
                return;
            }
            for path in event.paths {
                if ignored(&root_buf, &path) {
                    continue;
                }
                let _ = tx.send(path);
            }
        })?;
        inner.watch(root, RecursiveMode::Recursive)?;

        let suppress: Arc<Mutex<HashMap<PathBuf, (i64, Instant)>>> = Arc::default();
        let suppress_thread = suppress.clone();
        std::thread::spawn(move || {
            let mut pending: HashSet<PathBuf> = HashSet::new();
            loop {
                let received = if pending.is_empty() {
                    rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
                } else {
                    rx.recv_timeout(DEBOUNCE)
                };
                match received {
                    Ok(path) => {
                        pending.insert(path);
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {
                        let mut sup = suppress_thread.lock().unwrap();
                        let changed: Vec<PathBuf> = pending
                            .drain()
                            .filter(|p| match sup.remove(p) {
                                Some((mt, _)) => mtime_millis(p) != mt,
                                None => true,
                            })
                            .collect();
                        drop(sup);
                        if !changed.is_empty() {
                            on_change(changed);
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Disconnected) => return,
                }
            }
        });
        Ok(Self { _inner: inner, suppress })
    }

    /// Records that `path` was just written by the app itself; the next event
    /// for it is dropped when the mtime still matches.
    pub fn suppress(&self, path: &Path) {
        let mut sup = self.suppress.lock().unwrap();
        sup.retain(|_, (_, at)| at.elapsed() < SUPPRESS_WINDOW);
        sup.insert(path.to_path_buf(), (mtime_millis(path), Instant::now()));
    }
}

/// Hidden entries (anything under a dot-folder such as `.git`) and build
/// artifacts never count as a change.
fn ignored(root: &Path, path: &Path) -> bool {
    let rel = path.strip_prefix(root).unwrap_or(path);
    if rel.components().any(|c| c.as_os_str().to_string_lossy().starts_with('.')) {
        return true;
    }
    path.file_name().map(|n| is_build_artifact(&n.to_string_lossy())).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn watch(root: &Path) -> (Watcher, mpsc::Receiver<Vec<PathBuf>>) {
        let (tx, rx) = mpsc::channel();
        let w = Watcher::new(root, move |paths| {
            let _ = tx.send(paths);
        })
        .expect("watcher");
        (w, rx)
    }

    #[test]
    fn reports_a_new_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let (_w, rx) = watch(&root);
        let target = root.join("new.tex");
        fs::write(&target, "hello").unwrap();
        let paths = rx.recv_timeout(Duration::from_secs(3)).expect("no change reported");
        assert!(paths.contains(&target), "{paths:?}");
    }

    #[test]
    fn ignores_hidden_folders_and_artifacts() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir(root.join(".git")).unwrap();
        let (_w, rx) = watch(&root);
        fs::write(root.join(".git/index"), "x").unwrap();
        fs::write(root.join("main.aux"), "x").unwrap();
        assert!(rx.recv_timeout(DEBOUNCE * 2).is_err());
    }

    #[test]
    fn suppresses_self_writes_only() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let target = root.join("a.tex");
        fs::write(&target, "original").unwrap();
        let (w, rx) = watch(&root);

        // The app's save sequence: write, then tell the watcher right away.
        fs::write(&target, "saved by the app").unwrap();
        w.suppress(&target);
        if let Ok(paths) = rx.recv_timeout(DEBOUNCE + Duration::from_millis(300)) {
            panic!("a suppressed self-write was reported: {paths:?}");
        }

        // A genuine external change right after still comes through.
        std::thread::sleep(Duration::from_millis(20));
        fs::write(&target, "changed from outside").unwrap();
        let paths = rx.recv_timeout(Duration::from_secs(3)).expect("external change not reported");
        assert!(paths.contains(&target), "{paths:?}");
    }
}
