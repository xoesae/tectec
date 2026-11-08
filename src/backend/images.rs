//! The image library: every raster/SVG image in the project with where it is
//! used and whether that usage is missing a caption or a label.

use super::index::{relative_slash, walk_files};
use super::project::{self, ext_lower, kind_of, unique_path, Kind, Res};
use regex::Regex;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use crate::tf;

#[derive(Clone, Debug)]
pub struct Usage {
    pub file: PathBuf,
    pub line: usize,
}

#[derive(Clone, Debug)]
pub struct Image {
    pub name: String,
    pub path: PathBuf,
    /// What goes inside `\includegraphics{}`: relative to the main file.
    pub rel_path: String,
    pub size: u64,
    pub uses: Vec<Usage>,
    pub missing_caption: bool,
    pub missing_label: bool,
}

/// `\includegraphics` resolves relative to the main file's folder.
fn figure_base(root: &Path, main_file: Option<&Path>) -> PathBuf {
    main_file
        .and_then(|m| project::resolve(root, m).ok())
        .and_then(|m| m.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| root.to_path_buf())
}

static BEGIN_FIG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\begin\{(?:figure|wrapfigure|SCfigure)\*?\}").unwrap());
static END_FIG_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\end\{(?:figure|wrapfigure|SCfigure)\*?\}").unwrap());
static CAPTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\caption\b").unwrap());
static LABEL_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\\label\{").unwrap());
static GRAPHICS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\\includegraphics(?:\[[^\]]*\])?\{([^}]*)\}").unwrap());

pub fn list(root: &Path, main_file: Option<&Path>) -> Res<Vec<Image>> {
    let base = figure_base(root, main_file);
    let mut images: Vec<Image> = Vec::new();
    let mut by_key: HashMap<String, Vec<usize>> = HashMap::new();
    walk_files(root, &mut |path, name| {
        if !matches!(kind_of(name, false), Kind::Image | Kind::Svg) {
            return;
        }
        let rel = relative_slash(&base, path);
        let idx = images.len();
        images.push(Image {
            name: name.to_string(),
            path: path.to_path_buf(),
            rel_path: rel.clone(),
            size: fs::metadata(path).map(|m| m.len()).unwrap_or(0),
            uses: vec![],
            missing_caption: false,
            missing_label: false,
        });
        by_key.entry(rel.clone()).or_default().push(idx);
        if let Some(dot) = rel.rfind('.').filter(|d| !rel[*d..].contains('/')) {
            by_key.entry(rel[..dot].to_string()).or_default().push(idx);
        }
    });

    walk_files(root, &mut |path, name| {
        if !matches!(ext_lower(name).as_str(), "tex" | "sty" | "cls") {
            return;
        }
        scan_file(path, &mut |arg, line, has_caption, has_label| {
            let arg = arg.trim();
            let no_ext = match arg.rfind('.') {
                Some(d) if !arg[d..].contains('/') => &arg[..d],
                _ => arg,
            };
            let matches = by_key.get(arg).or_else(|| by_key.get(no_ext));
            for &i in matches.into_iter().flatten() {
                let img = &mut images[i];
                img.uses.push(Usage { file: path.to_path_buf(), line });
                img.missing_caption |= !has_caption;
                img.missing_label |= !has_label;
            }
        });
    });
    images.sort_by(|a, b| a.rel_path.cmp(&b.rel_path));
    Ok(images)
}

/// Reports each `\includegraphics{}` with whether its figure environment
/// has a `\caption`/`\label` anywhere before it closes.
fn scan_file(path: &Path, on_usage: &mut dyn FnMut(&str, usize, bool, bool)) {
    let Ok(text) = fs::read_to_string(path) else { return };
    let mut in_figure = false;
    let (mut has_caption, mut has_label) = (false, false);
    let mut pending: Vec<(String, usize)> = Vec::new();
    let mut flush = |pending: &mut Vec<(String, usize)>, cap: bool, lab: bool| {
        for (arg, line) in pending.drain(..) {
            on_usage(&arg, line, cap, lab);
        }
    };
    for (i, line) in text.lines().enumerate() {
        let number = i + 1;
        if BEGIN_FIG_RE.is_match(line) {
            flush(&mut pending, has_caption, has_label);
            in_figure = true;
            has_caption = false;
            has_label = false;
        }
        if in_figure {
            has_caption |= CAPTION_RE.is_match(line);
            has_label |= LABEL_RE.is_match(line);
        }
        for m in GRAPHICS_RE.captures_iter(line) {
            if in_figure {
                pending.push((m[1].to_string(), number));
            } else {
                pending.push((m[1].to_string(), number));
                flush(&mut pending, false, false);
            }
        }
        if in_figure && END_FIG_RE.is_match(line) {
            flush(&mut pending, has_caption, has_label);
            in_figure = false;
            has_caption = false;
            has_label = false;
        }
    }
    flush(&mut pending, has_caption, has_label);
}

/// The `\includegraphics{}` arguments of every source file, to tell a PDF
/// figure from a PDF to read.
pub struct GraphicsInUse {
    base: PathBuf,
    args: HashSet<String>,
}

