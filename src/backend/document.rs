//! The whole document as the reader gets it: the main file with every
//! `\input`/`\include` followed in place, in order. Gives the outline of all
//! its files, the prose words under each heading, and what the preamble says
//! (language, bibliography), so a thesis split in chapters reads as one.
//!
//! Word counting reuses the editor's lexer (`highlight::prose_word_starts`),
//! so the counts agree with what the editor shows.

use super::outline::{self, Entry};
use super::texparse;
use crate::editor::highlight;
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// Deeper than any real document; also ends include cycles.
const MAX_DEPTH: usize = 12;

#[derive(Clone, Debug, PartialEq)]
pub struct Heading {
    pub file: PathBuf,
    pub entry: Entry,
    /// Prose words from this heading to the next one, whatever file they are in.
    pub words: usize,
}

/// The language the preamble declares, for the spell checker.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Language {
    Portuguese,
    English,
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Document {
    pub headings: Vec<Heading>,
    /// Prose words before the first heading.
    pub front_words: usize,
    /// Every file read, in reading order (the main file first).
    pub files: Vec<PathBuf>,
    /// The heading each included file starts under (`None`: before the first).
    pub file_starts: HashMap<PathBuf, Option<usize>>,
    /// After an `\\input` line (file, line), the heading the reading is under:
    /// the included file's last one.
    pub after_includes: Vec<(PathBuf, usize, Option<usize>)>,
    /// Some file cites (`\cite…`).
    pub cites: bool,
    /// The document prints a bibliography (`\bibliography`, `\printbibliography`
    /// or `thebibliography`).
    pub bibliography: bool,
    /// Loads biblatex (`\printbibliography`) rather than BibTeX styles.
    pub biblatex: bool,
    /// Loads abntex2cite, which sets the bibliography style itself.
    pub abnt_citations: bool,
    pub language: Option<Language>,
}

impl Document {
    pub fn words(&self) -> usize {
        self.front_words + self.headings.iter().map(|h| h.words).sum::<usize>()
    }

    /// The heading the given place is under: the last one before `line` in
    /// `file`, or the one the file was included under.
    pub fn heading_at(&self, file: &Path, line: usize) -> Option<usize> {
        // The nearest thing above the line in this file: a heading, or an
        // \input whose content ended under some heading.
        let heading = self.headings.iter().enumerate().rev().find(|(_, h)| h.file == file && h.entry.line <= line).map(|(i, h)| (h.entry.line, Some(i)));
        let include = self.after_includes.iter().rev().find(|(f, l, _)| f == file && *l <= line).map(|(_, l, i)| (*l, *i));
        match (heading, include) {
            (Some(h), Some(i)) => if i.0 >= h.0 { i.1 } else { h.1 },
            (Some(h), None) => h.1,
            (None, Some(i)) => i.1,
            (None, None) => self.file_starts.get(file).copied().flatten(),
        }
    }

    pub fn contains(&self, file: &Path) -> bool {
        self.files.iter().any(|f| f == file)
    }
}

static INCLUDE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\(?:input|include|subfile)\s*\{([^}]+)\}").unwrap());
static CITE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\[a-zA-Z]*cite[a-zA-Z]*\*?\s*(?:\[[^\]]*\]\s*)*\{").unwrap());
static BIBLIOGRAPHY_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:bibliography\s*\{|printbibliography|begin\s*\{thebibliography\})").unwrap());
static PACKAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:usepackage|documentclass)\s*(?:\[([^\]]*)\])?\s*\{([^}]*)\}").unwrap());
static MAIN_LANGUAGE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\setmainlanguage\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}").unwrap());

