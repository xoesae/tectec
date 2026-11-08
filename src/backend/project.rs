//! The open project: the file tree and every disk operation.
//!
//! **Every** function that reads, writes or deletes takes the project root and
//! runs the path through [`resolve`] before touching anything, so a path coming
//! from a log, a SyncTeX map or the UI can never escape the project.
//!
//! Error messages are in Portuguese on purpose: they end up on the status bar.

use std::fs;
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::time::UNIX_EPOCH;
use crate::i18n::t;
use crate::tf;

pub type Res<T> = Result<T, String>;

/// Classifies a file so the UI can pick the right viewer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Dir,
    Tex,
    Image,
    Svg,
    Pdf,
    Text,
    Binary,
    Other,
}

impl Kind {
    /// Whether this kind opens in the text editor by default.
    pub fn editable(self) -> bool {
        matches!(self, Kind::Tex | Kind::Text | Kind::Other)
    }
}

pub fn kind_of(name: &str, is_dir: bool) -> Kind {
    if is_dir {
        return Kind::Dir;
    }
    match ext_lower(name).as_str() {
        "tex" | "sty" | "cls" | "bib" => Kind::Tex,
        "png" | "jpg" | "jpeg" | "gif" | "webp" | "bmp" => Kind::Image,
        "svg" => Kind::Svg,
        "pdf" => Kind::Pdf,
        "md" | "txt" | "log" | "json" | "yaml" | "yml" | "toml" | "csv" => Kind::Text,
        _ => Kind::Other,
    }
}

/// Whether the file can be the document's main file: a LaTeX source.
pub fn is_document_source(path: &Path) -> bool {
    ext_lower(path) == "tex"
}

/// Lowercased extension without the dot ("" when there is none).
pub fn ext_lower(name: impl AsRef<Path>) -> String {
    name.as_ref()
        .extension()
        .map(|e| e.to_string_lossy().to_lowercase())
        .unwrap_or_default()
}

pub fn file_name(path: &Path) -> String {
    path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default()
}

const BINARY_SNIFF_BYTES: usize = 8192;

/// Sniffs the first few KB for a null byte: the heuristic git and grep use.
pub fn looks_binary(path: &Path) -> bool {
    let Ok(mut f) = fs::File::open(path) else { return false };
    let mut buf = vec![0u8; BINARY_SNIFF_BYTES];
    let n = f.read(&mut buf).unwrap_or(0);
    buf[..n].contains(&0)
}

fn file_kind(path: &Path, name: &str) -> Kind {
    let kind = kind_of(name, false);
    if kind == Kind::Other && looks_binary(path) {
        return Kind::Binary;
    }
    kind
}