impl GraphicsInUse {
    pub fn scan(root: &Path, main_file: Option<&Path>) -> Self {
        let mut args = HashSet::new();
        walk_files(root, &mut |path, name| {
            if matches!(ext_lower(name).as_str(), "tex" | "sty" | "cls") {
                scan_file(path, &mut |arg, _, _, _| {
                    args.insert(arg.trim().to_string());
                });
            }
        });
        Self { base: figure_base(root, main_file), args }
    }

    /// Whether `path` is included, with or without its extension.
    pub fn contains(&self, path: &Path) -> bool {
        let rel = relative_slash(&self.base, path);
        let no_ext = match rel.rfind('.') {
            Some(d) if !rel[d..].contains('/') => &rel[..d],
            _ => rel.as_str(),
        };
        self.args.contains(&rel) || self.args.contains(no_ext)
    }
}

/// Writes a pasted image into `<folder>/colado-<timestamp>.<ext>`, `folder`
/// being relative to the root.
pub fn save_clipboard(root: &Path, main_file: Option<&Path>, folder: &Path, data: &[u8], ext: &str) -> Res<Image> {
    if data.is_empty() {
        return Err("nada para colar".into());
    }
    let dir = root.join(folder);
    fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let name = format!("colado-{}.{}", chrono::Local::now().format("%Y%m%d-%H%M%S"), normalize_ext(ext));
    let path = unique_path(&dir.join(name));
    fs::write(&path, data).map_err(|e| e.to_string())?;
    let base = figure_base(root, main_file);
    Ok(Image {
        name: project::file_name(&path),
        rel_path: relative_slash(&base, &path),
        size: data.len() as u64,
        path,
        uses: vec![],
        missing_caption: false,
        missing_label: false,
    })
}

/// Copies an image file from anywhere into `folder` (relative to the root),
/// keeping its name (disambiguated, never overwriting). An image already
/// inside the project is used where it is.
pub fn import_file(root: &Path, main_file: Option<&Path>, folder: &Path, src: &Path) -> Res<Image> {
    if !matches!(kind_of(&project::file_name(src), false), Kind::Image | Kind::Svg) && ext_lower(src) != "pdf" {
        return Err(tf!("{name} is not an image", name = project::file_name(src)));
    }
    let base = figure_base(root, main_file);
    let root_abs = fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf());
    let src_abs = fs::canonicalize(src).map_err(|e| e.to_string())?;
    let path = if src_abs.starts_with(&root_abs) {
        src_abs
    } else {
        let dir = root.join(folder);
        fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let dest = unique_path(&dir.join(src.file_name().unwrap_or_default()));
        fs::copy(src, &dest).map_err(|e| e.to_string())?;
        dest
    };
    Ok(Image {
        name: project::file_name(&path),
        rel_path: relative_slash(&base, &path),
        size: fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        path,
        uses: vec![],
        missing_caption: false,
        missing_label: false,
    })
}

