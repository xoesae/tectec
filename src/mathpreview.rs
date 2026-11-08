//! Inline math preview: hovering over `$...$` or `\[...\]` in the editor shows
//! the formula typeset, without waiting for a full compile, the port of the
//! old frontend's KaTeX tooltip (mathPreview.ts). There is no KaTeX here, so
//! the formula goes through the same toolchain the document already needs:
//! pdflatex on a one-formula `standalone` document, then rasterized. That
//! takes a moment, so it runs on a background thread and every result is
//! cached by formula.
//!
//! `find_math_at` is a pragmatic filter (regex over a bounded window), not a
//! parser, the same tradeoff the old frontend and `texparse` make.

use crate::backend::latex;
use crate::i18n::t;
use crate::tf;
use regex::Regex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

#[derive(Clone, Debug, PartialEq)]
pub struct MathMatch {
    /// Char indices of the whole match, delimiters included.
    pub from: usize,
    pub to: usize,
    pub formula: String,
    /// `\[...\]` (block) vs `$...$` (inline).
    pub display: bool,
}

// Single line only, tolerant of an escaped `\$` inside the formula. Known,
// accepted gaps (same as the old frontend): no `\(...\)`/`$$...$$`.
static INLINE_MATH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$(?:\\.|[^$\\\n])*\$").unwrap());
static DISPLAY_MATH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\\\[.*?\\\]").unwrap());

/// `\[...\]` often spans several lines, but scanning the whole document on
/// every hover would not scale for a thesis: only this many lines around the
/// pointer are looked at.
const DISPLAY_WINDOW_LINES: usize = 25;

/// The math span under char index `pos`, if any.
pub fn find_math_at(text: &str, pos: usize) -> Option<MathMatch> {
    let byte = char_to_byte(text, pos);
    let line_start = text[..byte].rfind('\n').map(|b| b + 1).unwrap_or(0);
    let line_end = text[byte..].find('\n').map(|b| byte + b).unwrap_or(text.len());

    for m in INLINE_MATH_RE.find_iter(&text[line_start..line_end]) {
        let (a, b) = (line_start + m.start(), line_start + m.end());
        if byte >= a && byte <= b {
            return Some(MathMatch {
                from: byte_to_char(text, a),
                to: byte_to_char(text, b),
                formula: text[a + 1..b - 1].to_string(),
                display: false,
            });
        }
    }

    let mut win_start = line_start;
    for _ in 0..DISPLAY_WINDOW_LINES {
        if win_start == 0 {
            break;
        }
        win_start = text[..win_start - 1].rfind('\n').map(|b| b + 1).unwrap_or(0);
    }
    let mut win_end = line_end;
    for _ in 0..DISPLAY_WINDOW_LINES {
        if win_end >= text.len() {
            break;
        }
        win_end = text[win_end + 1..].find('\n').map(|b| win_end + 1 + b).unwrap_or(text.len());
    }
    for m in DISPLAY_MATH_RE.find_iter(&text[win_start..win_end]) {
        let (a, b) = (win_start + m.start(), win_start + m.end());
        if byte >= a && byte <= b {
            return Some(MathMatch {
                from: byte_to_char(text, a),
                to: byte_to_char(text, b),
                formula: text[a + 2..b - 2].trim().to_string(),
                display: true,
            });
        }
    }
    None
}

fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

fn byte_to_char(s: &str, bi: usize) -> usize {
    s[..bi].chars().count()
}

/// The one-formula document handed to pdflatex. Plain `article` (not
/// `standalone`, which a minimal TeX install lacks); the raster is cropped
/// to the ink afterwards. `\displaystyle` keeps a `\[...\]` block looking
/// like one. `preamble` brings the project's math packages and macros.
pub fn snippet_document(formula: &str, display: bool, preamble: &str) -> String {
    let style = if display { "\\displaystyle " } else { "" };
    format!(
        "\\documentclass{{article}}\n\\usepackage{{amsmath}}\n\\usepackage{{amssymb}}\n{preamble}\\pagestyle{{empty}}\n\
         \\begin{{document}}\n\\noindent${style}{formula}$\n\\end{{document}}\n"
    )
}

/// Rendering resolution. The tooltip shows it at `DISPLAY_SCALE` of its pixel
/// size so it stays sharp on HiDPI screens.
const DPI: u32 = 240;
const DISPLAY_SCALE: f32 = 0.5;
const RENDER_TIMEOUT: Duration = Duration::from_secs(20);
/// How long the pointer has to rest over a formula before it is rendered.
const HOVER_DELAY: f64 = 0.35;
const CACHE_LIMIT: usize = 256;

