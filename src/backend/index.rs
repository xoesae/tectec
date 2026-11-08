//! Scans the project for the autocomplete: `\label{}` names, `.bib` entries
//! and files that can go into an `\includegraphics{}`.
//!
//! The scan covers the whole project, not what `\input` reaches: a `\label` in
//! a chapter that is not included yet is exactly what you want to complete.

use super::project::{self, ext_lower, kind_of, Kind, Res};
use super::texparse;
use regex::Regex;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{LazyLock, Mutex};

#[derive(Clone, Debug)]
pub struct LabelEntry {
    pub name: String,
    pub file: PathBuf,
    pub line: usize,
    /// The section title or caption in force.
    pub context: String,
}

#[derive(Clone, Debug, Default)]
pub struct CiteEntry {
    pub key: String,
    pub kind: String,
    pub title: String,
    pub author: String,
    pub year: String,
    pub doi: String,
    pub file: PathBuf,
}

#[derive(Clone, Debug)]
pub struct GraphicEntry {
    /// Relative to the main file's folder, forward slashes.
    pub path: String,
    pub kind: Kind,
}

/// A command or environment the project defines itself (`\newcommand`,
/// `\DeclareMathOperator`, `\newenvironment`, `\newtheorem`…).
#[derive(Clone, Debug, PartialEq)]
pub struct MacroEntry {
    /// With the backslash for a command, bare for an environment.
    pub name: String,
    pub args: usize,
    pub environment: bool,
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug, Default)]
pub struct Index {
    pub labels: Vec<LabelEntry>,
    pub citations: Vec<CiteEntry>,
    pub graphics: Vec<GraphicEntry>,
    pub macros: Vec<MacroEntry>,
    /// What a formula needs from the project to render as in the document:
    /// see `math_preamble`.
    pub math_preamble: String,
}

static LABEL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\label\s*(?:\[[^\]]*\])?\s*\{([^}]+)\}").unwrap());
static SECTION_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:sub){0,2}section\*?\s*(?:\[[^\]]*\])?\s*\{(.+)").unwrap());
static CHAPTER_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\chapter\*?\s*(?:\[[^\]]*\])?\s*\{(.+)").unwrap());
static CAPTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\caption\s*(?:\[[^\]]*\])?\s*\{(.+)").unwrap());
static ENV_END_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\end\s*\{").unwrap());

static NEWCOMMAND_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:(?:re|provide)?newcommand|DeclareRobustCommand)\*?\s*\{?\s*(\\[A-Za-z]+)\s*\}?\s*(?:\[(\d)\])?").unwrap()
});
static MATHOP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\DeclareMathOperator\*?\s*\{?\s*(\\[A-Za-z]+)").unwrap());
static DEF_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\def\s*(\\[A-Za-z]+)").unwrap());
static NEWENV_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\\(?:(?:re)?newenvironment|newtheorem|declaretheorem)\*?\s*\{([A-Za-z*]+)\}\s*(?:\[(\d)\])?").unwrap()
});

const CAPTION_REACH_IN_LINES: usize = 6;

/// Walks every non-hidden file under `root`, calling `visit` for each.
pub fn walk_files(root: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(entries) = fs::read_dir(root) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if name.starts_with('.') {
            continue;
        }
        let path = e.path();
        if path.is_dir() {
            walk_files(&path, visit);
        } else {
            visit(&path, &name);
        }
    }
}

pub fn build(root: &Path, main_file: Option<&Path>) -> Res<Index> {
    if root.as_os_str().is_empty() {
        return Err("nenhum projeto aberto".into());
    }
    let figure_base = match main_file {
        Some(m) => project::resolve(root, m)?.parent().map(Path::to_path_buf).unwrap_or(root.to_path_buf()),
        None => root.to_path_buf(),
    };
    let mut index = Index::default();
    let mut math = MathPreamble::default();
    walk_files(root, &mut |path, name| {
        match ext_lower(name).as_str() {
            "tex" => {
                index.labels.extend(labels_in_file(path));
                index.macros.extend(macros_in_file(path));
                if let Ok(text) = fs::read_to_string(path) {
                    math.add(&text);
                }
            }
            "sty" | "cls" => index.macros.extend(macros_in_file(path)),
            "bib" => index.citations.extend(parse_bib_file(path)),
            _ => {}
        }
        let kind = kind_of(name, false);
        if matches!(kind, Kind::Image | Kind::Svg | Kind::Pdf) {
            if kind == Kind::Pdf && is_build_output(path) {
                return;
            }
            index.graphics.push(GraphicEntry { path: relative_slash(&figure_base, path), kind });
        }
    });
    index.labels.sort_by(|a, b| a.name.cmp(&b.name));
    index.citations.sort_by(|a, b| a.key.cmp(&b.key));
    index.graphics.sort_by(|a, b| a.path.cmp(&b.path));
    index.macros.sort_by(|a, b| a.name.cmp(&b.name));
    index.macros.dedup_by(|a, b| a.name == b.name && a.environment == b.environment);
    index.math_preamble = math.finish();
    Ok(index)
}

