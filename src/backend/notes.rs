//! Free-text notes: a note is any `.md` file in the project, opened in the
//! same editor as any other file. This only covers listing them with a title
//! and creating one from a title.

use super::index::walk_files;
use super::project::{self, ext_lower, mtime_millis, Res};
use std::fs;
use std::path::{Path, PathBuf};
use crate::i18n::t;

/// Markdown that documents the project rather than notes about the work.
const NOT_NOTES: [&str; 3] = ["readme", "changelog", "license"];

#[derive(Clone, Debug)]
pub struct Note {
    pub title: String,
    pub path: PathBuf,
    pub mod_time: i64,
}

/// Every note's file, anywhere in the project.
pub fn files(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    walk_files(root, &mut |path, name| {
        let stem = name.rsplit_once('.').map_or(name, |(s, _)| s).to_lowercase();
        if ext_lower(name) == "md" && !NOT_NOTES.contains(&stem.as_str()) {
            out.push(path.to_path_buf());
        }
    });
    out
}

/// Every note, most recently modified first.
pub fn list(root: &Path) -> Res<Vec<Note>> {
    let mut notes: Vec<Note> = files(root).into_iter().map(|p| Note { title: title_of(&p), mod_time: mtime_millis(&p), path: p }).collect();
    notes.sort_by(|a, b| b.mod_time.cmp(&a.mod_time));
    Ok(notes)
}

/// A leading `# Heading` is the title; otherwise the file name.
fn title_of(path: &Path) -> String {
    let fallback = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    let Ok(text) = fs::read_to_string(path) else { return fallback };
    let Some(first) = text.lines().map(str::trim).find(|l| !l.is_empty()) else { return fallback };
    if first.starts_with('#') {
        let heading = first.trim_start_matches('#').trim();
        if !heading.is_empty() {
            return heading.to_string();
        }
    }
    fallback
}

/// Lowercase, underscore-separated, accents kept.
pub fn slugify(title: &str) -> String {
    let mut out = String::new();
    let mut gap = false;
    for c in title.trim().to_lowercase().chars() {
        if c.is_alphanumeric() {
            if gap && !out.is_empty() {
                out.push('_');
            }
            gap = false;
            out.push(c);
        } else {
            gap = true;
        }
    }
    if out.is_empty() {
        "nota".into()
    } else {
        out
    }
}

/// A new note in `folder` (relative to the root), named after its title.
pub fn create(root: &Path, folder: &Path, title: &str) -> Res<Note> {
    let title = if title.trim().is_empty() { t("Untitled note").to_string() } else { title.trim().to_string() };
    let dir = root.join(folder);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = project::create_file(root, &dir, &format!("{}.md", slugify(&title)))?;
    fs::write(&path, format!("# {title}\n\n")).map_err(|e| e.to_string())?;
    Ok(Note { title, mod_time: mtime_millis(&path), path })
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOTES: &str = "notes";

    #[test]
    fn slugs() {
        assert_eq!(slugify("Revisão do Capítulo 2!"), "revisão_do_capítulo_2");
        assert_eq!(slugify("  ?? "), "nota");
    }

    #[test]
    fn create_and_list() {
        let dir = tempfile::tempdir().unwrap();
        let n = create(dir.path(), Path::new(NOTES), "Ideias soltas").unwrap();
        assert!(n.path.ends_with("notes/ideias_soltas.md"));
        let l = list(dir.path()).unwrap();
        assert_eq!(l[0].title, "Ideias soltas");
        assert!(create(dir.path(), Path::new(NOTES), "Ideias soltas").is_err());
    }

    #[test]
    fn slugify_cases() {
        for (inp, want) in [
            ("Ideias para o capítulo 3", "ideias_para_o_capítulo_3"),
            ("  Espaços   nas bordas  ", "espaços_nas_bordas"),
            ("Dúvida: rever isso?!", "dúvida_rever_isso"),
            ("UPPERCASE TITLE", "uppercase_title"),
            ("...", "nota"),
            ("", "nota"),
        ] {
            assert_eq!(slugify(inp), want, "{inp:?}");
        }
    }

    #[test]
    fn list_without_folder_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(list(dir.path()).unwrap().is_empty());
    }

    #[test]
    fn create_seeds_the_heading() {
        let dir = tempfile::tempdir().unwrap();
        let n = create(dir.path(), Path::new(NOTES), "Ideias para o capítulo 3").unwrap();
        assert_eq!(n.title, "Ideias para o capítulo 3");
        assert_eq!(project::file_name(&n.path), "ideias_para_o_capítulo_3.md");
        assert_eq!(fs::read_to_string(&n.path).unwrap(), "# Ideias para o capítulo 3\n\n");
    }

    #[test]
    fn blank_title_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(create(dir.path(), Path::new(NOTES), "   ").unwrap().title, "Untitled note");
    }

    #[test]
    fn title_falls_back_to_the_file_name() {
        let dir = tempfile::tempdir().unwrap();
        let notes = dir.path().join(NOTES);
        fs::create_dir_all(&notes).unwrap();
        fs::write(notes.join("sem-titulo.md"), "só texto solto, sem heading\n").unwrap();
        let l = list(dir.path()).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].title, "sem-titulo");
    }

    #[test]
    fn notes_anywhere_but_not_project_docs() {
        let dir = tempfile::tempdir().unwrap();
        let r = dir.path();
        for f in ["notes/a.md", "cap1/rascunho.md", "README.md", "docs/CHANGELOG.md", "LICENSE.md", "main.tex"] {
            fs::create_dir_all(r.join(f).parent().unwrap()).unwrap();
            fs::write(r.join(f), "x").unwrap();
        }
        let mut names: Vec<String> = list(r).unwrap().into_iter().map(|n| project::file_name(&n.path)).collect();
        names.sort();
        assert_eq!(names, ["a.md", "rascunho.md"]);
    }

    #[test]
    fn create_in_the_configured_folder() {
        let dir = tempfile::tempdir().unwrap();
        let n = create(dir.path(), Path::new("diario/notas"), "Hoje").unwrap();
        assert_eq!(n.path, dir.path().join("diario/notas/hoje.md"));
    }

    #[test]
    fn most_recent_first() {
        let dir = tempfile::tempdir().unwrap();
        create(dir.path(), Path::new(NOTES), "Primeira").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        create(dir.path(), Path::new(NOTES), "Segunda").unwrap();
        let titles: Vec<String> = list(dir.path()).unwrap().into_iter().map(|n| n.title).collect();
        assert_eq!(titles, ["Segunda", "Primeira"]);
    }
}
