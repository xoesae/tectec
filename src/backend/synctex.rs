//! Reads the map LaTeX writes with `-synctex=1` and answers the two questions
//! synced scrolling asks: where in the PDF a source line landed, and which
//! source line produced a point on a page.
//!
//! Coordinates are PDF points with the origin at the top left of the page;
//! a record's Y is its baseline, the box runs from `y - height` to `y + depth`.

use flate2::read::GzDecoder;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

/// 65536 sp = 1 PDF point.
const SP_PER_POINT: f64 = 65536.0;

#[derive(Clone, Copy, Debug, Default)]
struct Record {
    page: usize,
    tag: usize,
    line: usize,
    x: f64,
    y: f64,
    width: f64,
    height: f64,
    depth: f64,
    boxed: bool,
}

impl Record {
    fn top(&self) -> f64 {
        self.y - self.height
    }
    fn bottom(&self) -> f64 {
        self.y + self.depth
    }
    fn contains(&self, x: f64, y: f64) -> bool {
        x >= self.x && x <= self.x + self.width && y >= self.top() && y <= self.bottom()
    }
    fn area(&self) -> f64 {
        self.width * (self.height + self.depth)
    }
}

#[derive(Debug)]
pub struct Map {
    files: HashMap<usize, PathBuf>,
    records: Vec<Record>,
    by_line: HashMap<usize, HashMap<usize, Vec<usize>>>,
    by_page: HashMap<usize, Vec<usize>>,
    lines_by_tag: HashMap<usize, Vec<usize>>,
}

/// Result of a forward search (source → PDF). `page` is 1-based.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Position {
    pub page: usize,
    pub x: f64,
    /// Top of the box.
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

/// Result of an inverse search (PDF → source).
#[derive(Clone, Debug, PartialEq)]
pub struct Source {
    pub file: PathBuf,
    pub line: usize,
}