/// The formula, whether it is displayed, and a hash of the preamble it was
/// rendered with.
type Key = (String, bool, u64);

enum Entry {
    Pending,
    /// White glyphs on alpha, tinted with the theme's text colour when drawn.
    Image(egui::ColorImage),
    Texture(egui::TextureHandle),
    Failed(String),
}

#[derive(Default)]
pub struct MathPreview {
    cache: Arc<Mutex<HashMap<Key, Entry>>>,
    /// The formula under the pointer and since when.
    hovering: Option<(Key, f64)>,
    /// The project's math packages and macros, and their hash.
    preamble: Arc<String>,
    preamble_hash: u64,
}

impl MathPreview {
    /// Sets the project's math packages and macros, so formulas using them
    /// render as in the document.
    pub fn set_preamble(&mut self, preamble: &str) {
        if preamble != self.preamble.as_str() {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            preamble.hash(&mut h);
            self.preamble_hash = h.finish();
            self.preamble = Arc::new(preamble.to_string());
        }
    }

    /// Called by the editor every frame with what the pointer is over (or
    /// nothing); shows the tooltip once the pointer has rested long enough.
    pub fn hover(&mut self, ctx: &egui::Context, hit: Option<(&MathMatch, egui::Rect)>, text_color: egui::Color32) {
        let Some((m, anchor)) = hit.filter(|(m, _)| !m.formula.trim().is_empty()) else {
            self.hovering = None;
            return;
        };
        let key: Key = (m.formula.clone(), m.display, self.preamble_hash);
        let now = ctx.input(|i| i.time);
        let since = match &self.hovering {
            Some((k, t)) if *k == key => *t,
            _ => {
                self.hovering = Some((key.clone(), now));
                now
            }
        };
        if now - since < HOVER_DELAY {
            ctx.request_repaint_after(Duration::from_secs_f64(HOVER_DELAY - (now - since) + 0.01));
            return;
        }
        self.request(ctx, &key);

        let mut cache = self.cache.lock().unwrap();
        if let Some(Entry::Image(_)) = cache.get(&key) {
            if let Some(Entry::Image(img)) = cache.remove(&key) {
                let tex = ctx.load_texture(format!("math:{}:{}", key.1, key.0), img, egui::TextureOptions::LINEAR);
                cache.insert(key.clone(), Entry::Texture(tex));
            }
        }
        let entry = cache.get(&key);
        egui::Area::new(egui::Id::new("math-preview"))
            .order(egui::Order::Tooltip)
            .pivot(egui::Align2::LEFT_BOTTOM)
            .fixed_pos(anchor.left_top() - egui::vec2(0.0, 4.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| match entry {
                    Some(Entry::Texture(tex)) => {
                        let size = tex.size_vec2() * DISPLAY_SCALE;
                        ui.add(egui::Image::new((tex.id(), size)).tint(text_color));
                    }
                    Some(Entry::Failed(msg)) => {
                        ui.weak(msg);
                    }
                    _ => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.weak(t("Rendering the formula…"));
                        });
                    }
                });
            });
    }

    fn request(&self, ctx: &egui::Context, key: &Key) {
        let mut cache = self.cache.lock().unwrap();
        if cache.contains_key(key) {
            return;
        }
        if cache.len() >= CACHE_LIMIT {
            cache.retain(|_, e| matches!(e, Entry::Pending));
        }
        cache.insert(key.clone(), Entry::Pending);
        drop(cache);
        let (cache, key, ctx, preamble) = (self.cache.clone(), key.clone(), ctx.clone(), self.preamble.clone());
        std::thread::spawn(move || {
            // A preamble that does not compile here (a package missing, a
            // definition that needs the document class) must not cost every
            // formula its preview: without it, most still render.
            let rendered = match render(&key.0, key.1, &preamble) {
                Err(_) if !preamble.is_empty() => render(&key.0, key.1, ""),
                r => r,
            };
            let entry = match rendered {
                Ok(img) => Entry::Image(img),
                Err(e) => Entry::Failed(e),
            };
            cache.lock().unwrap().insert(key, entry);
            ctx.request_repaint();
        });
    }
}

