//! The PDFs read for the work: every PDF in the project except what it
//! produces itself (compiled documents, saved versions) and PDF figures. A
//! JSON sidecar at the root (`.papers.json`) links each, by its path, to a
//! `.bib` key and a summary.

use super::images::GraphicsInUse;
use super::index::{is_build_output, relative_slash, walk_files};
use super::project::{self, ext_lower, unique_path, Res};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use crate::tf;

const META_FILE: &str = ".papers.json";
/// Where the metadata lived when papers were only those in `papers/`,
/// keyed by file name.
const LEGACY_FOLDER: &str = "papers";
const LEGACY_META_FILE: &str = ".meta.json";

#[derive(Clone, Debug, Default)]
pub struct Paper {
    pub name: String,
    pub path: PathBuf,
    pub size: u64,
    pub cite_key: String,
    pub summary: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct Meta {
    #[serde(rename = "citeKey", default)]
    cite_key: String,
    #[serde(default)]
    summary: String,
}

/// A paper's key in the metadata: its path from the root, `/`-separated.
fn key(root: &Path, path: &Path) -> String {
    relative_slash(root, path)
}

fn read_json(path: &Path) -> Option<BTreeMap<String, Meta>> {
    fs::read_to_string(path).ok().and_then(|s| serde_json::from_str(&s).ok())
}

/// The metadata, the legacy file's brought over while the new one does
/// not exist yet (it is written on the first change).
fn read_meta(root: &Path) -> BTreeMap<String, Meta> {
    if let Some(m) = read_json(&root.join(META_FILE)) {
        return m;
    }
    read_json(&root.join(LEGACY_FOLDER).join(LEGACY_META_FILE))
        .unwrap_or_default()
        .into_iter()
        .map(|(name, m)| (format!("{LEGACY_FOLDER}/{name}"), m))
        .collect()
}

fn write_meta(root: &Path, m: &BTreeMap<String, Meta>) -> Res<()> {
    let data = serde_json::to_string_pretty(m).map_err(|e| e.to_string())?;
    fs::write(root.join(META_FILE), data).map_err(|e| e.to_string())?;
    let _ = fs::remove_file(root.join(LEGACY_FOLDER).join(LEGACY_META_FILE));
    Ok(())
}

/// Every paper in the project, by name. `versions` is the folder (relative
/// to the root) where PDF versions are saved: none of those is a paper.
pub fn list(root: &Path, main_file: Option<&Path>, versions: &Path) -> Res<Vec<Paper>> {
    let meta = read_meta(root);
    let versions = root.join(versions);
    let figures = GraphicsInUse::scan(root, main_file);
    let mut papers = Vec::new();
    walk_files(root, &mut |path, name| {
        if ext_lower(name) != "pdf" || is_build_output(path) || path.starts_with(&versions) || figures.contains(path) {
            return;
        }
        let m = meta.get(&key(root, path)).cloned().unwrap_or_default();
        papers.push(Paper {
            name: name.to_string(),
            path: path.to_path_buf(),
            size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            cite_key: m.cite_key,
            summary: m.summary,
        });
    });
    papers.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.path.cmp(&b.path)));
    Ok(papers)
}

/// Copies each PDF into `folder` (relative to the root), never overwriting.
/// A PDF already inside the project stays where it is.
pub fn add(root: &Path, folder: &Path, sources: &[PathBuf]) -> Res<Vec<Paper>> {
    let dir = root.join(folder);
    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let mut added = Vec::new();
    for src in sources {
        if ext_lower(src) != "pdf" {
            return Err(tf!("{name} is not a PDF", name = project::file_name(src)));
        }
        let src_abs = fs::canonicalize(src).map_err(|e| e.to_string())?;
        let path = match src_abs.strip_prefix(&root_abs) {
            Ok(rel) => root.join(rel),
            Err(_) => {
                fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
                let dest = unique_path(&dir.join(src.file_name().unwrap_or_default()));
                fs::copy(src, &dest).map_err(|e| e.to_string())?;
                dest
            }
        };
        added.push(Paper {
            name: project::file_name(&path),
            size: fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            path,
            ..Default::default()
        });
    }
    Ok(added)
}

pub fn set_meta(root: &Path, path: &Path, cite_key: &str, summary: &str) -> Res<()> {
    if !path.is_file() {
        return Err(tf!("paper not found: {name}", name = project::file_name(path)));
    }
    let mut m = read_meta(root);
    m.insert(key(root, path), Meta { cite_key: cite_key.trim().into(), summary: summary.trim().into() });
    write_meta(root, &m)
}