/// A known image extension (lowercase, no dot); anything else becomes `png`.
fn normalize_ext(ext: &str) -> String {
    let ext = ext.trim().trim_start_matches('.').to_lowercase();
    match ext.as_str() {
        "jpg" | "jpeg" | "png" | "gif" | "webp" | "bmp" | "svg" => ext,
        _ => "png".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn usages_and_missing_parts() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir(root.join("img")).unwrap();
        fs::write(root.join("img/a.png"), [0u8]).unwrap();
        fs::write(root.join("img/b.png"), [0u8]).unwrap();
        fs::write(root.join("img/c.png"), [0u8]).unwrap();
        fs::write(
            root.join("main.tex"),
            "\\begin{figure}\n\\includegraphics{img/a}\n\\caption{A}\n\\label{fig:a}\n\\end{figure}\n\\includegraphics[width=1cm]{img/b.png}\n",
        )
        .unwrap();
        let l = list(root, Some(&root.join("main.tex"))).unwrap();
        let get = |n: &str| l.iter().find(|i| i.name == n).unwrap();
        assert_eq!(get("a.png").uses.len(), 1);
        assert!(!get("a.png").missing_caption && !get("a.png").missing_label);
        assert!(get("b.png").missing_caption && get("b.png").missing_label);
        assert!(get("c.png").uses.is_empty());
    }

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    #[test]
    fn unused_image() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("img/photo.png"), "fake png");
        write(&root.join("main.tex"), "\\documentclass{article}\n");
        let l = list(root, Some(&root.join("main.tex"))).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].rel_path, "img/photo.png");
        assert!(l[0].uses.is_empty());
    }

    #[test]
    fn bare_includegraphics_misses_caption_and_label() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("img/photo.png"), "fake png");
        write(&root.join("main.tex"), "\\documentclass{article}\n\\begin{document}\n\\includegraphics{img/photo}\n\\end{document}\n");
        let l = list(root, Some(&root.join("main.tex"))).unwrap();
        assert_eq!(l[0].uses.len(), 1);
        assert_eq!(l[0].uses[0].line, 3);
        assert!(l[0].missing_caption && l[0].missing_label);
    }

    #[test]
    fn figure_with_caption_and_label() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("img/photo.png"), "fake png");
        write(
            &root.join("main.tex"),
            "\\begin{document}\n\\begin{figure}[htb]\n    \\centering\n    \\includegraphics[width=0.8\\linewidth]{img/photo.png}\n\
             \\caption{Uma foto}\n    \\label{fig:photo}\n\\end{figure}\n\\end{document}\n",
        );
        let l = list(root, Some(&root.join("main.tex"))).unwrap();
        assert_eq!(l[0].uses.len(), 1);
        assert!(!l[0].missing_caption && !l[0].missing_label);
    }

    #[test]
    fn figure_missing_caption_and_label() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("img/photo.png"), "fake png");
        write(&root.join("main.tex"), "\\begin{figure}[htb]\n    \\includegraphics{img/photo.png}\n\\end{figure}\n");
        let l = list(root, Some(&root.join("main.tex"))).unwrap();
        assert!(l[0].missing_caption && l[0].missing_label);
    }

    #[test]
    fn rel_path_is_relative_to_the_main_file() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("chapters/img/diagram.png"), "fake png");
        write(&root.join("chapters/main.tex"), "\\documentclass{article}\n");
        let l = list(root, Some(&root.join("chapters/main.tex"))).unwrap();
        assert_eq!(l[0].rel_path, "img/diagram.png");
        let l = list(root, None).unwrap();
        assert_eq!(l[0].rel_path, "chapters/img/diagram.png");
    }

    #[test]
    fn ignores_non_image_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("main.tex"), "\\documentclass{article}\n");
        write(&root.join("refs.bib"), "@article{a,}");
        write(&root.join("output.pdf"), "%PDF fake");
        assert!(list(root, None).unwrap().is_empty());
    }

    #[test]
    fn save_clipboard_writes_and_lists() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let main = root.join("main.tex");
        write(&main, "\\documentclass{article}\n");
        let img = save_clipboard(root, Some(&main), Path::new("images"), b"fake png bytes", "png").unwrap();
        assert_eq!(img.path.parent().unwrap(), root.join("images"));
        assert_eq!(fs::read(&img.path).unwrap(), b"fake png bytes");
        let l = list(root, Some(&main)).unwrap();
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].path, img.path);
        // Same second: disambiguated, not overwritten.
        let again = save_clipboard(root, Some(&main), Path::new("images"), b"x", "png").unwrap();
        assert_ne!(again.path, img.path);
    }

    #[test]
    fn save_clipboard_rejects_empty() {
        let dir = tempfile::tempdir().unwrap();
        assert!(save_clipboard(dir.path(), None, Path::new("images"), &[], "png").is_err());
    }

    #[test]
    fn save_clipboard_normalizes_the_extension() {
        let dir = tempfile::tempdir().unwrap();
        let img = save_clipboard(dir.path(), None, Path::new("images"), b"x", "weird").unwrap();
        assert_eq!(ext_lower(&img.path), "png");
        let img = save_clipboard(dir.path(), None, Path::new("images"), b"x", ".JPG").unwrap();
        assert_eq!(ext_lower(&img.path), "jpg");
        assert_eq!(normalize_ext("../../x"), "png");
    }

    #[test]
    fn import_copies_outside_images_and_reuses_inside_ones() {
        let (root, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let src = other.path().join("grafico.png");
        write(&src, "png");
        let img = import_file(root.path(), None, Path::new("images"), &src).unwrap();
        assert_eq!(img.rel_path, "images/grafico.png");
        // Same name again: a copy beside it, never overwriting.
        let again = import_file(root.path(), None, Path::new("images"), &src).unwrap();
        assert_ne!(again.path, img.path);
        // Already in the project: used in place.
        write(&root.path().join("fig/a.png"), "png");
        let inside = import_file(root.path(), None, Path::new("images"), &root.path().join("fig/a.png")).unwrap();
        assert_eq!(inside.rel_path, "fig/a.png");
        write(&other.path().join("notas.txt"), "x");
        assert!(import_file(root.path(), None, Path::new("images"), &other.path().join("notas.txt")).is_err());
    }

    #[test]
    fn imports_into_the_configured_folder() {
        let (root, other) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let main = root.path().join("tese/main.tex");
        write(&main, "\\documentclass{article}\n");
        let src = other.path().join("mapa.png");
        write(&src, "png");
        let img = import_file(root.path(), Some(&main), Path::new("assets/figuras"), &src).unwrap();
        assert_eq!(img.path.parent().unwrap(), root.path().join("assets/figuras"));
        assert_eq!(img.rel_path, "../assets/figuras/mapa.png", "relative to the main file");
    }

    #[test]
    fn graphics_in_use_with_or_without_extension() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        write(&root.join("main.tex"), "\\includegraphics{fig/plot}\n\\includegraphics[width=2cm]{fig/logo.pdf}\n");
        let used = GraphicsInUse::scan(root, Some(&root.join("main.tex")));
        assert!(used.contains(&root.join("fig/plot.pdf")));
        assert!(used.contains(&root.join("fig/logo.pdf")));
        assert!(!used.contains(&root.join("papers/knuth.pdf")));
    }
}
