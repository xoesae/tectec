//! PDF page sizes, rasterization and the words on a page with their boxes,
//! read in-process with hayro (pure Rust): the app needs nothing installed
//! besides LaTeX itself, and a page no longer costs a process launch.

use hayro::hayro_interpret::font::Glyph;
use hayro::hayro_interpret::{
    BlendMode, ClipPath, Context, Device, FillRule, GlyphDrawMode, Image, InterpreterSettings, Paint, PathDrawMode,
    SoftMask,
};
use hayro::hayro_interpret::util::{RectExt, TransformExt};
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{RenderCache, RenderSettings};
use kurbo::{Affine, BezPath, Rect, Shape};
use std::path::Path;
use crate::i18n::t;
use crate::tf;

/// Page dimensions in PDF points (1/72 inch).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PageSize {
    pub width: f32,
    pub height: f32,
}

/// A word on a page, in points with the origin at the top left.
#[derive(Clone, Debug)]
pub struct Word {
    pub text: String,
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

fn open(path: &Path) -> Result<Pdf, String> {
    let data = std::fs::read(path).map_err(|e| tf!("could not read the PDF: {e}", e))?;
    Pdf::new(data).map_err(|e| tf!("unreadable PDF: {e}", e = format!("{e:?}")))
}

/// Every page's size in points (rotation applied), in order.
pub fn info(path: &Path) -> Result<Vec<PageSize>, String> {
    let pdf = open(path)?;
    let pages: Vec<PageSize> = pdf
        .pages()
        .iter()
        .map(|p| {
            let (width, height) = p.render_dimensions();
            PageSize { width, height }
        })
        .collect();
    if pages.is_empty() {
        return Err(t("PDF without pages").into());
    }
    Ok(pages)
}

/// Rasterizes one page (1-based) to exactly `width`×`height` pixels.
pub fn render_size(path: &Path, page: usize, width: u32, height: u32) -> Result<image::RgbaImage, String> {
    let pdf = open(path)?;
    let page = pdf.pages().get(page.wrapping_sub(1)).ok_or(t("no such page"))?;
    let (pw, ph) = page.render_dimensions();
    let (width, height) = (width.clamp(1, u16::MAX as u32), height.clamp(1, u16::MAX as u32));
    let settings = RenderSettings {
        x_scale: width as f32 / pw,
        y_scale: height as f32 / ph,
        width: Some(width as u16),
        height: Some(height as u16),
        bg_color: WHITE,
    };
    let cache = RenderCache::new();
    let pixmap = hayro::render(page, &cache, &InterpreterSettings::default(), &settings);
    // Opaque white background: premultiplied and straight RGBA coincide.
    let bytes = pixmap.data_as_u8_slice().to_vec();
    image::RgbaImage::from_raw(width, height, bytes).ok_or_else(|| "imagem de tamanho inesperado".to_string())
}

/// Rasterizes one page (1-based) at `dpi`.
pub fn render(path: &Path, page: usize, dpi: u32) -> Result<image::RgbaImage, String> {
    let pdf = open(path)?;
    let p = pdf.pages().get(page.wrapping_sub(1)).ok_or(t("no such page"))?;
    let (pw, ph) = p.render_dimensions();
    let scale = dpi as f32 / 72.0;
    drop(pdf);
    render_size(path, page, (pw * scale).round() as u32, (ph * scale).round() as u32)
}

// ---------- Text ----------

/// One glyph as it lands on the page: its text and box in points, top-left
/// origin.
#[derive(Clone, Debug)]
struct PlacedGlyph {
    text: String,
    rect: Rect,
    /// The baseline's y and the font size, to tell lines and words apart.
    baseline: f64,
    size: f64,
}

/// A device that only records where glyphs land.
#[derive(Default)]
struct TextDevice {
    glyphs: Vec<PlacedGlyph>,
}

impl<'a> Device<'a> for TextDevice {
    fn set_soft_mask(&mut self, _: Option<SoftMask<'a>>) {}
    fn set_blend_mode(&mut self, _: BlendMode) {}
    fn draw_path(&mut self, _: &BezPath, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
    fn push_clip_path(&mut self, _: &ClipPath) {}
    fn push_transparency_group(&mut self, _: f32, _: Option<SoftMask<'a>>, _: BlendMode) {}
    fn draw_glyph(&mut self, glyph: &Glyph<'a>, transform: Affine, glyph_transform: Affine, _: &Paint<'a>, _: &GlyphDrawMode) {
        let text = match glyph.as_unicode() {
            Some(hayro::hayro_interpret::hayro_cmap::BfString::Char(c)) => c.to_string(),
            Some(hayro::hayro_interpret::hayro_cmap::BfString::String(s)) => s,
            None => return,
        };
        // Glyph space has 1000 units per em.
        let full = transform * glyph_transform;
        // The glyph's em box: advance wide (or its outline when unknown), from
        // descender to ascender, so words get a box as tall as the line.
        let advance = match glyph {
            Glyph::Outline(g) => g.advance_width().map(|a| a as f64),
            _ => None,
        };
        let outline_box = match glyph {
            Glyph::Outline(g) => {
                let b = g.outline().bounding_box();
                (b.width() > 0.0).then_some(b)
            }
            _ => None,
        };
        let (x0, x1) = match (advance, outline_box) {
            (Some(a), _) if a > 0.0 => (0.0, a),
            (_, Some(b)) => (b.x0, b.x1),
            _ => (0.0, 500.0),
        };
        let em = Rect::new(x0, -220.0, x1, 750.0);
        let rect = full.transform_rect_bbox(em);
        let origin = full * kurbo::Point::ORIGIN;
        let size = full.transform_rect_bbox(Rect::new(0.0, 0.0, 1000.0, 1000.0)).height();
        self.glyphs.push(PlacedGlyph { text, rect, baseline: origin.y, size });
    }
    fn draw_image(&mut self, _: Image<'a, '_>, _: Affine) {}
    fn pop_clip_path(&mut self) {}
    fn pop_transparency_group(&mut self) {}
    fn draw_rect(&mut self, _: &Rect, _: Affine, _: &Paint<'a>, _: &PathDrawMode) {}
}

/// Groups glyphs, in content order, into words: a new word starts at a
/// space, a gap wider than a fraction of the font size, or a baseline change.
fn words_from_glyphs(glyphs: &[PlacedGlyph]) -> Vec<Word> {
    let mut words: Vec<Word> = Vec::new();
    let mut current: Option<(Word, f64, f64)> = None; // word, baseline, size
    let flush = |current: &mut Option<(Word, f64, f64)>, words: &mut Vec<Word>| {
        if let Some((w, _, _)) = current.take() {
            if !w.text.trim().is_empty() {
                words.push(w);
            }
        }
    };
    for g in glyphs {
        if g.text.trim().is_empty() {
            flush(&mut current, &mut words);
            continue;
        }
        let joins = current.as_ref().map(|(w, baseline, size)| {
            let same_line = (g.baseline - baseline).abs() < size.max(g.size) * 0.4;
            let gap = g.rect.x0 as f32 - w.x1;
            same_line && gap < (size.max(g.size) * 0.18) as f32 && gap > -(size.max(g.size) as f32)
        });
        match (joins, current.as_mut()) {
            (Some(true), Some((w, _, _))) => {
                w.text.push_str(&g.text);
                w.x0 = w.x0.min(g.rect.x0 as f32);
                w.y0 = w.y0.min(g.rect.y0 as f32);
                w.x1 = w.x1.max(g.rect.x1 as f32);
                w.y1 = w.y1.max(g.rect.y1 as f32);
            }
            _ => {
                flush(&mut current, &mut words);
                let word = Word {
                    text: g.text.clone(),
                    x0: g.rect.x0 as f32,
                    y0: g.rect.y0 as f32,
                    x1: g.rect.x1 as f32,
                    y1: g.rect.y1 as f32,
                };
                current = Some((word, g.baseline, g.size));
            }
        }
    }
    flush(&mut current, &mut words);
    for w in &mut words {
        w.text = compose_accents(&w.text);
    }
    words
}

/// The combining mark for an accent TeX drew as its own glyph (OT1 fonts
/// have no accented letters: "é" is "´" then "e", "ç" is "c" then "¸").
fn combining(accent: char) -> Option<char> {
    Some(match accent {
        '´' => '\u{301}',
        '`' => '\u{300}',
        'ˆ' | '^' => '\u{302}',
        '˜' | '~' => '\u{303}',
        '¨' => '\u{308}',
        'ˇ' => '\u{30C}',
        '˘' => '\u{306}',
        '˙' => '\u{307}',
        '˚' => '\u{30A}',
        '¯' => '\u{304}',
        '˝' => '\u{30B}',
        '¸' => '\u{327}',
        _ => return None,
    })
}

/// Joins accents drawn as separate glyphs to their letter and composes the
/// result: "s´erie" → "série", "Introdu¸c˜ao" → "Introdução".
fn compose_accents(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut pending: Option<char> = None;
    for (i, &c) in chars.iter().enumerate() {
        let letter = |j: usize| chars.get(j).is_some_and(|c| c.is_alphabetic() || *c == 'ı');
        match combining(c) {
            // TeX draws the accent right before its letter…
            Some(m) if letter(i + 1) => pending = Some(m),
            // …or, for some fonts' cedilla, right after it.
            Some(m) if c == '¸' && i > 0 && letter(i - 1) => out.push(m),
            _ => {
                // A dotless i under an accent is just an i.
                out.push(if c == 'ı' && pending.is_some() { 'i' } else { c });
                if let Some(m) = pending.take() {
                    out.push(m);
                }
            }
        }
    }
    unicode_normalization::UnicodeNormalization::nfc(out.as_str()).collect()
}

/// The words on one page (1-based) with their boxes in points, top-left
/// origin, in reading (content) order.
pub fn words(path: &Path, page: usize) -> Result<Vec<Word>, String> {
    let pdf = open(path)?;
    let page = pdf.pages().get(page.wrapping_sub(1)).ok_or(t("no such page"))?;
    let (w, h) = page.render_dimensions();
    let initial = page.initial_transform(true).to_kurbo();
    let cache = hayro::hayro_interpret::InterpreterCache::new();
    let mut ctx = Context::new(initial, Rect::new(0.0, 0.0, w as f64, h as f64), &cache, page.xref(), InterpreterSettings::default());
    let mut device = TextDevice::default();
    device.push_clip_path(&ClipPath { path: page.intersected_crop_box().to_kurbo().to_path(0.1), fill: FillRule::NonZero });
    hayro::hayro_interpret::interpret_page(page, &mut ctx, &mut device);
    Ok(words_from_glyphs(&device.glyphs))
}

#[cfg(test)]
mod tests {
    #[test]
    fn composes_tex_accents() {
        assert_eq!(super::compose_accents("s´erie"), "série");
        assert_eq!(super::compose_accents("c¸"), "ç");
        assert_eq!(super::compose_accents("Introdu¸c˜ao"), "Introdução");
        assert_eq!(super::compose_accents("harmˆonica"), "harmônica");
        assert_eq!(super::compose_accents("n´ıvel"), "nível");
        assert_eq!(super::compose_accents("x^"), "x^");
    }
}