/// Math packages the formula preview loads when the project does. Others
/// (fonts, layout, bibliography…) are left out: they cost time, and a
/// formula does not need them.
const MATH_PACKAGES: &[&str] = &[
    "amsmath", "amssymb", "amsfonts", "mathtools", "bm", "physics", "mathrsfs", "esint", "braket", "cancel", "siunitx",
    "stmaryrd", "xcolor", "dsfont", "bbm", "upgreek", "nicefrac", "units", "commath", "mathabx", "bbold",
];

static USEPACKAGE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\(?:usepackage|RequirePackage)\s*(?:\[[^\]]*\])?\s*\{([^}]*)\}").unwrap());
static DEFINITION_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\\(?:(?:re|provide)?newcommand|DeclareMathOperator|DeclareRobustCommand|def)\b").unwrap()
});

/// Opened minus closed braces, not counting escaped ones (`\{`).
fn brace_delta(line: &str) -> i32 {
    let mut delta = 0;
    let mut escaped = false;
    for c in line.chars() {
        match c {
            _ if escaped => escaped = false,
            '\\' => escaped = true,
            '{' => delta += 1,
            '}' => delta -= 1,
            _ => {}
        }
    }
    delta
}

/// The project's command definitions and math packages, in the order met,
/// so the math preview renders `\norm{x}` or `\R` as the document does.
#[derive(Default)]
struct MathPreamble {
    packages: Vec<String>,
    definitions: Vec<String>,
}

impl MathPreamble {
    fn add(&mut self, text: &str) {
        let lines: Vec<&str> = text.lines().map(texparse::without_comment).collect();
        let mut i = 0;
        while i < lines.len() {
            let line = lines[i].trim_start();
            for c in USEPACKAGE_RE.captures_iter(line) {
                for name in c[1].split(',').map(str::trim) {
                    if MATH_PACKAGES.contains(&name) && !self.packages.iter().any(|p| p == name) {
                        self.packages.push(name.to_string());
                    }
                }
            }
            if !DEFINITION_RE.is_match(line) {
                i += 1;
                continue;
            }
            // A definition can span lines: up to where its braces balance.
            let mut def = String::new();
            let mut depth = 0i32;
            while i < lines.len() && def.lines().count() < 20 {
                let l = lines[i];
                depth += brace_delta(l);
                def.push_str(l.trim_end());
                def.push('\n');
                i += 1;
                if depth <= 0 {
                    break;
                }
            }
            if depth == 0 && !def.contains('@') && !self.definitions.contains(&def) {
                self.definitions.push(def);
            }
        }
    }

    fn finish(self) -> String {
        let mut out = String::new();
        if !self.packages.is_empty() {
            out.push_str(&format!("\\usepackage{{{}}}\n", self.packages.join(",")));
        }
        for d in self.definitions {
            out.push_str(&d);
        }
        out
    }
}

/// `path` relative to `base` with forward slashes, climbing with `..` when
/// needed: what goes inside an `\includegraphics{}`.
pub fn relative_slash(base: &Path, path: &Path) -> String {
    let base: Vec<_> = base.components().collect();
    let target: Vec<_> = path.components().collect();
    let common = base.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = std::iter::repeat_n("..".to_string(), base.len() - common).collect();
    parts.extend(target[common..].iter().map(|c| c.as_os_str().to_string_lossy().into_owned()));
    parts.join("/")
}

/// The PDF the project itself produced sits next to a `.tex` of the same name.
pub fn is_build_output(pdf: &Path) -> bool {
    pdf.with_extension("tex").exists()
}