/// Reads the document starting at `main`. `read` gives a file's text: the
/// editor's buffer when it is open, else the disk.
pub fn build(main: &Path, read: &dyn Fn(&Path) -> Option<String>) -> Document {
    let mut doc = Document::default();
    let Some(text) = read(main) else { return doc };
    // LaTeX resolves every \input against the main file's folder.
    let base = main.parent().unwrap_or(Path::new("")).to_path_buf();
    let preamble_end = text.find("\\begin{document}").unwrap_or(0);
    read_preamble(&text[..preamble_end], &mut doc);
    let mut seen = HashSet::new();
    walk(main, &text, highlight::body(&text), &base, read, &mut doc, &mut seen, 0);
    doc
}

fn read_preamble(preamble: &str, doc: &mut Document) {
    let preamble: String = preamble.lines().map(texparse::without_comment).collect::<Vec<_>>().join("\n");
    for caps in PACKAGE_RE.captures_iter(&preamble) {
        let options = caps.get(1).map(|o| o.as_str()).unwrap_or("");
        let names = &caps[2];
        for name in names.split(',').map(str::trim) {
            match name {
                "biblatex" => doc.biblatex = true,
                "abntex2cite" => doc.abnt_citations = true,
                // The last language option is babel's main language; a class
                // option (abntex2's `brazil`) counts too.
                _ => {
                    if let Some(language) = options.split(',').rev().find_map(|o| language_of(o.trim())) {
                        doc.language = Some(language);
                    }
                }
            }
        }
    }
    if let Some(caps) = MAIN_LANGUAGE_RE.captures(&preamble) {
        doc.language = language_of(caps[1].trim()).or(doc.language);
    }
}

fn language_of(name: &str) -> Option<Language> {
    match name {
        "brazil" | "brazilian" | "portuguese" | "portuges" | "portugues" => Some(Language::Portuguese),
        "english" | "american" | "british" | "USenglish" | "UKenglish" | "canadian" | "australian" => Some(Language::English),
        _ => None,
    }
}

