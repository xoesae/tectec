//! Pending work from two places: `% TODO:` comments in the LaTeX source and
//! unchecked checkboxes (`- [ ]`) in the project's notes, wherever they are.

use super::index::walk_files;
use super::project::ext_lower;
use regex::Regex;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

#[derive(Clone, Debug)]
pub struct Entry {
    pub text: String,
    /// true for a `.tex` comment, false for a note checkbox.
    pub from_code: bool,
    pub file: PathBuf,
    pub line: usize,
}

static CODE_TODO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"%\s*TODO:?\s*(.+)").unwrap());
static NOTE_TODO_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*[-*]\s*\[ \]\s*(.+)").unwrap());

pub fn build(root: &Path) -> Vec<Entry> {
    let mut entries = Vec::new();
    walk_files(root, &mut |path, name| {
        if matches!(ext_lower(name).as_str(), "tex" | "sty" | "cls") {
            entries.extend(scan(path, &CODE_TODO_RE, true));
        }
    });
    for note in super::notes::files(root) {
        entries.extend(scan(&note, &NOTE_TODO_RE, false));
    }
    entries.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    entries
}

fn scan(path: &Path, re: &Regex, from_code: bool) -> Vec<Entry> {
    let Ok(text) = fs::read_to_string(path) else { return vec![] };
    text.lines()
        .enumerate()
        .filter_map(|(i, line)| {
            let m = re.captures(line)?;
            let t = m[1].trim();
            (!t.is_empty()).then(|| Entry { text: t.to_string(), from_code, file: path.to_path_buf(), line: i + 1 })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn code_and_notes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(
            &root.join("main.tex"),
            "\\documentclass{article}\n% TODO: revisar a introdução\n\\begin{document}\nTexto normal.\n\
             %TODO sem dois pontos também conta\n\\end{document}\n",
        );
        write(
            &root.join("notes/ideias.md"),
            "# Ideias\n\n- [ ] escrever a conclusão\n- [x] já fiz essa parte\nTexto solto não é um todo.\n",
        );
        let entries = build(root);
        assert_eq!(entries.len(), 3, "{entries:?}");
        let code: Vec<_> = entries.iter().filter(|e| e.from_code).collect();
        let notes: Vec<_> = entries.iter().filter(|e| !e.from_code).collect();
        assert_eq!(code.len(), 2);
        assert_eq!(code[0].text, "revisar a introdução");
        assert_eq!(code[0].line, 2);
        assert_eq!(notes.len(), 1, "the checked box must not count");
        assert_eq!(notes[0].text, "escrever a conclusão");
    }

    #[test]
    fn no_todos_is_empty() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("main.tex"), "\\documentclass{article}\n");
        assert!(build(dir.path()).is_empty());
    }

    #[test]
    fn ignores_dot_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(".git/hooks/fake.tex"), "% TODO: não deveria aparecer\n");
        assert!(build(dir.path()).is_empty());
    }
}