fn labels_in_file(path: &Path) -> Vec<LabelEntry> {
    let Ok(text) = fs::read_to_string(path) else { return vec![] };
    let mut found = Vec::new();
    let mut section = String::new();
    let mut caption = String::new();
    let mut caption_line = 0;
    for (i, raw) in text.lines().enumerate() {
        let number = i + 1;
        let line = texparse::without_comment(raw);
        if let Some(m) = CHAPTER_RE.captures(line) {
            section = texparse::argument_text(&m[1]);
        } else if let Some(m) = SECTION_RE.captures(line) {
            section = texparse::argument_text(&m[1]);
        }
        if let Some(m) = CAPTION_RE.captures(line) {
            caption = texparse::argument_text(&m[1]);
            caption_line = number;
        } else if ENV_END_RE.is_match(line) {
            caption.clear();
        }
        for m in LABEL_RE.captures_iter(line) {
            let name = m[1].trim();
            if name.is_empty() {
                continue;
            }
            let context = if !caption.is_empty() && number - caption_line <= CAPTION_REACH_IN_LINES {
                caption.clone()
            } else {
                section.clone()
            };
            found.push(LabelEntry { name: name.to_string(), file: path.to_path_buf(), line: number, context });
        }
    }
    found
}

/// Commands and environments a file defines.
pub fn macros_in_file(path: &Path) -> Vec<MacroEntry> {
    let Ok(text) = fs::read_to_string(path) else { return vec![] };
    let mut found = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = texparse::without_comment(raw);
        if !line.contains('\\') {
            continue;
        }
        let mut push = |name: &str, args: Option<regex::Match>, environment: bool| {
            let args = args.and_then(|a| a.as_str().parse().ok()).unwrap_or(0);
            found.push(MacroEntry { name: name.to_string(), args, environment, file: path.to_path_buf(), line: i + 1 });
        };
        for m in NEWCOMMAND_RE.captures_iter(line) {
            push(&m[1], m.get(2), false);
        }
        for m in MATHOP_RE.captures_iter(line) {
            push(&m[1], None, false);
        }
        for m in DEF_RE.captures_iter(line) {
            push(&m[1], None, false);
        }
        for m in NEWENV_RE.captures_iter(line) {
            push(&m[1], m.get(2), true);
        }
    }
    found
}

/// Reads a `.bib` far enough for the autocomplete and the reference library.
/// Not a complete BibTeX parser: `@string`, `crossref` and `#` are left out.
pub fn parse_bib_file(path: &Path) -> Vec<CiteEntry> {
    let Ok(bytes) = fs::read(path) else { return vec![] };
    let text = String::from_utf8_lossy(&bytes);
    let mut entries = Vec::new();
    for (key, kind, fields) in parse_bib(&text) {
        let get = |k: &str| fields.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone()).unwrap_or_default();
        entries.push(CiteEntry {
            key,
            kind,
            title: get("title"),
            author: get("author"),
            year: get("year"),
            doi: get("doi"),
            file: path.to_path_buf(),
        });
    }
    entries
}

/// (key, lowercase kind, fields) for every real entry.
pub fn parse_bib(text: &str) -> Vec<(String, String, Vec<(String, String)>)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'@' {
            i += 1;
            continue;
        }
        let (kind, body, next) = bib_entry(text, i);
        i = next.max(i + 1);
        let kind = kind.to_lowercase();
        if matches!(kind.as_str(), "comment" | "preamble" | "string") {
            continue;
        }
        let (key, fields) = bib_body(body);
        if key.is_empty() {
            continue;
        }
        out.push((key, kind, fields));
    }
    out
}

/// (key, byte range from the `@` to the closing brace) for every real entry,
/// in file order.
pub fn bib_spans(text: &str) -> Vec<(String, std::ops::Range<usize>)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'@' {
            i += 1;
            continue;
        }
        let start = i;
        let (kind, body, next) = bib_entry(text, i);
        i = next.max(i + 1);
        if matches!(kind.to_lowercase().as_str(), "comment" | "preamble" | "string") {
            continue;
        }
        let (key, _) = bib_body(body);
        if !key.is_empty() {
            out.push((key, start..(next + 1).min(b.len())));
        }
    }
    out
}

