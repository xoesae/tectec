//! Project-wide search: every line in the project's editable text files that
//! contains the query, case-insensitively. Read-only: replacing goes through
//! the same read/write path (with its conflict check) as saving a tab.

use super::index::walk_files;
use super::project::{kind_of, Kind, Res};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug)]
pub struct Match {
    pub file: PathBuf,
    pub line: usize,
    pub text: String,
}

pub fn find(root: &Path, query: &str) -> Res<Vec<Match>> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Ok(vec![]);
    }
    let mut matches = Vec::new();
    walk_files(root, &mut |path, name| {
        if !matches!(kind_of(name, false), Kind::Tex | Kind::Text) {
            return;
        }
        let Ok(data) = fs::read(path) else { return };
        if data[..data.len().min(8192)].contains(&0) {
            return;
        }
        let text = String::from_utf8_lossy(&data);
        for (i, line) in text.split('\n').enumerate() {
            if line.to_lowercase().contains(&q) {
                matches.push(Match { file: path.to_path_buf(), line: i + 1, text: line.trim().to_string() });
            }
        }
    });
    matches.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    Ok(matches)
}

/// Replaces every case-insensitive occurrence of `query`, returning the new
/// text and how many were replaced.
pub fn replace_all_case_insensitive(content: &str, query: &str, replacement: &str) -> (String, usize) {
    let re = regex::RegexBuilder::new(&regex::escape(query)).case_insensitive(true).build();
    let Ok(re) = re else { return (content.to_string(), 0) };
    let count = re.find_iter(content).count();
    (re.replace_all(content, regex::NoExpand(replacement)).into_owned(), count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_and_replaces() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.tex"), "Olá Mundo\nnada\nmundo cruel").unwrap();
        fs::write(dir.path().join("b.png"), "mundo").unwrap();
        let m = find(dir.path(), "MUNDO").unwrap();
        assert_eq!(m.len(), 2);
        assert_eq!(m[1].line, 3);
        let (t, n) = replace_all_case_insensitive("Mundo mundo", "mundo", "$1x");
        assert_eq!((t.as_str(), n), ("$1x $1x", 2));
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn matches_across_files() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("main.tex"), "linha um\nAES-128 é o algoritmo\noutra linha\n");
        write(&dir.path().join("notes/ideias.md"), "# Ideias\numa nota sobre aes\n");
        assert_eq!(find(dir.path(), "aes").unwrap().len(), 2);
    }

    #[test]
    fn line_number_and_text() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("main.tex"), "primeira\nsegunda com termo\nterceira\n");
        let m = find(dir.path(), "termo").unwrap();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].line, 2);
        assert_eq!(m[0].text, "segunda com termo");
    }

    #[test]
    fn skips_binary_and_unknown_files() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("main.tex"), "termo aqui\n");
        // A .txt with a null byte is skipped even though its extension says text.
        write(&dir.path().join("weird.txt"), "termo\0binario");
        write(&dir.path().join("data.unknownext"), "termo aqui");
        let m = find(dir.path(), "termo").unwrap();
        assert_eq!(m.len(), 1, "{m:?}");
        assert_eq!(m[0].file, dir.path().join("main.tex"));
    }

    #[test]
    fn blank_query_finds_nothing() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join("main.tex"), "algo\n");
        assert!(find(dir.path(), "   ").unwrap().is_empty());
    }

    #[test]
    fn ignores_dot_dirs() {
        let dir = tempfile::tempdir().unwrap();
        write(&dir.path().join(".git/config.tex"), "termo\n");
        assert!(find(dir.path(), "termo").unwrap().is_empty());
    }
}