/// Lexically normalizes a path (`a/./b/../c` → `a/c`) without touching disk.
pub fn clean(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for comp in path.components() {
        match comp {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

fn resolved_root(root: &Path) -> Res<PathBuf> {
    let abs = std::path::absolute(root).map_err(|e| e.to_string())?;
    Ok(fs::canonicalize(&abs).unwrap_or(abs))
}

/// Returns the absolute path of `raw` after confirming it lives inside `root`.
pub fn resolve(root: &Path, raw: &Path) -> Res<PathBuf> {
    if raw.as_os_str().is_empty() {
        return Err("caminho vazio".into());
    }
    if root.as_os_str().is_empty() {
        return Err("nenhum projeto aberto".into());
    }
    let root_abs = resolved_root(root)?;
    let mut abs = clean(&std::path::absolute(raw).map_err(|e| e.to_string())?);
    if let Ok(real) = fs::canonicalize(&abs) {
        abs = real;
    } else if let (Some(parent), Some(name)) = (abs.parent(), abs.file_name()) {
        // Does not exist yet (creating or renaming): validate its folder.
        if let Ok(real_parent) = fs::canonicalize(parent) {
            abs = real_parent.join(name);
        }
    }
    if !abs.starts_with(&root_abs) {
        return Err("caminho fora do projeto".into());
    }
    Ok(abs)
}

pub fn validate_name(name: &str) -> Res<String> {
    let name = name.trim();
    if name.is_empty() {
        return Err(t("the name cannot be empty").into());
    }
    if name == "." || name == ".." || name.contains('/') || name.contains('\\') {
        return Err(t("the name cannot contain slashes").into());
    }
    Ok(name.to_string())
}

/// What LaTeX leaves next to the source: hidden from the tree, never deleted.
/// Matched against the whole lowercased name: `main.synctex.gz`'s extension
/// is `.gz`.
const BUILD_ARTIFACTS: &[&str] = &[
    ".aux", ".log", ".out", ".toc", ".lof", ".lot", ".synctex.gz", ".synctex", ".synctex(busy)", ".fls",
    ".fdb_latexmk", ".xdv", ".bbl", ".blg", ".bcf", ".run.xml", ".idx", ".ilg", ".ind", ".nav", ".snm", ".vrb",
    // backref (hyperref), glossaries, nomencl, lists of algorithms and listings.
    ".brf", ".glo", ".gls", ".glg", ".nlo", ".nls", ".acn", ".acr", ".alg", ".loa", ".lol",
];

pub fn is_build_artifact(name: &str) -> bool {
    let lower = name.to_lowercase();
    BUILD_ARTIFACTS.iter().any(|s| lower.len() > s.len() && lower.ends_with(s))
}

#[derive(Clone, Debug)]
pub struct Node {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub kind: Kind,
    pub children: Vec<Node>,
}

impl Node {
    pub fn find(&self, path: &Path) -> Option<&Node> {
        if self.path == path {
            return Some(self);
        }
        if !self.is_dir || !path.starts_with(&self.path) {
            return None;
        }
        self.children.iter().find_map(|c| c.find(path))
    }

    /// Every file under this node, depth first.
    pub fn files(&self, out: &mut Vec<Node>) {
        for c in &self.children {
            if c.is_dir {
                c.files(out);
            } else {
                out.push(Node { children: vec![], ..c.clone() });
            }
        }
    }
}

/// Walks an already resolved path into a tree: folders first, then
/// alphabetical; hidden entries, build artifacts and the PDF compiled from a
/// .tex beside it skipped (the preview shows that one).
pub fn build(root: &Path) -> Res<Node> {
    let meta = fs::metadata(root).map_err(|e| e.to_string())?;
    let name = file_name(root);
    if !meta.is_dir() {
        return Ok(Node { kind: file_kind(root, &name), name, path: root.to_path_buf(), is_dir: false, children: vec![] });
    }
    let mut entries: Vec<(String, PathBuf, bool)> = fs::read_dir(root)
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok())
        .map(|e| {
            let path = e.path();
            let is_dir = path.is_dir();
            (e.file_name().to_string_lossy().into_owned(), path, is_dir)
        })
        .collect();
    entries.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase())));

    let sources: std::collections::HashSet<String> =
        entries.iter().filter(|(n, _, d)| !d && ext_lower(n) == "tex").map(|(n, _, _)| n[..n.len() - 4].to_lowercase()).collect();
    let compiled = |name: &str| ext_lower(name) == "pdf" && sources.contains(&name[..name.len() - 4].to_lowercase());
    let mut children = Vec::new();
    for (name, path, is_dir) in entries {
        if name.starts_with('.') || (!is_dir && (is_build_artifact(&name) || compiled(&name))) {
            continue;
        }
        if let Ok(child) = build(&path) {
            children.push(child);
        }
    }
    Ok(Node { name, path: root.to_path_buf(), is_dir: true, kind: Kind::Dir, children })
}

/// The first `main.tex`/`index.tex`, breadth first. Otherwise
/// nothing: the first source the user opens becomes the main file.
pub fn guess_main_file(tree: &Node) -> Option<PathBuf> {
    let mut queue: std::collections::VecDeque<&Node> = tree.children.iter().collect();
    while let Some(node) = queue.pop_front() {
        let lower = node.name.to_lowercase();
        if !node.is_dir && matches!(lower.as_str(), "main.tex" | "index.tex") {
            return Some(node.path.clone());
        }
        if node.is_dir {
            queue.extend(node.children.iter());
        }
    }
    None
}