fn bib_entry(text: &str, start: usize) -> (&str, &str, usize) {
    let b = text.as_bytes();
    let mut i = start + 1;
    while i < b.len() && b[i].is_ascii_alphabetic() {
        i += 1;
    }
    let kind = &text[start + 1..i];
    while i < b.len() && (b[i] == b' ' || b[i] == b'\t') {
        i += 1;
    }
    if i >= b.len() || b[i] != b'{' {
        return (kind, "", i);
    }
    let opening = i;
    let mut level = 0;
    while i < b.len() {
        match b[i] {
            b'{' => level += 1,
            b'}' => {
                level -= 1;
                if level == 0 {
                    return (kind, &text[opening + 1..i], i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    (kind, &text[opening + 1..], b.len())
}

fn bib_body(body: &str) -> (String, Vec<(String, String)>) {
    let mut fields = Vec::new();
    let Some(comma) = body.find(',') else { return (body.trim().to_string(), fields) };
    let key = body[..comma].trim().to_string();
    let mut rest = &body[comma + 1..];
    while !rest.is_empty() {
        let Some(eq) = rest.find('=') else { break };
        let name = rest[..eq].trim().to_lowercase();
        let (value, consumed) = bib_value(&rest[eq + 1..]);
        if !name.is_empty() {
            fields.push((name, clean_bib_value(value)));
        }
        rest = &rest[eq + 1 + consumed..];
        match rest.find(',') {
            Some(c) => rest = &rest[c + 1..],
            None => break,
        }
    }
    (key, fields)
}

fn bib_value(rest: &str) -> (&str, usize) {
    let b = rest.as_bytes();
    let mut i = 0;
    while i < b.len() && matches!(b[i], b' ' | b'\t' | b'\n' | b'\r') {
        i += 1;
    }
    if i >= b.len() {
        return ("", i);
    }
    match b[i] {
        b'{' => {
            let start = i;
            let mut level = 0;
            while i < b.len() {
                match b[i] {
                    b'{' => level += 1,
                    b'}' => {
                        level -= 1;
                        if level == 0 {
                            return (&rest[start + 1..i], i + 1);
                        }
                    }
                    _ => {}
                }
                i += 1;
            }
            (&rest[start + 1..], b.len())
        }
        b'"' => {
            let start = i;
            i += 1;
            while i < b.len() {
                if b[i] == b'"' && b[i - 1] != b'\\' {
                    return (&rest[start + 1..i], i + 1);
                }
                i += 1;
            }
            (&rest[start + 1..], b.len())
        }
        _ => {
            let start = i;
            while i < b.len() && b[i] != b',' && b[i] != b'\n' {
                i += 1;
            }
            (rest[start..i].trim(), i)
        }
    }
}

fn clean_bib_value(value: &str) -> String {
    value.replace(['{', '}'], "").split_whitespace().collect::<Vec<_>>().join(" ")
}

// ---------- Installed packages ----------

static PACKAGES: Mutex<Option<Vec<String>>> = Mutex::new(None);

/// Packages available in the LaTeX install, for `\usepackage{}`. Scanned once.
pub fn packages() -> Vec<String> {
    let mut cached = PACKAGES.lock().unwrap();
    if let Some(list) = cached.as_ref() {
        return list.clone();
    }
    let mut list = scan_packages(&tex_roots());
    if list.is_empty() {
        list = COMMON_PACKAGES.iter().map(|s| s.to_string()).collect();
    }
    *cached = Some(list.clone());
    list
}

fn tex_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    if let Ok(out) = Command::new("kpsewhich").arg("-var-value=TEXMFDIST").output() {
        let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !dir.is_empty() {
            roots.push(Path::new(&dir).join("tex"));
        }
    }
    for c in ["/usr/share/texlive/texmf-dist/tex", "/usr/local/texlive/texmf-dist/tex"] {
        let p = PathBuf::from(c);
        if p.exists() && !roots.contains(&p) {
            roots.push(p);
        }
    }
    roots
}

fn scan_packages(roots: &[PathBuf]) -> Vec<String> {
    fn walk(dir: &Path, seen: &mut BTreeSet<String>) {
        let Ok(entries) = fs::read_dir(dir) else { return };
        for e in entries.flatten() {
            let path = e.path();
            if path.is_dir() {
                walk(&path, seen);
            } else if ext_lower(&path) == "sty" {
                if let Some(stem) = path.file_stem() {
                    seen.insert(stem.to_string_lossy().into_owned());
                }
            }
        }
    }
    let mut seen = BTreeSet::new();
    for r in roots {
        walk(r, &mut seen);
    }
    seen.into_iter().collect()
}

const COMMON_PACKAGES: &[&str] = &[
    "amsfonts", "amsmath", "amssymb", "amsthm", "array", "babel", "biblatex", "booktabs", "caption", "cleveref",
    "color", "csquotes", "enumitem", "fancyhdr", "float", "fontenc", "geometry", "graphicx", "hyperref", "inputenc",
    "listings", "longtable", "mathtools", "microtype", "multicol", "multirow", "natbib", "parskip", "pgfplots",
    "setspace", "siunitx", "subcaption", "tabularx", "tcolorbox", "tikz", "titlesec", "todonotes", "url", "wrapfig",
    "xcolor",
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bib_parsing() {
        let text = "@comment{x}\n@Article{knuth84,\n  author = {Donald {E.} Knuth},\n  title = \"Literate Programming\",\n  year = 1984,\n  doi={10.1/x}\n}\n@book{b2, title={Só título}}";
        let e = parse_bib(text);
        assert_eq!(e.len(), 2);
        assert_eq!(e[0].0, "knuth84");
        assert_eq!(e[0].1, "article");
        let f = &e[0].2;
        assert!(f.contains(&("author".into(), "Donald E. Knuth".into())));
        assert!(f.contains(&("title".into(), "Literate Programming".into())));
        assert!(f.contains(&("year".into(), "1984".into())));
        assert_eq!(e[1].2[0].1, "Só título");
    }

    #[test]
    fn relative_paths() {
        assert_eq!(relative_slash(Path::new("/p/src"), Path::new("/p/src/img/a.png")), "img/a.png");
        assert_eq!(relative_slash(Path::new("/p/src"), Path::new("/p/img/a.png")), "../img/a.png");
    }

    #[test]
    fn labels_with_context() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("a.tex");
        fs::write(&f, "\\section{Intro}\n\\label{sec:intro}\n\\begin{figure}\n\\caption{Setup}\n\\label{fig:setup}\n\\end{figure}\n\\label{after}\n").unwrap();
        let l = labels_in_file(&f);
        assert_eq!(l[0].context, "Intro");
        assert_eq!(l[1].context, "Setup");
        assert_eq!(l[2].context, "Intro");
    }

    const MAIN_WITH_LABELS: &str = "\\documentclass{article}\n\\usepackage{graphicx}\n\\begin{document}\n\\section{Metodologia}\n\
\\label{sec:metodologia}\nTexto qualquer citando \\cite{knuth1984}.\n\n% \\label{com:comentado} não deveria entrar no índice\n\
\\begin{figure}\n  \\includegraphics{fig/grafico.png}\n  \\caption{Um gr\\'afico do \\emph{experimento} principal}\n  \\label{fig:grafico}\n\
\\end{figure}\n\n\\begin{equation}\\label{eq:principal} E = mc^2 \\end{equation}\n\\input{cap/extra}\n\\end{document}\n";

    const TEST_BIB: &str = "@article{knuth1984,\n  author  = {Donald E. Knuth},\n  title   = {Literate Programming},\n\
  journal = {The Computer Journal},\n  year    = {1984},\n}\n\n@book{lamport1994,\n  author = \"Leslie Lamport\",\n\
  title  = {{\\LaTeX}: A Document Preparation System},\n  year   = 1994\n}\n\n@comment{isto não é uma entrada}\n";

    /// A small project carrying every trap: a commented-out label, a caption
    /// with markup, a .bib with all three value styles, and a PDF that is a
    /// build output.
    fn indexed_project() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        for (rel, content) in [
            ("main.tex", MAIN_WITH_LABELS),
            ("cap/extra.tex", "\\section{Resultados obtidos}\nMais texto.\n\\label{sec:resultados}\n"),
            ("refs.bib", TEST_BIB),
            ("fig/grafico.png", "\u{89}PNG pretend"),
            ("fig/diagrama.pdf", "%PDF pretend"),
            ("main.pdf", "%PDF build output"),
            (".git/config", "should not be scanned"),
        ] {
            let p = dir.path().join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, content).unwrap();
        }
        dir
    }

    #[test]
    fn build_finds_labels() {
        let dir = indexed_project();
        let root = dir.path();
        let index = build(root, Some(&root.join("main.tex"))).unwrap();
        let get = |n: &str| index.labels.iter().find(|l| l.name == n);
        for name in ["sec:metodologia", "fig:grafico", "eq:principal", "sec:resultados"] {
            assert!(get(name).is_some(), "{name} missing");
        }
        assert!(get("com:comentado").is_none(), "a commented-out \\label entered the index");
        assert_eq!(get("fig:grafico").unwrap().context, "Um grafico do experimento principal");
        // After \end{figure} the caption no longer applies.
        assert_eq!(get("eq:principal").unwrap().context, "Metodologia");
        assert_eq!(get("eq:principal").unwrap().line, 15);
        let res = get("sec:resultados").unwrap();
        assert_eq!(res.context, "Resultados obtidos");
        assert_eq!(res.file.file_name().unwrap(), "extra.tex");
    }

    #[test]
    fn build_reads_bib() {
        let dir = indexed_project();
        let index = build(dir.path(), Some(&dir.path().join("main.tex"))).unwrap();
        assert_eq!(index.citations.len(), 2, "{:?}", index.citations);
        let get = |k: &str| index.citations.iter().find(|c| c.key == k).unwrap();
        let knuth = get("knuth1984");
        assert_eq!((knuth.kind.as_str(), knuth.year.as_str(), knuth.author.as_str()), ("article", "1984", "Donald E. Knuth"));
        assert_eq!(knuth.title, "Literate Programming");
        // Quotes, nested braces and a bare year: what a real .bib does.
        let lamport = get("lamport1994");
        assert_eq!((lamport.author.as_str(), lamport.year.as_str()), ("Leslie Lamport", "1994"));
        assert_eq!(lamport.title, "\\LaTeX: A Document Preparation System");
    }

    #[test]
    fn build_lists_graphics_relative_to_the_main_file() {
        let dir = indexed_project();
        let index = build(dir.path(), Some(&dir.path().join("main.tex"))).unwrap();
        let paths: Vec<&str> = index.graphics.iter().map(|g| g.path.as_str()).collect();
        assert!(paths.contains(&"fig/grafico.png") && paths.contains(&"fig/diagrama.pdf"), "{paths:?}");
        assert!(!paths.contains(&"main.pdf"), "the compiled PDF is not a figure");
    }

    #[test]
    fn build_rejects_closed_project_or_outside_main_file() {
        assert!(build(Path::new(""), None).is_err());
        let dir = indexed_project();
        let outside = dir.path().parent().unwrap().join("other.tex");
        assert!(build(dir.path(), Some(&outside)).is_err());
    }

    #[test]
    fn packages_brings_known_packages() {
        if !crate::backend::latex::which("kpsewhich") {
            return;
        }
        let p = packages();
        for want in ["amsmath", "hyperref"] {
            assert!(p.iter().any(|x| x == want), "{want} missing among {} packages", p.len());
        }
        assert!(!p.iter().any(|x| x.ends_with(".sty")));
    }

    #[test]
    fn math_preamble_takes_definitions_and_math_packages() {
        let mut m = MathPreamble::default();
        m.add(
            "\\documentclass{article}\n\\usepackage[utf8]{inputenc}\n\\usepackage{amsmath, bm,geometry}\n\
             \\newcommand{\\norm}[1]{\\lVert #1 \\rVert} % norma\n\\DeclareMathOperator{\\argmax}{arg\\,max}\n\
             \\newcommand{\\longo}{%\n  \\alpha + \\beta\n}\n\\def\\x@y{z}\n\\begin{document}\n\\end{document}\n",
        );
        m.add("\\newcommand{\\norm}[1]{\\lVert #1 \\rVert}\n\\usepackage{mathtools}\n");
        assert_eq!(
            m.finish(),
            "\\usepackage{amsmath,bm,mathtools}\n\\newcommand{\\norm}[1]{\\lVert #1 \\rVert}\n\
             \\DeclareMathOperator{\\argmax}{arg\\,max}\n\\newcommand{\\longo}{\n  \\alpha + \\beta\n}\n"
        );
    }

    #[test]
    fn finds_project_macros() {
        let dir = tempfile::tempdir().unwrap();
        let f = dir.path().join("preamble.tex");
        fs::write(
            &f,
            "\\newcommand{\\R}{\\mathbb{R}}\n\\newcommand\\norm[1]{\\lVert #1\\rVert}\n\\renewcommand*{\\vec}[2]{}\n\
             \\DeclareMathOperator*{\\argmax}{arg\\,max}\n\\def\\eps{\\varepsilon}\n\\newenvironment{resumo}[1]{}{}\n\
             \\newtheorem{teorema}{Teorema}\n% \\newcommand{\\comentado}{}\n",
        )
        .unwrap();
        let m: Vec<(String, usize, bool)> = macros_in_file(&f).into_iter().map(|m| (m.name, m.args, m.environment)).collect();
        assert_eq!(
            m,
            [
                ("\\R".into(), 0, false),
                ("\\norm".into(), 1, false),
                ("\\vec".into(), 2, false),
                ("\\argmax".into(), 0, false),
                ("\\eps".into(), 0, false),
                ("resumo".into(), 1, true),
                ("teorema".into(), 0, true),
            ]
        );
    }
}