/// A scratch directory removed when dropped.
struct TempDir(PathBuf);

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn temp_dir() -> Result<TempDir, String> {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("tectec-math-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    Ok(TempDir(dir))
}

fn render(formula: &str, display: bool, preamble: &str) -> Result<egui::ColorImage, String> {
    render_document(&snippet_document(formula, display, preamble)).map_err(|e| match e {
        RenderError::Tex(Some(message)) => tf!("Invalid formula: {message}", message),
        RenderError::Tex(None) => t("Invalid formula").into(),
        RenderError::Empty => t("Empty formula").into(),
        RenderError::Other(e) => e,
    })
}

/// Why a document could not be rendered.
pub enum RenderError {
    /// LaTeX stopped, with the `! message` it gave.
    Tex(Option<String>),
    /// It typeset to a blank page.
    Empty,
    Other(String),
}

/// Typesets a small standalone document and returns the inked part of its
/// first page as white glyphs on alpha (tinted when drawn), at `DPI`; shown
/// at `DISPLAY_SCALE` of its size. With the built-in LaTeX or the system's,
/// as the preferences say.
pub fn render_document(doc: &str) -> Result<egui::ColorImage, RenderError> {
    if !latex::system_tex() {
        let pdf = crate::backend::embedded::typeset(doc).map_err(|log| RenderError::Tex(first_message(&log)))?;
        let dir = temp_dir().map_err(RenderError::Other)?;
        let path = dir.0.join("formula.pdf");
        std::fs::write(&path, pdf).map_err(|e| RenderError::Other(e.to_string()))?;
        let img = crate::backend::pdf::render(&path, 1, DPI).map_err(RenderError::Other)?;
        let img = crop_to_ink(&img).ok_or(RenderError::Empty)?;
        return Ok(to_alpha_mask(&img));
    }
    if !latex::which("pdflatex") {
        return Err(RenderError::Other(t("pdflatex not found, so the formula preview is unavailable").into()));
    }
    let dir = temp_dir().map_err(RenderError::Other)?;
    let tex = dir.0.join("formula.tex");
    std::fs::write(&tex, doc).map_err(|e| RenderError::Other(e.to_string()))?;
    let args: Vec<String> = vec![
        "-interaction=nonstopmode".into(),
        "-halt-on-error".into(),
        "-no-shell-escape".into(),
        "formula.tex".into(),
    ];
    let cancel = AtomicBool::new(false);
    let log = latex::run(&cancel, Instant::now() + RENDER_TIMEOUT, &dir.0, "pdflatex", &args)
        .map_err(|_| RenderError::Other(t("The formula took too long to render").into()))?;
    let pdf = dir.0.join("formula.pdf");
    if !pdf.is_file() {
        return Err(RenderError::Tex(first_message(&log)));
    }
    let img = crate::backend::pdf::render(Path::new(&pdf), 1, DPI).map_err(RenderError::Other)?;
    let img = crop_to_ink(&img).ok_or(RenderError::Empty)?;
    Ok(to_alpha_mask(&img))
}

/// The size an image from `render_document` is shown at.
pub fn display_size(tex: &egui::TextureHandle) -> egui::Vec2 {
    tex.size_vec2() * DISPLAY_SCALE
}

/// The `! message` LaTeX stopped on.
fn first_message(log: &str) -> Option<String> {
    log.lines().find_map(|l| l.strip_prefix("! ")).map(|m| m.trim().to_string())
}

/// The part of the page with ink on it, plus a small margin.
fn crop_to_ink(img: &image::RgbaImage) -> Option<image::RgbaImage> {
    const MARGIN: u32 = 4;
    let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
    for (x, y, p) in img.enumerate_pixels() {
        if p[0].min(p[1]).min(p[2]) < 250 {
            x0 = x0.min(x);
            y0 = y0.min(y);
            x1 = x1.max(x);
            y1 = y1.max(y);
        }
    }
    if x0 > x1 {
        return None;
    }
    let (x0, y0) = (x0.saturating_sub(MARGIN), y0.saturating_sub(MARGIN));
    let (x1, y1) = ((x1 + MARGIN).min(img.width() - 1), (y1 + MARGIN).min(img.height() - 1));
    Some(image::imageops::crop_imm(img, x0, y0, x1 - x0 + 1, y1 - y0 + 1).to_image())
}

/// Black-on-white raster → white glyphs whose alpha is the ink, so the
/// tooltip can tint it to the theme's text colour in light and dark alike.
fn to_alpha_mask(img: &image::RgbaImage) -> egui::ColorImage {
    let size = [img.width() as usize, img.height() as usize];
    let pixels = img
        .pixels()
        .map(|p| {
            let lum = (p[0] as u32 * 299 + p[1] as u32 * 587 + p[2] as u32 * 114) / 1000;
            egui::Color32::from_white_alpha(255 - lum as u8)
        })
        .collect();
    egui::ColorImage { size, pixels, source_size: egui::vec2(size[0] as f32, size[1] as f32) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(text: &str, needle: &str) -> usize {
        text[..text.find(needle).unwrap()].chars().count()
    }

    #[test]
    fn inline_math_under_pointer() {
        let t = "Seja $x^2 + y$ e depois $z$.";
        let m = find_math_at(t, at(t, "^2")).unwrap();
        assert_eq!(m.formula, "x^2 + y");
        assert!(!m.display);
        assert_eq!(&t.chars().skip(m.from).take(m.to - m.from).collect::<String>(), "$x^2 + y$");
        assert_eq!(find_math_at(t, at(t, "z$")).unwrap().formula, "z");
        assert_eq!(find_math_at(t, at(t, "depois")), None);
    }

    #[test]
    fn escaped_dollar_stays_inside() {
        let t = r"custa $\$5 + x$ ok";
        assert_eq!(find_math_at(t, at(t, "+")).unwrap().formula, r"\$5 + x");
    }

    #[test]
    fn inline_math_does_not_span_lines() {
        let t = "abre $x\nfecha y$";
        assert_eq!(find_math_at(t, at(t, "x")), None);
    }

    #[test]
    fn display_math_across_lines() {
        let t = "Texto ação\n\\[\n  \\int_0^1 f(x)\\,dx\n\\]\nfim";
        let m = find_math_at(t, at(t, "int")).unwrap();
        assert!(m.display);
        assert_eq!(m.formula, r"\int_0^1 f(x)\,dx");
        assert_eq!(find_math_at(t, at(t, "fim")), None);
    }

    #[test]
    fn display_math_outside_window_is_missed() {
        let mut t = String::from("\\[\n");
        for _ in 0..(DISPLAY_WINDOW_LINES + 5) {
            t.push_str("a +\n");
        }
        t.push_str("b\n\\]\n");
        let pos = t.chars().count() - 6;
        assert_eq!(find_math_at(&t, pos), None);
    }

    #[test]
    fn snippet_wraps_formula() {
        let doc = snippet_document(r"\frac{a}{b}", true, "\\newcommand{\\R}{\\mathbb{R}}\n");
        assert!(doc.contains(r"$\displaystyle \frac{a}{b}$"));
        assert!(doc.contains("article"));
        assert!(doc.find(r"\newcommand{\R}").unwrap() < doc.find(r"\begin{document}").unwrap());
        assert!(!snippet_document("x", false, "").contains("displaystyle"));
    }

    #[test]
    fn first_error_line() {
        assert_eq!(first_message("foo\n! Undefined control sequence.\nl.4"), Some("Undefined control sequence.".into()));
        assert_eq!(first_message("ok"), None);
    }

    /// End to end through the built-in LaTeX (the packages come from its
    /// cache, or the network the first time).
    #[test]
    fn renders_formulas() {
        let img = render(r"\frac{a}{b} + \sum_{i=1}^n i", true, "").unwrap();
        assert!(img.size[0] > 20 && img.size[1] > 20);
        assert!(img.pixels.iter().any(|p| p.a() > 200), "no ink");
        // The project's macros make it through.
        assert!(render(r"\norm{x}", false, "").is_err());
        assert!(render(r"\norm{x}", false, "\\newcommand{\\norm}[1]{\\lVert #1 \\rVert}\n").is_ok());
        let err = render(r"\nosuchmacro x", false, "").unwrap_err();
        assert!(err.contains("Undefined control sequence"), "{err}");
    }

    #[test]
    fn crop_keeps_only_ink() {
        let mut img = image::RgbaImage::from_pixel(100, 80, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(30, 20, image::Rgba([0, 0, 0, 255]));
        img.put_pixel(40, 25, image::Rgba([0, 0, 0, 255]));
        let c = crop_to_ink(&img).unwrap();
        assert_eq!((c.width(), c.height()), (11 + 8, 6 + 8));
        let blank = image::RgbaImage::from_pixel(10, 10, image::Rgba([255, 255, 255, 255]));
        assert!(crop_to_ink(&blank).is_none());
    }

    #[test]
    fn mask_turns_ink_into_alpha() {
        let mut img = image::RgbaImage::from_pixel(2, 1, image::Rgba([255, 255, 255, 255]));
        img.put_pixel(1, 0, image::Rgba([0, 0, 0, 255]));
        let m = to_alpha_mask(&img);
        assert_eq!(m.pixels[0].a(), 0);
        assert_eq!(m.pixels[1].a(), 255);
    }
}