pub fn mtime_millis(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub struct ReadResult {
    pub content: String,
    pub mod_time: i64,
}

/// Text contents of a file in the project. Refuses on content, not on
/// extension: a null byte near the front means it is not text.
pub fn read(root: &Path, path: &Path) -> Res<ReadResult> {
    let abs = resolve(root, path)?;
    if looks_binary(&abs) {
        return Err(tf!("{name} is a binary file and cannot be opened as text", name = file_name(&abs)));
    }
    let bytes = fs::read(&abs).map_err(|e| e.to_string())?;
    Ok(ReadResult { content: String::from_utf8_lossy(&bytes).into_owned(), mod_time: mtime_millis(&abs) })
}

pub struct WriteResult {
    pub mod_time: i64,
    pub conflict: bool,
}

/// Saves text, refusing when the file changed on disk since
/// `expected_mod_time` (0 skips the check).
pub fn write(root: &Path, path: &Path, content: &str, expected_mod_time: i64) -> Res<WriteResult> {
    let abs = resolve(root, path)?;
    if expected_mod_time != 0 && abs.exists() {
        let current = mtime_millis(&abs);
        if current != expected_mod_time {
            return Ok(WriteResult { mod_time: current, conflict: true });
        }
    }
    atomic_write(&abs, content.as_bytes())?;
    Ok(WriteResult { mod_time: mtime_millis(&abs), conflict: false })
}

/// Writes a file so a crash or a full disk mid-write never leaves it
/// truncated: the bytes go to a temporary file beside it, are flushed to
/// disk, and only then replace the original in one rename. Keeps the
/// original's permissions.
pub fn atomic_write(path: &Path, data: &[u8]) -> Res<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    let name = file_name(path);
    let tmp = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| -> std::io::Result<()> {
        let mut f = fs::File::create(&tmp)?;
        f.write_all(data)?;
        f.sync_all()?;
        if let Ok(meta) = fs::metadata(path) {
            fs::set_permissions(&tmp, meta.permissions())?;
        }
        fs::rename(&tmp, path)?;
        // The rename itself must reach the disk too.
        #[cfg(unix)]
        if let Ok(d) = fs::File::open(dir) {
            let _ = d.sync_all();
        }
        Ok(())
    })();
    if let Err(e) = result {
        let _ = fs::remove_file(&tmp);
        return Err(tf!("could not write {name}: {e}", name, e));
    }
    Ok(())
}

/// Copies the PDF into `folder` (relative to the root), named after its base
/// name, today's date and an optional label. Never overwrites an existing
/// snapshot.
pub fn export_version(root: &Path, folder: &Path, pdf_path: &Path, label: &str) -> Res<PathBuf> {
    let abs_pdf = resolve(root, pdf_path)?;
    match fs::metadata(&abs_pdf) {
        Ok(m) if m.len() > 0 => {}
        _ => return Err(t("compile the document before saving a version").into()),
    }
    let dir = resolved_root(root)?.join(folder);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let base = abs_pdf.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let mut name = format!("{}_{}", base, chrono::Local::now().format("%Y-%m-%d"));
    let label = sanitize_label(label);
    if !label.is_empty() {
        name.push('_');
        name.push_str(&label);
    }
    let dest = unique_path(&dir.join(format!("{name}.pdf")));
    fs::copy(&abs_pdf, &dest).map_err(|e| e.to_string())?;
    Ok(dest)
}

fn sanitize_label(label: &str) -> String {
    label.chars().map(|c| if matches!(c, '/' | '\\' | ':') { '-' } else { c }).collect::<String>().trim().to_string()
}

/// Appends " (2)", " (3)"… before the extension until the name is free.
pub fn unique_path(path: &Path) -> PathBuf {
    if !path.exists() {
        return path.to_path_buf();
    }
    let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|e| format!(".{}", e.to_string_lossy())).unwrap_or_default();
    let dir = path.parent().unwrap_or(Path::new(""));
    (2..)
        .map(|i| dir.join(format!("{stem} ({i}){ext}")))
        .find(|p| !p.exists())
        .unwrap()
}