#[allow(clippy::too_many_arguments)]
fn walk(
    file: &Path,
    text: &str,
    range: std::ops::Range<usize>,
    base: &Path,
    read: &dyn Fn(&Path) -> Option<String>,
    doc: &mut Document,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) {
    if !seen.insert(file.to_path_buf()) || depth > MAX_DEPTH {
        return;
    }
    doc.files.push(file.to_path_buf());
    let body = &text[range.clone()];
    let code: String = body.split('\n').map(texparse::without_comment).collect::<Vec<_>>().join("\n");
    doc.cites |= CITE_RE.is_match(&code);
    doc.bibliography |= BIBLIOGRAPHY_RE.is_match(&code);

    let words = highlight::prose_word_starts(text);
    let count = |a: usize, b: usize| words.partition_point(|&w| w < b) - words.partition_point(|&w| w < a);

    // Headings and includes, in text order, by the byte their line starts at.
    enum Event {
        Heading(Entry),
        Include(PathBuf),
    }
    let line_starts: Vec<usize> = std::iter::once(0).chain(text.match_indices('\n').map(|(i, _)| i + 1)).collect();
    let mut events: Vec<(usize, Event)> = outline::parse(text)
        .into_iter()
        .map(|e| (line_starts[e.line - 1], Event::Heading(e)))
        .filter(|(at, _)| range.contains(at))
        .collect();
    for (number, line) in text.split('\n').enumerate() {
        let at = line_starts[number];
        if !range.contains(&at) && !(range.start > at && range.start < at + line.len()) {
            continue;
        }
        for caps in INCLUDE_RE.captures_iter(texparse::without_comment(line)) {
            let name = caps[1].trim();
            let path = super::project::clean(&base.join(name));
            let path = if path.extension().is_some_and(|e| e == "tex") { path } else { path.with_extension("tex") };
            events.push((at.max(range.start), Event::Include(path)));
        }
    }
    events.sort_by_key(|(at, _)| *at);

    let mut from = range.start;
    let add_words = |doc: &mut Document, n: usize| match doc.headings.last_mut() {
        Some(h) => h.words += n,
        None => doc.front_words += n,
    };
    for (at, event) in events {
        add_words(doc, count(from, at));
        from = at;
        match event {
            Event::Heading(entry) => doc.headings.push(Heading { file: file.to_path_buf(), entry, words: 0 }),
            Event::Include(path) => {
                if let Some(included) = read(&path) {
                    doc.file_starts.entry(path.clone()).or_insert(doc.headings.len().checked_sub(1));
                    let len = included.len();
                    walk(&path, &included, 0..len, base, read, doc, seen, depth + 1);
                    let line = line_starts.partition_point(|&s| s <= at);
                    doc.after_includes.push((file.to_path_buf(), line, doc.headings.len().checked_sub(1)));
                }
            }
        }
    }
    add_words(doc, count(from, range.end));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build_from(files: &[(&str, &str)]) -> Document {
        let map: HashMap<PathBuf, String> = files.iter().map(|(p, t)| (PathBuf::from(p), t.to_string())).collect();
        build(Path::new("/p/main.tex"), &|p: &Path| map.get(p).cloned())
    }

    fn outline(doc: &Document) -> Vec<(String, String, usize)> {
        doc.headings.iter().map(|h| (h.file.display().to_string(), h.entry.title.clone(), h.words)).collect()
    }

    #[test]
    fn follows_inputs_in_reading_order() {
        let doc = build_from(&[
            (
                "/p/main.tex",
                "\\documentclass{report}\n\\usepackage[english,brazil]{babel}\n\\begin{document}\nUm dois.\n\\input{cap/um}\n\\chapter{Fim}\nTrês quatro cinco.\n\\end{document}\nfora",
            ),
            ("/p/cap/um.tex", "\\chapter{Um}\nSeis \\emph{sete} $x+y$ oito.\n% comentário ignorado\n\\section{Dois}\nNove \\cite{k}.\n"),
        ]);
        assert_eq!(
            outline(&doc),
            vec![
                ("/p/cap/um.tex".into(), "Um".into(), 4),
                ("/p/cap/um.tex".into(), "Dois".into(), 2),
                ("/p/main.tex".into(), "Fim".into(), 4),
            ]
        );
        assert_eq!(doc.front_words, 2);
        assert_eq!(doc.words(), 12);
        assert_eq!(doc.language, Some(Language::Portuguese));
        assert!(doc.cites && !doc.bibliography);
        assert_eq!(doc.files.len(), 2);
        // The cursor in the chapter file, and in main after the \input.
        assert_eq!(doc.heading_at(Path::new("/p/cap/um.tex"), 5), Some(1));
        assert_eq!(doc.heading_at(Path::new("/p/main.tex"), 4), None);
        // Right after the \input: under the chapter file's last section.
        assert_eq!(doc.heading_at(Path::new("/p/main.tex"), 5), Some(1));
        assert_eq!(doc.heading_at(Path::new("/p/main.tex"), 7), Some(2));
    }

    #[test]
    fn preamble_facts() {
        let doc = build_from(&[(
            "/p/main.tex",
            "\\documentclass[12pt,brazil]{abntex2}\n\\usepackage[alf]{abntex2cite}\n% \\usepackage{biblatex}\n\\begin{document}\n\\bibliography{refs}\n\\end{document}",
        )]);
        assert_eq!(doc.language, Some(Language::Portuguese));
        assert!(doc.abnt_citations && !doc.biblatex && doc.bibliography);
        let english = build_from(&[("/p/main.tex", "\\usepackage{polyglossia}\n\\setmainlanguage{english}\n\\begin{document}\\end{document}")]);
        assert_eq!(english.language, Some(Language::English));
    }

    #[test]
    fn survives_cycles_and_missing_files() {
        let doc = build_from(&[("/p/main.tex", "\\input{main}\n\\input{nada}\nTexto.")]);
        assert_eq!(doc.files.len(), 1);
        assert_eq!(doc.words(), 1);
    }
}