static RECORD_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([\[\(hvxkgr$f])(\d+),(\d+)(?:,\d+)?:(-?\d+),(-?\d+)(.*)$").unwrap());

/// The `.synctex.gz` (or uncompressed `.synctex`) next to the main file.
pub fn file_for(main_file: &Path) -> Option<PathBuf> {
    let gz = main_file.with_extension("synctex.gz");
    if gz.exists() {
        return Some(gz);
    }
    let plain = main_file.with_extension("synctex");
    plain.exists().then_some(plain)
}

pub fn parse(path: &Path) -> Result<Map, String> {
    let file = fs::File::open(path).map_err(|e| e.to_string())?;
    let reader: Box<dyn Read> =
        if path.to_string_lossy().ends_with(".gz") { Box::new(GzDecoder::new(file)) } else { Box::new(file) };
    let reader = BufReader::new(reader);

    let mut m = Map {
        files: HashMap::new(),
        records: Vec::new(),
        by_line: HashMap::new(),
        by_page: HashMap::new(),
        lines_by_tag: HashMap::new(),
    };
    let dir = path.parent().unwrap_or(Path::new("."));
    let (mut unit, mut magnification) = (1.0f64, 1.0f64);
    let (mut offset_x, mut offset_y) = (0.0f64, 0.0f64);
    let mut in_content = false;
    let mut page = 0usize;

    for line in reader.split(b'\n') {
        let line = line.map_err(|e| format!("lendo {}: {e}", path.display()))?;
        let line = String::from_utf8_lossy(&line);
        let line = line.trim_end_matches('\r');

        // Input:<tag>:<path>; the path may contain ':'. TeX writes it when it
        // opens the file, so an \include or a late \input shows up among the
        // pages. Tectonic leaves the path of its bundle files empty.
        if let Some(rest) = line.strip_prefix("Input:") {
            if let Some((tag, file)) = rest.split_once(':') {
                if let (Ok(tag), false) = (tag.parse::<usize>(), file.is_empty()) {
                    let p = Path::new(file);
                    let abs = if p.is_absolute() { p.to_path_buf() } else { dir.join(p) };
                    m.files.insert(tag, super::project::clean(&abs));
                }
            }
            continue;
        }
        if !in_content {
            if line == "Content:" {
                in_content = true;
            } else if let Some(v) = line.strip_prefix("Magnification:") {
                if let Ok(v) = v.trim().parse::<f64>() {
                    if v > 0.0 {
                        magnification = v / 1000.0;
                    }
                }
            } else if let Some(v) = line.strip_prefix("Unit:") {
                if let Ok(v) = v.trim().parse::<f64>() {
                    if v > 0.0 {
                        unit = v;
                    }
                }
            } else if let Some(v) = line.strip_prefix("X Offset:") {
                offset_x = v.trim().parse().unwrap_or(0.0);
            } else if let Some(v) = line.strip_prefix("Y Offset:") {
                offset_y = v.trim().parse().unwrap_or(0.0);
            }
            continue;
        }
        if line.is_empty() {
            continue;
        }
        if let Some(n) = line.strip_prefix('{') {
            page = n.parse().unwrap_or(0);
            continue;
        }
        if line.starts_with('}') {
            page = 0;
            continue;
        }
        if page == 0 {
            continue;
        }
        let Some(c) = RECORD_RE.captures(line) else { continue };
        let tag: usize = c[2].parse().unwrap_or(0);
        let line_number: usize = c[3].parse().unwrap_or(0);
        if line_number == 0 {
            continue;
        }
        let to_points = |v: f64| v * unit * magnification / SP_PER_POINT;
        let x: f64 = c[4].parse().unwrap_or(0.0);
        let y: f64 = c[5].parse().unwrap_or(0.0);
        let mut rec = Record {
            page,
            tag,
            line: line_number,
            x: to_points(x + offset_x),
            y: to_points(y + offset_y),
            ..Default::default()
        };
        if let Some(rest) = c[6].strip_prefix(':') {
            let parts: Vec<&str> = rest.split(',').collect();
            if let Ok(v) = parts[0].parse::<f64>() {
                rec.width = to_points(v);
            }
            if parts.len() >= 3 {
                rec.height = parts[1].parse::<f64>().map(to_points).unwrap_or(0.0);
                rec.depth = parts[2].parse::<f64>().map(to_points).unwrap_or(0.0);
                rec.boxed = rec.width > 0.0 && rec.height + rec.depth > 0.0;
            }
        }
        let index = m.records.len();
        m.records.push(rec);
        m.by_page.entry(page).or_default().push(index);
        let lines = m.by_line.entry(tag).or_default();
        if !lines.contains_key(&line_number) {
            m.lines_by_tag.entry(tag).or_default().push(line_number);
        }
        lines.entry(line_number).or_default().push(index);
    }

    if m.records.is_empty() {
        return Err(format!("nenhum registro em {}", super::project::file_name(path)));
    }
    for lines in m.lines_by_tag.values_mut() {
        lines.sort_unstable();
    }
    Ok(m)
}

impl Map {
    fn tag_of(&self, file: &Path) -> Option<usize> {
        let target = fs::canonicalize(file).unwrap_or_else(|_| super::project::clean(file));
        for (tag, path) in &self.files {
            if *path == target || fs::canonicalize(path).map(|p| p == target).unwrap_or(false) {
                return Some(*tag);
            }
        }
        // Last resort: synctex sometimes stores the path the way LaTeX saw it.
        let base = target.file_name()?;
        self.files.iter().find(|(_, p)| p.file_name() == Some(base)).map(|(t, _)| *t)
    }

    /// (file, line) → position in the PDF. Lines that produced no material
    /// fall back to the nearest line that did.
    pub fn forward(&self, file: &Path, line: usize) -> Option<Position> {
        let tag = self.tag_of(file)?;
        let lines = self.by_line.get(&tag)?;
        let indexes = match lines.get(&line) {
            Some(v) if !v.is_empty() => v,
            _ => lines.get(&self.nearest_line(tag, line)?)?,
        };
        // Prefer a real box: it gives a highlight with area instead of a point.
        let mut best = self.records[indexes[0]];
        for &i in indexes {
            let r = self.records[i];
            if r.boxed && (!best.boxed || r.page < best.page || (r.page == best.page && r.top() < best.top())) {
                best = r;
            }
        }
        // A point record sits on the baseline with zero height: climb to the
        // box containing it.
        if !best.boxed {
            if let Some(b) = self.box_containing(best.page, best.x, best.y) {
                best = b;
            }
        }
        Some(Position { page: best.page, x: best.x, y: best.top(), width: best.width, height: best.height + best.depth })
    }

    fn box_containing(&self, page: usize, x: f64, y: f64) -> Option<Record> {
        self.by_page
            .get(&page)?
            .iter()
            .map(|&i| self.records[i])
            .filter(|r| r.boxed && r.contains(x, y))
            .min_by(|a, b| a.area().total_cmp(&b.area()))
    }

    fn nearest_line(&self, tag: usize, line: usize) -> Option<usize> {
        self.lines_by_tag.get(&tag)?.iter().copied().min_by_key(|l| l.abs_diff(line))
    }

    /// A point on a page (1-based) → the file and line that produced it.
    pub fn inverse(&self, page: usize, x: f64, y: f64) -> Option<Source> {
        let indexes = self.by_page.get(&page)?;
        let mut best: Option<usize> = None;
        let mut smallest_area = 0.0;
        let mut best_distance = -1.0f64;
        for &i in indexes {
            let r = &self.records[i];
            if r.boxed && r.contains(x, y) {
                let area = r.area();
                if best_distance != 0.0 || area < smallest_area {
                    best = Some(i);
                    smallest_area = area;
                    best_distance = 0.0;
                }
                continue;
            }
            if best_distance == 0.0 {
                continue;
            }
            let d = distance_to(r, x, y);
            if best_distance < 0.0 || d < best_distance {
                best = Some(i);
                best_distance = d;
            }
        }
        let mut r = self.records[best?];
        if best_distance == 0.0 {
            r = self.node_in(&r, page, x, y).unwrap_or(r);
        }
        Some(Source { file: self.files.get(&r.tag)?.clone(), line: r.line })
    }

    /// A line's box carries the line where its paragraph ended, which may be
    /// in the file that \input the one the words came from. The nodes in it
    /// (characters, glue, kerns) carry their own: the one on the clicked
    /// line, at or just left of the click.
    fn node_in(&self, outer: &Record, page: usize, x: f64, y: f64) -> Option<Record> {
        self.by_page
            .get(&page)?
            .iter()
            .map(|&i| self.records[i])
            .filter(|r| !r.boxed && self.files.contains_key(&r.tag))
            .filter(|r| r.x >= outer.x && r.x <= outer.x + outer.width && r.y >= outer.top() && r.y <= outer.bottom())
            .min_by(|a, b| {
                let key = |r: &Record| ((r.y - y).abs(), r.x > x, (r.x - x).abs());
                let (ka, kb) = (key(a), key(b));
                ka.0.total_cmp(&kb.0).then(ka.1.cmp(&kb.1)).then(ka.2.total_cmp(&kb.2))
            })
    }
}

fn distance_to(r: &Record, x: f64, y: f64) -> f64 {
    let dx = if x < r.x {
        r.x - x
    } else if x > r.x + r.width {
        x - (r.x + r.width)
    } else {
        0.0
    };
    let dy = if y < r.top() {
        r.top() - y
    } else if y > r.bottom() {
        y - r.bottom()
    } else {
        0.0
    };
    dx * dx + dy * dy
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "SyncTeX Version:1\nInput:1:/p/main.tex\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n{1\n[1,5:0,65536000:65536000,6553600,0\n(1,6:4736286,8799518:22609920,655360,196608\nx1,7:4736286,9799518\n)\n]\n}1\n";

    #[test]
    fn forward_and_inverse() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.synctex");
        fs::write(&path, SAMPLE).unwrap();
        let m = parse(&path).unwrap();
        let pos = m.forward(Path::new("/p/main.tex"), 6).unwrap();
        assert_eq!(pos.page, 1);
        assert!((pos.x - 72.27).abs() < 0.1);
        assert!(pos.height > 0.0);
        let src = m.inverse(1, pos.x + 1.0, pos.y + 1.0).unwrap();
        assert_eq!(src.line, 6);
        // Line 3 produced nothing: nearest line wins.
        assert!(m.forward(Path::new("/p/main.tex"), 3).is_some());
    }

    /// Page 2 comes from a file opened after page 1 was shipped out.
    const SAMPLE_LATE_INPUT: &str = "SyncTeX Version:1\nInput:1:/p/main.tex\nInput:2:\nOutput:pdf\nMagnification:1000\nUnit:1\nX Offset:0\nY Offset:0\nContent:\n{1\n[1,5:0,65536000:65536000,6553600,0\n(1,6:4736286,8799518:22609920,655360,196608\n)\n]\n}1\nInput:3:/p/cap/dois.tex\n{2\n[1,6:0,65536000:65536000,6553600,0\n(3,1:4736286,8799518:22609920,655360,196608\n)\n]\n}2\n";

    #[test]
    fn inputs_declared_among_the_pages() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("main.synctex");
        fs::write(&path, SAMPLE_LATE_INPUT).unwrap();
        let m = parse(&path).unwrap();
        let pos = m.forward(Path::new("/p/cap/dois.tex"), 1).unwrap();
        assert_eq!(pos.page, 2);
        let src = m.inverse(2, pos.x + 1.0, pos.y + 1.0).unwrap();
        assert_eq!(src, Source { file: PathBuf::from("/p/cap/dois.tex"), line: 1 });
        // Tectonic's bundle files have no path: not the project folder.
        assert!(!m.files.contains_key(&2));
    }

    const SYNC_SOURCE: &str = "\\documentclass{article}\n\\usepackage[margin=2cm]{geometry}\n\\begin{document}\n\
\\section{Primeira secao}\nLinha de texto numero um do documento.\n\nLinha de texto numero dois, um pouco mais longa que a anterior.\n\
\\newpage\nEsta linha esta na segunda pagina.\n\\end{document}\n";

    /// The .pdf/.synctex.gz pair of a known document; `None` where pdflatex
    /// is not installed.
    fn compile_with_synctex() -> Option<(tempfile::TempDir, PathBuf)> {
        if !crate::backend::latex::which("pdflatex") {
            return None;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let tex = root.join("m.tex");
        fs::write(&tex, SYNC_SOURCE).unwrap();
        let out = std::process::Command::new("pdflatex")
            .args(["-interaction=nonstopmode", "-halt-on-error", "-synctex=1", "m.tex"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(out.status.success(), "pdflatex failed:\n{}", String::from_utf8_lossy(&out.stdout));
        assert!(root.join("m.synctex.gz").exists());
        Some((dir, tex))
    }

    /// The conversion has three traps: the unit (65536 sp per point), the
    /// origin (top, not bottom) and Y being the baseline, not the top of the
    /// box. Getting any wrong "almost" works, so check against the word
    /// boxes the PDF reader finds, an independent measurement of the same PDF.
    #[test]
    fn coordinates_match_the_pdf() {
        let Some((_dir, tex)) = compile_with_synctex() else { return };
        let m = parse(&tex.with_extension("synctex.gz")).unwrap();
        let pdf = tex.with_extension("pdf");
        // `word` is the first word of the printed line: in the heading that
        // is the section number.
        for (line, want_page, word) in [(4, 1, "1"), (5, 1, "Linha"), (9, 2, "Esta")] {
            let pos = m.forward(&tex, line).unwrap_or_else(|| panic!("line {line}: nothing found"));
            assert_eq!(pos.page, want_page, "line {line}");
            let words = crate::backend::pdf::words(&pdf, want_page).unwrap();
            let truth = words.iter().find(|w| w.text == word).unwrap_or_else(|| panic!("{word:?} not on page"));
            let diff = pos.y - truth.y0 as f64;
            assert!(diff.abs() <= 1.5, "line {line}: synctex top {:.2}, PDF {:.2}", pos.y, truth.y0);
            assert!(pos.x >= 0.0 && pos.x <= truth.x0 as f64 + 1.5, "line {line}: box at x={:.2}, word at {:.2}", pos.x, truth.x0);
            assert!(pos.height > 0.0, "line {line}: highlight with no height");
        }
    }

    /// The position the forward search returned leads back to its line.
    #[test]
    fn inverse_returns_to_the_right_line() {
        let Some((_dir, tex)) = compile_with_synctex() else { return };
        let m = parse(&tex.with_extension("synctex.gz")).unwrap();
        for line in [4, 5, 9] {
            let pos = m.forward(&tex, line).unwrap();
            let src = m.inverse(pos.page, pos.x + pos.width / 2.0, pos.y + pos.height / 2.0).unwrap();
            assert_eq!(src.file.file_name().unwrap(), "m.tex");
            // One line off is fine: a wrapped paragraph can be attributed to the next line.
            assert!(src.line.abs_diff(line) <= 1, "line {line} came back as {}", src.line);
        }
    }

    /// A click on text that came from an \input or \include leads to that
    /// file, and its lines can be found in the PDF.
    #[test]
    fn included_files_both_ways() {
        if !crate::backend::latex::which("pdflatex") {
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        fs::create_dir(root.join("cap")).unwrap();
        fs::write(
            root.join("m.tex"),
            "\\documentclass{article}\n\\begin{document}\nTexto principal.\n\n\\input{cap/um}\n\\include{cap/dois}\n\\input{tres.tex}\n\\end{document}\n",
        )
        .unwrap();
        fs::write(root.join("cap/um.tex"), "Primeiro capitulo.\n").unwrap();
        fs::write(root.join("cap/dois.tex"), "Segundo capitulo.\n").unwrap();
        fs::write(root.join("tres.tex"), "Terceiro arquivo.\n").unwrap();
        let out = std::process::Command::new("pdflatex")
            .args(["-interaction=nonstopmode", "-halt-on-error", "-synctex=1", "m.tex"])
            .current_dir(&root)
            .output()
            .unwrap();
        assert!(out.status.success(), "pdflatex failed:\n{}", String::from_utf8_lossy(&out.stdout));
        let m = parse(&root.join("m.synctex.gz")).unwrap();
        for file in ["cap/um.tex", "cap/dois.tex", "tres.tex"] {
            let file = root.join(file);
            let pos = m.forward(&file, 1).unwrap_or_else(|| panic!("{}: not in the map", file.display()));
            let src = m.inverse(pos.page, pos.x + 1.0, pos.y + pos.height / 2.0).unwrap();
            assert_eq!(src, Source { file, line: 1 });
        }
    }

    #[test]
    fn blank_line_falls_back_and_unknown_file_misses() {
        let Some((_dir, tex)) = compile_with_synctex() else { return };
        let m = parse(&tex.with_extension("synctex.gz")).unwrap();
        assert!(m.forward(&tex, 6).is_some(), "line 6 is blank");
        assert!(m.forward(Path::new("/tmp/not-part-of-it.tex"), 1).is_none());
    }
}