/// Creates a file inside a folder; with no extension it becomes a `.tex`,
/// and only a `.tex` gets a skeleton.
pub fn create_file(root: &Path, dir: &Path, name: &str) -> Res<PathBuf> {
    let dir_abs = resolve(root, dir)?;
    let mut name = validate_name(name)?;
    if Path::new(&name).extension().is_none() {
        name.push_str(".tex");
    }
    let path = dir_abs.join(&name);
    if path.exists() {
        return Err(tf!("a file named {name} already exists", name));
    }
    let content = if ext_lower(&name) == "tex" {
        "\\documentclass{article}\n\\begin{document}\n\n\\end{document}\n"
    } else {
        ""
    };
    fs::write(&path, content).map_err(|e| e.to_string())?;
    Ok(path)
}

pub fn create_folder(root: &Path, dir: &Path, name: &str) -> Res<PathBuf> {
    let dir_abs = resolve(root, dir)?;
    let name = validate_name(name)?;
    let path = dir_abs.join(&name);
    if path.exists() {
        return Err(tf!("something named {name} already exists here", name));
    }
    fs::create_dir(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Renames in place and returns the new path. Never overwrites.
pub fn rename(root: &Path, path: &Path, new_name: &str) -> Res<PathBuf> {
    let abs = resolve(root, path)?;
    if abs == resolved_root(root)? {
        return Err(t("the project root cannot be renamed").into());
    }
    let new_name = validate_name(new_name)?;
    let target = abs.parent().unwrap_or(Path::new("/")).join(&new_name);
    if target == abs {
        return Ok(abs);
    }
    if target.exists() {
        return Err(tf!("something named {name} already exists here", name = new_name));
    }
    fs::rename(&abs, &target).map_err(|e| e.to_string())?;
    Ok(target)
}

/// Removes a file or a whole folder. Confirming is the UI's job.
pub fn delete(root: &Path, path: &Path) -> Res<()> {
    let abs = resolve(root, path)?;
    if abs == resolved_root(root)? {
        return Err(t("the project root cannot be deleted").into());
    }
    if abs.is_dir() {
        fs::remove_dir_all(&abs).map_err(|e| e.to_string())
    } else {
        fs::remove_file(&abs).map_err(|e| e.to_string())
    }
}

/// Opens the folder holding the path in the system file manager.
pub fn reveal(root: &Path, path: &Path) -> Res<()> {
    let abs = resolve(root, path)?;
    let mut cmd = if cfg!(target_os = "macos") {
        let mut c = Command::new("open");
        c.arg("-R").arg(&abs);
        c
    } else if cfg!(target_os = "windows") {
        let mut c = Command::new("explorer");
        c.arg(format!("/select,{}", abs.display()));
        c
    } else {
        let dir = if abs.is_dir() { abs.clone() } else { abs.parent().unwrap_or(&abs).to_path_buf() };
        let mut c = Command::new("xdg-open");
        c.arg(dir);
        c
    };
    cmd.spawn().map(|_| ()).map_err(|e| e.to_string())
}

/// Path relative to the project root, with forward slashes, for display.
pub fn relative(path: &Path, root: Option<&Path>) -> String {
    match root.and_then(|r| path.strip_prefix(r).ok()) {
        Some(rel) if !rel.as_os_str().is_empty() => rel.to_string_lossy().replace('\\', "/"),
        _ => file_name(path),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_refuses_escape() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::write(root.join("a.tex"), "x").unwrap();
        assert!(resolve(root, &root.join("a.tex")).is_ok());
        assert!(resolve(root, &root.join("new.tex")).is_ok());
        assert!(resolve(root, &root.join("../etc/passwd")).is_err());
        assert!(resolve(root, Path::new("")).is_err());
    }

    #[test]
    fn artifacts_and_kinds() {
        assert!(is_build_artifact("main.synctex.gz"));
        assert!(is_build_artifact("main.AUX"));
        assert!(!is_build_artifact(".log"));
        assert_eq!(kind_of("a.TEX", false), Kind::Tex);
        assert_eq!(kind_of("a.svg", false), Kind::Svg);
    }

    #[test]
    fn write_detects_conflict() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let file = root.join("a.tex");
        fs::write(&file, "one").unwrap();
        let r = read(root, &file).unwrap();
        let w = write(root, &file, "two", r.mod_time).unwrap();
        assert!(!w.conflict);
        let w = write(root, &file, "three", w.mod_time - 1000).unwrap();
        assert!(w.conflict);
        assert_eq!(fs::read_to_string(&file).unwrap(), "two");
    }

    #[test]
    fn create_rename_delete() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let f = create_file(root, root, "cap").unwrap();
        assert!(f.ends_with("cap.tex"));
        assert!(fs::read_to_string(&f).unwrap().contains("documentclass"));
        assert!(create_file(root, root, "cap.tex").is_err());
        let g = rename(root, &f, "dois.tex").unwrap();
        assert!(g.exists() && !f.exists());
        assert!(delete(root, root).is_err());
        delete(root, &g).unwrap();
        assert!(!g.exists());
    }

    #[test]
    fn unique_paths() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("x.pdf");
        fs::write(&p, "a").unwrap();
        assert_eq!(unique_path(&p), dir.path().join("x (2).pdf"));
    }

    /// A throwaway root, canonicalized so it compares with what `resolve`
    /// returns (/tmp can be a symlink).
    fn test_root() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        (dir, root)
    }

    fn kinds(tree: &Node) -> std::collections::HashMap<String, Kind> {
        tree.children.iter().map(|c| (c.name.clone(), c.kind)).collect()
    }

    #[test]
    fn resolve_rejects_paths_outside() {
        let (_d, root) = test_root();
        let parent = root.parent().unwrap();
        let outside = parent.join(format!("{}-neighbour.tex", file_name(&root)));
        fs::write(&outside, "x").unwrap();
        for c in [
            outside.clone(),
            root.join("..").join(file_name(&outside)),
            parent.join(format!("{}-other", file_name(&root))),
            PathBuf::from("/etc/passwd"),
            PathBuf::new(),
        ] {
            assert!(resolve(&root, &c).is_err(), "{c:?} passed");
        }
        fs::remove_file(outside).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn resolve_rejects_escaping_symlink() {
        let (_d, root) = test_root();
        let other = tempfile::tempdir().unwrap();
        let target = other.path().join("secret.txt");
        fs::write(&target, "x").unwrap();
        let link = root.join("shortcut.txt");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        assert!(resolve(&root, &link).is_err());
    }

    #[test]
    fn resolve_accepts_a_path_that_does_not_exist_yet() {
        let (_d, root) = test_root();
        assert!(resolve(&root, &root.join("does-not-exist-yet.tex")).is_ok());
    }

    #[test]
    fn validate_name_rejects_escape() {
        for name in ["", "   ", ".", "..", "a/b", "a\\b", "../outside.tex"] {
            assert!(validate_name(name).is_err(), "{name:?}");
        }
        assert_eq!(validate_name("  cap1.tex  ").unwrap(), "cap1.tex");
    }

    #[test]
    fn create_file_skeleton_only_for_tex() {
        let (_d, root) = test_root();
        let bib = create_file(&root, &root, "refs.bib").unwrap();
        assert_eq!(fs::read_to_string(bib).unwrap(), "");
    }

    #[test]
    fn rename_does_not_overwrite() {
        let (_d, root) = test_root();
        let (a, b) = (root.join("a.tex"), root.join("b.tex"));
        fs::write(&a, "contents of A").unwrap();
        fs::write(&b, "contents of B").unwrap();
        assert!(rename(&root, &a, "b.tex").is_err());
        assert_eq!(fs::read_to_string(&b).unwrap(), "contents of B");
        assert!(rename(&root, &root, "other").is_err(), "the root cannot be renamed");
        assert!(rename(&root, &a, "../escaped.tex").is_err());
        let moved = rename(&root, &a, "c.tex").unwrap();
        assert_eq!(file_name(&moved), "c.tex");
        assert!(!a.exists());
    }

    #[test]
    fn delete_rejects_root_and_outside() {
        let (_d, root) = test_root();
        assert!(delete(&root, &root).is_err());
        assert!(root.exists());
        let other = tempfile::tempdir().unwrap();
        let outside = other.path().join("important.txt");
        fs::write(&outside, "x").unwrap();
        assert!(delete(&root, &outside).is_err());
        assert!(outside.exists());
    }

    #[test]
    fn delete_removes_a_whole_folder() {
        let (_d, root) = test_root();
        fs::create_dir(root.join("cap")).unwrap();
        fs::write(root.join("cap/one.tex"), "x").unwrap();
        delete(&root, &root.join("cap")).unwrap();
        assert!(!root.join("cap").exists());
    }

    /// With no root at all, every operation dies at the door.
    #[test]
    fn nothing_passes_without_an_open_project() {
        let none = Path::new("");
        let target = Path::new("/etc/passwd");
        assert!(read(none, target).is_err());
        assert!(delete(none, target).is_err());
        assert!(write(none, target, "x", 0).is_err());
    }

    #[test]
    fn build_artifact_names() {
        for name in [
            "main.aux", "main.log", "main.synctex.gz", "main.synctex", "main.fdb_latexmk", "main.bbl", "main.blg", "main.brf",
            "main.run.xml", "main.nav", "main.snm", "main.vrb", "MAIN.LOG", "cap1.toc", "main.synctex(busy)",
        ] {
            assert!(is_build_artifact(name), "{name}");
        }
        // Names that only look like artifacts; a bare suffix is a hidden file.
        for name in ["main.tex", "refs.bib", "figura.png", "notas.md", "leiame.txt", "log", "changelog.md", "aux.tex", ".log", ".aux"] {
            assert!(!is_build_artifact(name), "{name}");
        }
    }

    #[test]
    fn build_skips_hidden_files_and_artifacts() {
        let (_d, root) = test_root();
        for name in ["main.tex", "main.aux", "main.log", "main.synctex.gz", ".hidden", "main.pdf", "artigo.pdf"] {
            fs::write(root.join(name), "x").unwrap();
        }
        fs::create_dir(root.join("fig")).unwrap();
        let tree = build(&root).unwrap();
        let k = kinds(&tree);
        assert_eq!(k.get("main.tex"), Some(&Kind::Tex));
        // A PDF with no source beside it is the user's.
        assert!(k.contains_key("artigo.pdf"));
        for name in ["main.aux", "main.log", "main.synctex.gz", ".hidden", "main.pdf"] {
            assert!(!k.contains_key(name), "{name} in the tree");
        }
        assert!(tree.children[0].name == "fig" && tree.children[0].is_dir, "folders come first");
    }

    /// An unrecognized extension used to open as text and get written back
    /// mangled on the next autosave; classifying by content stops that.
    #[test]
    fn build_classifies_binary_by_content() {
        let (_d, root) = test_root();
        let mut font = vec![0x00, 0x01, 0x00, 0x00, b'O', b'T', b'T', b'O'];
        font.extend([0u8; 100]);
        fs::write(root.join("figure.eps"), b"%!PS-Adobe\x00binary tail\x00").unwrap();
        fs::write(root.join("font.ttf"), font).unwrap();
        fs::write(root.join("config.custom"), "key = value\nother = 1\n").unwrap();
        // A recognized extension is trusted without sniffing.
        fs::write(root.join("broken.png"), [0u8, 1, 2]).unwrap();
        let k = kinds(&build(&root).unwrap());
        assert_eq!(k["figure.eps"], Kind::Binary);
        assert_eq!(k["font.ttf"], Kind::Binary);
        assert_eq!(k["config.custom"], Kind::Other);
        assert_eq!(k["broken.png"], Kind::Image);
    }

    #[test]
    fn read_refuses_binary_without_touching_it() {
        let (_d, root) = test_root();
        let original = b"\x89PNG\x00\r\ngarbage".to_vec();
        let path = root.join("weird.dat");
        fs::write(&path, &original).unwrap();
        assert!(read(&root, &path).is_err());
        assert_eq!(fs::read(&path).unwrap(), original);
    }

    #[test]
    fn read_returns_mod_time() {
        let (_d, root) = test_root();
        let path = root.join("a.tex");
        fs::write(&path, "hello").unwrap();
        let r = read(&root, &path).unwrap();
        assert_eq!(r.content, "hello");
        assert_ne!(r.mod_time, 0);
    }

    /// Something outside the app changes the file between a read and the
    /// next write: refuse instead of guessing, then let "overwrite anyway"
    /// through with the reported mtime.
    #[test]
    fn write_detects_external_conflict() {
        let (_d, root) = test_root();
        let path = root.join("a.tex");
        fs::write(&path, "original").unwrap();
        let r = read(&root, &path).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        fs::write(&path, "changed from outside").unwrap();
        let w = write(&root, &path, "edited in the app", r.mod_time).unwrap();
        assert!(w.conflict);
        assert_eq!(fs::read_to_string(&path).unwrap(), "changed from outside");
        let w2 = write(&root, &path, "edited in the app", w.mod_time).unwrap();
        assert!(!w2.conflict);
        assert_eq!(fs::read_to_string(&path).unwrap(), "edited in the app");
        // 0 means "never read": no check.
        assert!(!write(&root, &path, "again", 0).unwrap().conflict);
    }

    #[test]
    fn export_version_copies_without_touching_the_source() {
        let (_d, root) = test_root();
        let pdf = root.join("tese.pdf");
        fs::write(&pdf, "%PDF-1.5 fake content").unwrap();
        let dest = export_version(&root, Path::new("versions"), &pdf, "revisão orientador").unwrap();
        assert_eq!(dest.parent().unwrap(), root.join("versions"));
        let name = file_name(&dest);
        assert!(name.starts_with("tese_") && name.contains("revisão orientador"), "{name}");
        assert_eq!(fs::read_to_string(&dest).unwrap(), "%PDF-1.5 fake content");
        assert_eq!(fs::read_to_string(&pdf).unwrap(), "%PDF-1.5 fake content");
    }

    #[test]
    fn export_version_never_overwrites() {
        let (_d, root) = test_root();
        let pdf = root.join("tese.pdf");
        fs::write(&pdf, "v1").unwrap();
        let first = export_version(&root, Path::new("versions"), &pdf, "").unwrap();
        fs::write(&pdf, "v2").unwrap();
        let second = export_version(&root, Path::new("versions"), &pdf, "").unwrap();
        assert_ne!(first, second);
        assert_eq!(fs::read_to_string(first).unwrap(), "v1");
        assert_eq!(fs::read_to_string(second).unwrap(), "v2");
    }

    #[test]
    fn export_version_refuses_missing_or_empty_pdf() {
        let (_d, root) = test_root();
        assert!(export_version(&root, Path::new("versions"), &root.join("nope.pdf"), "").is_err());
        fs::write(root.join("empty.pdf"), "").unwrap();
        assert!(export_version(&root, Path::new("versions"), &root.join("empty.pdf"), "").is_err());
    }

    #[test]
    fn atomic_write_replaces_whole_and_leaves_no_temp() {
        let (_d, root) = test_root();
        let path = root.join("a.tex");
        fs::write(&path, "antigo").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        }
        atomic_write(&path, "novo conteúdo".as_bytes()).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "novo conteúdo");
        let leftovers: Vec<_> = fs::read_dir(&root).unwrap().flatten().filter(|e| e.file_name().to_string_lossy().ends_with(".tmp")).collect();
        assert!(leftovers.is_empty());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o640);
        }
        // A folder that does not exist: an error, and nothing left behind.
        assert!(atomic_write(&root.join("nao/existe.tex"), b"x").is_err());
    }
}