/// Deletes the PDF and its metadata together.
pub fn remove(root: &Path, path: &Path) -> Res<()> {
    project::delete(root, path)?;
    let mut m = read_meta(root);
    if m.remove(&key(root, path)).is_some() {
        write_meta(root, &m)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAPERS: &str = "papers";
    const VERSIONS: &str = "versions";

    fn fake_pdf(dir: &Path, name: &str) -> PathBuf {
        let p = dir.join(name);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(&p, "%PDF-1.4 fake for tests\n").unwrap();
        p
    }

    fn list_all(root: &Path) -> Vec<Paper> {
        list(root, None, Path::new(VERSIONS)).unwrap()
    }

    fn add_one(root: &Path, src: PathBuf) -> Paper {
        add(root, Path::new(PAPERS), &[src]).unwrap().remove(0)
    }

    #[test]
    fn empty_project_has_no_papers() {
        let root = tempfile::tempdir().unwrap();
        assert!(list_all(root.path()).is_empty());
    }

    #[test]
    fn add_copies_and_lists() {
        let (root, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let pdf = fake_pdf(src.path(), "cookbook.pdf");
        let added = add_one(root.path(), pdf.clone());
        assert_eq!(added.name, "cookbook.pdf");
        assert_eq!(added.path, root.path().join("papers/cookbook.pdf"), "copied into the folder, not pointing at the source");
        let l = list_all(root.path());
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].name, "cookbook.pdf");
    }

    #[test]
    fn add_goes_to_the_configured_folder() {
        let (root, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let added = add(root.path(), Path::new("leituras/2026"), &[fake_pdf(src.path(), "a.pdf")]).unwrap();
        assert_eq!(added[0].path, root.path().join("leituras/2026/a.pdf"));
    }

    #[test]
    fn add_keeps_a_pdf_already_in_the_project_in_place() {
        let root = tempfile::tempdir().unwrap();
        let inside = fake_pdf(root.path(), "refs/x.pdf");
        let added = add_one(root.path(), inside.clone());
        assert_eq!(added.path, inside);
        assert!(!root.path().join("papers").exists());
    }

    #[test]
    fn lists_pdfs_anywhere_but_not_outputs_versions_or_figures() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        fake_pdf(r, "papers/a.pdf");
        fake_pdf(r, "leituras/b.pdf");
        fake_pdf(r, "c.pdf");
        // What the project makes or includes.
        fs::write(r.join("main.tex"), "\\includegraphics{fig/plot}\n").unwrap();
        fake_pdf(r, "main.pdf");
        fake_pdf(r, "versions/main_2026-10-02.pdf");
        fake_pdf(r, "fig/plot.pdf");
        let names: Vec<String> = list(r, Some(&r.join("main.tex")), Path::new(VERSIONS)).unwrap().into_iter().map(|p| p.name).collect();
        assert_eq!(names, ["a.pdf", "b.pdf", "c.pdf"]);
    }

    #[test]
    fn add_rejects_non_pdf() {
        let (root, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let txt = src.path().join("notes.txt");
        fs::write(&txt, "not a pdf").unwrap();
        assert!(add(root.path(), Path::new(PAPERS), &[txt]).is_err());
    }

    #[test]
    fn add_does_not_overwrite_same_name() {
        let root = tempfile::tempdir().unwrap();
        let (s1, s2) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        add_one(root.path(), fake_pdf(s1.path(), "same.pdf"));
        let second = add_one(root.path(), fake_pdf(s2.path(), "same.pdf"));
        assert_ne!(second.name, "same.pdf");
        assert_eq!(list_all(root.path()).len(), 2);
    }

    #[test]
    fn meta_round_trips_through_list() {
        let (root, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let p = add_one(root.path(), fake_pdf(src.path(), "paper.pdf"));
        set_meta(root.path(), &p.path, "vaswani2017", "Introduz o Transformer.").unwrap();
        let l = list_all(root.path());
        assert_eq!(l[0].cite_key, "vaswani2017");
        assert_eq!(l[0].summary, "Introduz o Transformer.");
    }

    #[test]
    fn same_name_in_two_folders_keeps_separate_meta() {
        let root = tempfile::tempdir().unwrap();
        let (a, b) = (fake_pdf(root.path(), "x/same.pdf"), fake_pdf(root.path(), "y/same.pdf"));
        set_meta(root.path(), &a, "a2020", "").unwrap();
        set_meta(root.path(), &b, "b2021", "").unwrap();
        let keys: Vec<String> = list_all(root.path()).into_iter().map(|p| p.cite_key).collect();
        assert_eq!(keys, ["a2020", "b2021"]);
    }

    #[test]
    fn legacy_meta_is_carried_over() {
        let root = tempfile::tempdir().unwrap();
        let r = root.path();
        let a = fake_pdf(r, "papers/old.pdf");
        fake_pdf(r, "papers/other.pdf");
        fs::write(r.join("papers/.meta.json"), r#"{"old.pdf": {"citeKey": "old1999", "summary": "antigo"}}"#).unwrap();
        assert_eq!(list_all(r)[0].cite_key, "old1999");
        // The first change writes the new file and drops the old one.
        set_meta(r, &r.join("papers/other.pdf"), "other2000", "").unwrap();
        assert!(!r.join("papers/.meta.json").exists());
        let l = list_all(r);
        assert_eq!((l[0].path.clone(), l[0].cite_key.as_str()), (a, "old1999"));
        assert_eq!(l[1].cite_key, "other2000");
    }

    #[test]
    fn meta_of_unknown_paper_fails() {
        let root = tempfile::tempdir().unwrap();
        assert!(set_meta(root.path(), &root.path().join("nope.pdf"), "x", "y").is_err());
    }

    #[test]
    fn remove_deletes_file_and_meta() {
        let (root, src) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let p = add_one(root.path(), fake_pdf(src.path(), "gone.pdf"));
        set_meta(root.path(), &p.path, "key2020", "resumo").unwrap();
        remove(root.path(), &p.path).unwrap();
        assert!(list_all(root.path()).is_empty());
        assert!(!read_meta(root.path()).contains_key("papers/gone.pdf"));
    }
}
