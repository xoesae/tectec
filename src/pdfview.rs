//! The PDF viewer: continuous, virtualized scrolling, where every page has its
//! space reserved from the start (sizes are read up front), but a page is
//! only rasterized when it gets near the viewport.
//!
//! Sharpness: each page is rasterized (in-process, by hayro) at exactly the pixel size
//! it occupies on screen and drawn aligned to the pixel grid, so one PDF
//! pixel is one screen pixel, so there is no resampling blur. While zooming, the old
//! image is stretched and the exact one is only asked for once the zoom
//! settles.
//!
//! Fluidity: renders go through one priority queue served by a small
//! worker pool; every frame the queue is replaced by what the viewport
//! needs now, nearest pages first, so scrolling or zooming fast never
//! leaves a backlog of pages nobody looks at anymore. Recompiling swaps
//! images in place; the stale page stays until its replacement arrives.
//!
//! Interaction: a click asks for the inverse SyncTeX search (with the word
//! under the pointer, so the editor can land on it); dragging selects text
//! in reading order like a document reader; double click selects a word,
//! triple click a line. A finished selection is reported so the editor can
//! select the same text in the source.

use crate::backend::pdf::{self, PageSize, Word};
use crate::backend::synctex::Position;
use crate::i18n::t;
use crate::tf;
use crate::icons::{self, Icon};
use egui::{Color32, ColorImage, CursorIcon, Pos2, Rect, Sense, TextureHandle, TextureOptions, Vec2};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex};
use std::time::UNIX_EPOCH;

const GAP: f32 = 12.0;
const RENDER_THREADS: usize = 2;
/// Width of the find field and its arrows, once open.
const FIND_WIDTH: f32 = 260.0;
/// How long the zoom must stay put before pages are re-rendered for it.
const ZOOM_SETTLE: f64 = 0.15;
/// Textures further than this many viewport heights away are dropped.
const KEEP_VIEWPORTS: f32 = 4.0;

/// Paper and ink of the dark rendering: the theme's dark background and
/// text colours.
const DARK_PAPER: Color32 = Color32::from_rgb(0x24, 0x22, 0x20);
const DARK_INK: [u8; 3] = [0xE6, 0xE1, 0xDA];

/// Maps white paper to `DARK_PAPER` and black ink to `DARK_INK`, keeping
/// the hue of coloured marks roughly recognizable (links, figures).
fn darken(img: &mut image::RgbaImage) {
    let paper = [DARK_PAPER.r() as f32, DARK_PAPER.g() as f32, DARK_PAPER.b() as f32];
    for p in img.pixels_mut() {
        for c in 0..3 {
            let v = p[c] as f32 / 255.0;
            let ink = DARK_INK[c] as f32;
            p[c] = (paper[c] + (1.0 - v) * (ink - paper[c])).round().clamp(0.0, 255.0) as u8;
        }
    }
}

/// What a page texture was rendered for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct RenderKey {
    page: usize,
    width: u32,
    height: u32,
    version: u64,
    dark: bool,
}

enum Msg {
    Info(u64, Result<Vec<PageSize>, String>),
    Page { key: RenderKey, image: Result<ColorImage, String> },
    Words { page: usize, version: u64, words: Vec<Word> },
}

pub enum PdfEvent {
    /// A click at (x, y) PDF points on a 1-based page, with the word under
    /// the pointer and the few after it (to find the spot in the source).
    SyncClick { page: usize, x: f64, y: f64, words: Option<String> },
    /// Text selected on a page, starting at (x, y).
    Selected { page: usize, x: f64, y: f64, text: String },
    ExportVersion,
}

// ---------- Render queue ----------

#[derive(Default)]
struct QueueState {
    /// Wanted renders, most urgent last (workers pop from the end).
    jobs: Vec<(RenderKey, PathBuf)>,
    running: HashSet<RenderKey>,
    ctx: Option<egui::Context>,
    shutdown: bool,
}

struct Queue {
    state: Mutex<QueueState>,
    wake: Condvar,
}

fn worker(queue: Arc<Queue>, tx: Sender<Msg>) {
    loop {
        let (key, path, ctx) = {
            let mut st = queue.state.lock().unwrap();
            loop {
                if st.shutdown {
                    return;
                }
                if let Some((key, path)) = st.jobs.pop() {
                    st.running.insert(key);
                    break (key, path, st.ctx.clone());
                }
                st = queue.wake.wait(st).unwrap();
            }
        };
        let image = pdf::render_size(&path, key.page, key.width, key.height).map(|mut img| {
            if key.dark {
                darken(&mut img);
            }
            ColorImage::from_rgba_unmultiplied([img.width() as usize, img.height() as usize], img.as_raw())
        });
        queue.state.lock().unwrap().running.remove(&key);
        if tx.send(Msg::Page { key, image }).is_err() {
            return;
        }
        if let Some(ctx) = ctx {
            ctx.request_repaint();
        }
    }
}

// ---------- Selection ----------

#[derive(Clone, Copy, PartialEq, Debug)]
enum SelMode {
    /// From the word at `a` to the word at `b`, in reading order.
    Range,
    /// The word at `a`.
    Word,
    /// The line at `a`.
    Line,
}

struct Selection {
    page: usize,
    a: Pos2,
    b: Pos2,
    mode: SelMode,
}

/// Index of the word under `p`, or the nearest one on the page.
fn word_near(words: &[Word], p: Pos2) -> Option<usize> {
    let dist = |w: &Word| {
        let dx = if p.x < w.x0 { w.x0 - p.x } else if p.x > w.x1 { p.x - w.x1 } else { 0.0 };
        let dy = if p.y < w.y0 { w.y0 - p.y } else if p.y > w.y1 { p.y - w.y1 } else { 0.0 };
        // Being on the same line matters more than being horizontally close.
        dx + dy * 4.0
    };
    (0..words.len()).min_by(|&i, &j| dist(&words[i]).total_cmp(&dist(&words[j])))
}

fn word_under(words: &[Word], p: Pos2) -> Option<usize> {
    words.iter().position(|w| p.x >= w.x0 - 1.0 && p.x <= w.x1 + 1.0 && p.y >= w.y0 - 1.0 && p.y <= w.y1 + 1.0)
}

fn same_line(a: &Word, b: &Word) -> bool {
    let h = (a.y1 - a.y0).max(b.y1 - b.y0).max(1.0);
    (a.y0 - b.y0).abs() < h * 0.5
}

/// Indices of the selected words.
fn selected_indices(words: &[Word], sel: &Selection) -> Vec<usize> {
    let Some(ia) = word_near(words, sel.a) else { return vec![] };
    match sel.mode {
        SelMode::Word => vec![ia],
        SelMode::Line => (0..words.len()).filter(|&i| same_line(&words[i], &words[ia])).collect(),
        SelMode::Range => {
            let Some(ib) = word_near(words, sel.b) else { return vec![ia] };
            (ia.min(ib)..=ia.max(ib)).collect()
        }
    }
}

/// The text of some words: spaces within a line, newlines between lines,
/// and a word hyphenated across lines joined back together.
pub fn words_text(words: &[&Word], newlines: bool) -> String {
    let mut out = String::new();
    for (k, w) in words.iter().enumerate() {
        if k > 0 {
            let prev = words[k - 1];
            if !same_line(prev, w) {
                if prev.text.ends_with('-') && prev.text.len() > 1 {
                    out.pop(); // the hyphen
                } else {
                    out.push(if newlines { '\n' } else { ' ' });
                }
            } else {
                out.push(' ');
            }
        }
        out.push_str(&w.text);
    }
    out
}

/// One rect per line of selected words, in page points.
fn line_rects(words: &[&Word]) -> Vec<Rect> {
    let mut rects: Vec<Rect> = Vec::new();
    let mut last: Option<&Word> = None;
    for w in words {
        let r = Rect::from_min_max(Pos2::new(w.x0, w.y0), Pos2::new(w.x1, w.y1));
        match (last, rects.last_mut()) {
            (Some(prev), Some(cur)) if same_line(prev, w) => *cur = cur.union(r),
            _ => rects.push(r),
        }
        last = Some(w);
    }
    rects
}

// ---------- The view ----------

/// Searching the PDF's text.
#[derive(Default)]
struct Find {
    query: String,
    focus: bool,
    /// Every match: its page and the rects (page points) of its words.
    hits: Vec<(usize, Vec<Rect>)>,
    current: usize,
    /// What the hits were computed from: (query, version, pages with words).
    computed: (String, u64, usize),
}

/// Every match of `query` in a page's words, as the rects of the words it
/// covers. Compared the way the text sync compares PDF text: case, accents
/// (pdfLaTeX's "a"+"˜" too) and ligatures (ﬁ) do not matter. Several words
/// match consecutive words (the first by its end, the last by its start),
/// across line breaks.
pub fn find_in_words(words: &[Word], query: &str) -> Vec<Vec<Rect>> {
    let norm = |s: &str| crate::textmatch::normalize_pdf_text(s).into_iter().collect::<String>();
    let clean = |s: &str| norm(s).trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    let q: Vec<String> = query.split_whitespace().map(norm).collect();
    let rect = |w: &Word| Rect::from_min_max(Pos2::new(w.x0, w.y0), Pos2::new(w.x1, w.y1));
    let mut out = Vec::new();
    let n = q.len();
    if n == 0 {
        return out;
    }
    for i in 0..words.len() {
        let lower = norm(&words[i].text);
        if n == 1 {
            if lower.contains(&q[0]) {
                out.push(vec![rect(&words[i])]);
            }
            continue;
        }
        if i + n > words.len() || !clean(&lower).ends_with(q[0].trim_matches(|c: char| !c.is_alphanumeric())) {
            continue;
        }
        let middle_ok = (1..n - 1).all(|k| clean(&words[i + k].text) == q[k].trim_matches(|c: char| !c.is_alphanumeric()));
        let last_ok = clean(&words[i + n - 1].text).starts_with(q[n - 1].trim_matches(|c: char| !c.is_alphanumeric()));
        if middle_ok && last_ok {
            out.push(words[i..i + n].iter().map(rect).collect());
        }
    }
    out
}

pub struct PdfView {
    id: egui::Id,
    find: Option<Find>,
    path: Option<PathBuf>,
    version: u64,
    pages: Vec<PageSize>,
    textures: HashMap<usize, (TextureHandle, RenderKey)>,
    words: HashMap<usize, (u64, Vec<Word>)>,
    words_pending: HashSet<(usize, u64)>,
    /// Multiplier over "fit width" (or "fit page").
    zoom: f32,
    /// Fit whole pages to the panel instead of its width.
    pub fit_page: bool,
    /// Inverted, warm-tinted pages for dark themes.
    pub dark: bool,
    /// 1-based page to scroll to on the next frame.
    goto_page: Option<usize>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    queue: Arc<Queue>,
    workers_started: bool,
    /// (1-based page, rect in points) to bring into view, smoothly.
    reveal: Option<(usize, Rect)>,
    highlight: Option<(usize, Rect, f64, f64)>,
    selection: Option<Selection>,
    selected_text: String,
    /// Where the last click landed, for a short pulse: (page, point, time).
    pulse: Option<(usize, Pos2, f64)>,
    pub message: String,
    error: Option<String>,
    current_page: usize,
    last_viewport: Rect,
    pending_version: Option<u64>,
    /// Zoom around the pointer: (content offset to apply next frame).
    pending_offset: Option<Vec2>,
    last_offset: Vec2,
    last_scale: f32,
    scale_changed_at: f64,
    /// A zoom from a button or a key, eased: (from, to, start time).
    zoom_anim: Option<(f32, f32, f64)>,
    /// What the toolbar asked for this frame, for the pages to do.
    input: ToolbarInput,
}

#[derive(Default)]
struct ToolbarInput {
    ease_to: Option<f32>,
    /// Next (1) or previous (-1) find result.
    step: isize,
    close: bool,
    event: Option<PdfEvent>,
}

fn version_of(path: &Path) -> u64 {
    std::fs::metadata(path)
        .map(|m| {
            let t = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos() as u64).unwrap_or(0);
            t ^ m.len().rotate_left(32)
        })
        .unwrap_or(0)
}

impl Drop for PdfView {
    fn drop(&mut self) {
        self.queue.state.lock().unwrap().shutdown = true;
        self.queue.wake.notify_all();
    }
}

impl PdfView {
    pub fn new(id: impl std::hash::Hash + std::fmt::Debug, message: &str) -> Self {
        let (tx, rx) = channel();
        Self {
            id: egui::Id::new(id),
            find: None,
            path: None,
            version: 0,
            pages: vec![],
            textures: HashMap::new(),
            words: HashMap::new(),
            words_pending: HashSet::new(),
            zoom: 1.0,
            fit_page: false,
            dark: false,
            goto_page: None,
            tx,
            rx,
            queue: Arc::new(Queue { state: Mutex::new(QueueState::default()), wake: Condvar::new() }),
            workers_started: false,
            reveal: None,
            highlight: None,
            selection: None,
            selected_text: String::new(),
            pulse: None,
            message: message.to_string(),
            error: None,
            current_page: 1,
            last_viewport: Rect::NOTHING,
            pending_version: None,
            pending_offset: None,
            last_offset: Vec2::ZERO,
            last_scale: 0.0,
            scale_changed_at: 0.0,
            zoom_anim: None,
            input: ToolbarInput::default(),
        }
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    pub fn has_document(&self) -> bool {
        !self.pages.is_empty()
    }

    pub fn clear(&mut self, message: &str) {
        self.path = None;
        self.pages.clear();
        self.textures.clear();
        self.words.clear();
        self.selection = None;
        self.selected_text.clear();
        self.highlight = None;
        self.message = message.to_string();
        self.error = None;
        self.queue.state.lock().unwrap().jobs.clear();
    }

    /// Shows (or refreshes) a PDF. Re-reading is skipped when the file did
    /// not change since the last load.
    pub fn load(&mut self, ctx: &egui::Context, path: &Path) {
        let version = version_of(path);
        if self.path.as_deref() == Some(path) && (self.version == version || self.pending_version == Some(version)) {
            return;
        }
        if self.path.as_deref() != Some(path) {
            self.textures.clear();
            self.words.clear();
            self.pages.clear();
            self.selection = None;
            self.selected_text.clear();
        }
        self.path = Some(path.to_path_buf());
        self.pending_version = Some(version);
        let tx = self.tx.clone();
        let p = path.to_path_buf();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Msg::Info(version, pdf::info(&p)));
            ctx.request_repaint();
        });
    }

    /// Scrolls to a forward-search position (only when it is not already in
    /// view) and marks it for a moment.
    /// Opens the search bar, with the cursor in it (Ctrl+F over the preview).
    pub fn open_find(&mut self) {
        self.find.get_or_insert_with(Find::default).focus = true;
    }

    /// Whether the search bar is open.
    pub fn finding(&self) -> bool {
        self.find.is_some()
    }

    /// Recomputes the matches when the query, the document or the words
    /// loaded so far changed, asking for the words of every page.
    fn update_find(&mut self, ctx: &egui::Context) {
        let Some(query) = self.find.as_ref().map(|f| f.query.clone()) else { return };
        if query.trim().is_empty() {
            if let Some(f) = &mut self.find {
                f.hits.clear();
            }
            return;
        }
        for page in 1..=self.pages.len() {
            self.request_words(ctx, page);
        }
        let loaded = (1..=self.pages.len()).filter(|p| self.page_words(*p).is_some()).count();
        let key = (query, self.version, loaded);
        if self.find.as_ref().is_some_and(|f| f.computed == key) {
            return;
        }
        let mut hits = Vec::new();
        for page in 1..=self.pages.len() {
            if let Some(words) = self.page_words(page) {
                hits.extend(find_in_words(words, &key.0).into_iter().map(|r| (page, r)));
            }
        }
        let find = self.find.as_mut().unwrap();
        find.current = find.current.min(hits.len().saturating_sub(1));
        find.hits = hits;
        find.computed = key;
    }

    /// Scrolls to the match `step` away from the current one.
    fn step_find(&mut self, step: isize) {
        let Some(find) = &mut self.find else { return };
        if find.hits.is_empty() {
            return;
        }
        let n = find.hits.len() as isize;
        find.current = (find.current as isize + step).rem_euclid(n) as usize;
        let (page, rects) = &find.hits[find.current];
        let r = rects.iter().skip(1).fold(rects[0], |a, b| a.union(*b));
        self.reveal = Some((*page, r));
    }

    pub fn reveal(&mut self, pos: Position, now: f64) {
        let rect = Rect::from_min_size(
            Pos2::new(pos.x as f32, pos.y as f32),
            Vec2::new((pos.width as f32).max(4.0), (pos.height as f32).max(8.0)),
        );
        self.highlight = Some((pos.page, rect, now, now + 1.8));
        self.reveal = Some((pos.page, rect));
    }

    fn poll(&mut self, ctx: &egui::Context) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Info(version, result) => {
                    if self.pending_version != Some(version) {
                        continue;
                    }
                    self.pending_version = None;
                    match result {
                        Ok(pages) => {
                            self.pages = pages;
                            self.version = version;
                            self.error = None;
                            self.words.retain(|_, (v, _)| *v == version);
                            self.update_selected_text();
                        }
                        Err(e) => self.error = Some(e),
                    }
                }
                Msg::Page { key, image } => match image {
                    Ok(img) => {
                        // A render for an older version still beats nothing,
                        // but never replaces a newer one.
                        let newer = self.textures.get(&key.page).map(|(_, k)| k.version == self.version && key.version != self.version).unwrap_or(false);
                        if !newer && key.page <= self.pages.len().max(key.page) {
                            let name = format!("pdf-{:?}-{}", self.id, key.page);
                            let tex = ctx.load_texture(name, img, TextureOptions::LINEAR);
                            self.textures.insert(key.page, (tex, key));
                        }
                    }
                    Err(e) => self.error = Some(e),
                },
                Msg::Words { page, version, words } => {
                    self.words_pending.remove(&(page, version));
                    if version == self.version {
                        self.words.insert(page, (version, words));
                        self.update_selected_text();
                    }
                }
            }
        }
    }

    fn start_workers(&mut self, ctx: &egui::Context) {
        if self.workers_started {
            return;
        }
        self.workers_started = true;
        self.queue.state.lock().unwrap().ctx = Some(ctx.clone());
        for _ in 0..RENDER_THREADS {
            let (q, tx) = (self.queue.clone(), self.tx.clone());
            std::thread::spawn(move || worker(q, tx));
        }
    }

    /// Replaces the render queue with what is wanted now (most urgent first).
    fn schedule(&self, mut wanted: Vec<(f32, RenderKey)>) {
        let Some(path) = self.path.clone() else { return };
        wanted.sort_by(|a, b| b.0.total_cmp(&a.0)); // least urgent first: workers pop from the end
        let mut st = self.queue.state.lock().unwrap();
        let running = st.running.clone();
        st.jobs = wanted.into_iter().filter(|(_, k)| !running.contains(k)).map(|(_, k)| (k, path.clone())).collect();
        drop(st);
        self.queue.wake.notify_all();
    }

    fn request_words(&mut self, ctx: &egui::Context, page: usize) {
        let version = self.version;
        if self.words.get(&page).map(|(v, _)| *v == version).unwrap_or(false) || self.words_pending.contains(&(page, version)) {
            return;
        }
        let Some(path) = self.path.clone() else { return };
        self.words_pending.insert((page, version));
        let (tx, ctx) = (self.tx.clone(), ctx.clone());
        std::thread::spawn(move || {
            let words = pdf::words(&path, page).unwrap_or_default();
            let _ = tx.send(Msg::Words { page, version, words });
            ctx.request_repaint();
        });
    }

    fn page_words(&self, page: usize) -> Option<&[Word]> {
        self.words.get(&page).filter(|(v, _)| *v == self.version).map(|(_, w)| w.as_slice())
    }

    fn update_selected_text(&mut self) {
        self.selected_text.clear();
        let Some(sel) = &self.selection else { return };
        let Some(words) = self.page_words(sel.page) else { return };
        let idx = selected_indices(words, sel);
        let picked: Vec<&Word> = idx.iter().map(|&i| &words[i]).collect();
        self.selected_text = words_text(&picked, true);
    }

    /// The selection as an event for the editor.
    fn selection_event(&self) -> Option<PdfEvent> {
        let sel = self.selection.as_ref()?;
        let words = self.page_words(sel.page)?;
        let idx = selected_indices(words, sel);
        let first = &words[*idx.first()?];
        let picked: Vec<&Word> = idx.iter().map(|&i| &words[i]).collect();
        Some(PdfEvent::Selected {
            page: sel.page,
            x: ((first.x0 + first.x1) / 2.0) as f64,
            y: ((first.y0 + first.y1) / 2.0) as f64,
            text: words_text(&picked, false),
        })
    }

    /// The word under a point and the next few, for the click event.
    fn words_at(&self, page: usize, p: Pos2) -> Option<String> {
        let words = self.page_words(page)?;
        let i = word_under(words, p)?;
        let picked: Vec<&Word> = words[i..(i + 3).min(words.len())].iter().filter(|w| same_line(w, &words[i])).collect();
        Some(words_text(&picked, false))
    }

    /// `export_to`: the folder versions are saved in, when this view offers
    /// to save one.
    /// Takes in what the workers finished and updates the find results:
    /// once a frame, before the toolbar.
    pub fn prepare(&mut self, ctx: &egui::Context) {
        self.start_workers(ctx);
        self.poll(ctx);
        if !self.pages.is_empty() {
            self.update_find(ctx);
        }
    }

    pub fn has_pages(&self) -> bool {
        !self.pages.is_empty()
    }

    /// The toolbar and the pages: a PDF in a tab of its own.
    pub fn show(&mut self, ui: &mut egui::Ui, export_to: Option<&str>) -> Option<PdfEvent> {
        self.prepare(ui.ctx());
        if self.has_pages() {
            crate::widgets::bar(ui, |ui| {
                self.navigation(ui);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.tools(ui, export_to));
            });
            ui.separator();
        }
        self.pages_view(ui)
    }

    /// Zoom and pages: the left end of a toolbar.
    pub fn navigation(&mut self, ui: &mut egui::Ui) {
        // Steps (buttons, Ctrl+0) ease to their zoom; the wheel and the
        // pinch are continuous already. A step taken mid-way counts from
        // where the previous one is going.
        let zoom_base = self.zoom_anim.map_or(self.zoom, |(_, to, _)| to);
        ui.spacing_mut().item_spacing.x = 2.0;
        if ui.add(icons::icon_button(Icon::ZoomOut)).on_hover_text(t("Zoom out (Ctrl+wheel, Ctrl+−)")).clicked() {
            self.input.ease_to = Some(zoom_base / 1.2);
        }
        let fit_tip = if self.fit_page { t("Fit the whole page again (Ctrl+0)") } else { t("Fit the width again (Ctrl+0)") };
        if ui.add(egui::Button::new(format!("{:.0}%", self.zoom * 100.0))).on_hover_text(fit_tip).clicked() {
            self.input.ease_to = Some(1.0);
        }
        if ui.add(icons::icon_button(Icon::ZoomIn)).on_hover_text(t("Zoom in (Ctrl+wheel, Ctrl+=)")).clicked() {
            self.input.ease_to = Some(zoom_base * 1.2);
        }
        let (fit_icon, fit_tip) = if self.fit_page {
            (Icon::FitWidth, t("Fit the width"))
        } else {
            (Icon::FitPage, t("Fit the whole page"))
        };
        if ui.add(icons::icon_button(fit_icon)).on_hover_text(fit_tip).clicked() {
            self.fit_page = !self.fit_page;
            self.zoom = 1.0;
            self.zoom_anim = None;
            self.goto_page = Some(self.current_page);
        }
        ui.add_space(6.0);
        if ui.add_enabled(self.current_page > 1, icons::icon_button(Icon::ChevronUp)).on_hover_text(t("Previous page (PgUp)")).clicked() {
            self.goto_page = Some(self.current_page - 1);
        }
        ui.weak(format!("{} / {}", self.current_page, self.pages.len()));
        if ui.add_enabled(self.current_page < self.pages.len(), icons::icon_button(Icon::ChevronDown)).on_hover_text(t("Next page (PgDn)")).clicked() {
            self.goto_page = Some(self.current_page + 1);
        }
    }

    /// Find, save a version and copy: the right end of a toolbar, in a
    /// right-to-left layout.
    pub fn tools(&mut self, ui: &mut egui::Ui, export_to: Option<&str>) {
        let ctx = ui.ctx().clone();
        let finding = self.find.is_some();
        let side = crate::widgets::ICON_BUTTON;
        let search = egui::Button::new(Icon::Search.tinted(if finding { crate::theme::accent() } else { ui.visuals().text_color() })).min_size(egui::vec2(side, side));
        if ui.add(search).on_hover_text(t("Find in the PDF (Ctrl+F over the preview)")).clicked() {
            if finding {
                self.input.close = true;
            } else {
                self.open_find();
            }
        }
        // The find field opens out of the button, to its left,
        // pushing the buttons beside it along.
        let open = ctx.animate_bool_with_time(ui.id().with("find-open"), self.find.is_some(), crate::theme::MOTION);
        if open > 0.0 {
            let full = FIND_WIDTH.min(ui.available_width() - 3.0 * side).max(120.0);
            let (slot, _) = ui.allocate_exact_size(egui::vec2(full * open, side), Sense::hover());
            if let Some(find) = &mut self.find {
                // Laid out at full width from the button, shown
                // only as far as it has opened.
                let content = Rect::from_min_max(egui::pos2(slot.right() - full, slot.top()), slot.max);
                let mut row = ui.new_child(egui::UiBuilder::new().max_rect(content).layout(egui::Layout::right_to_left(egui::Align::Center)));
                row.set_clip_rect(slot.intersect(ui.clip_rect()));
                row.multiply_opacity(open);
                row.spacing_mut().item_spacing.x = 2.0;
                if row.add(icons::icon_button(Icon::ArrowDown)).on_hover_text(t("Next (Enter)")).clicked() {
                    self.input.step = 1;
                }
                if row.add(icons::icon_button(Icon::ArrowUp)).on_hover_text(t("Previous (Shift+Enter)")).clicked() {
                    self.input.step = -1;
                }
                row.add_space(4.0);
                // "3 of 12" inside the field's right end.
                let count = (!find.query.trim().is_empty()).then(|| {
                    if find.hits.is_empty() {
                        t("no results").to_string()
                    } else {
                        tf!("{current} of {total}", current = find.current + 1, total = find.hits.len())
                    }
                });
                let margin = egui::Margin { left: 8, right: if count.is_some() { 64 } else { 8 }, top: 4, bottom: 4 };
                let field = row.add(
                    egui::TextEdit::singleline(&mut find.query)
                        .hint_text(t("Find in the PDF"))
                        .margin(margin)
                        .desired_width(row.available_width()),
                );
                if let Some(count) = count {
                    let color = if find.hits.is_empty() { row.visuals().warn_fg_color } else { row.visuals().weak_text_color() };
                    row.painter().text(field.rect.right_center() - egui::vec2(8.0, 0.0), egui::Align2::RIGHT_CENTER, count, egui::FontId::proportional(11.0), color);
                }
                if std::mem::take(&mut find.focus) {
                    field.request_focus();
                }
                if field.lost_focus() && row.input(|i| i.key_pressed(egui::Key::Enter)) {
                    self.input.step = if row.input(|i| i.modifiers.shift) { -1 } else { 1 };
                    field.request_focus();
                }
                if field.has_focus() && row.input(|i| i.key_pressed(egui::Key::Escape)) {
                    self.input.close = true;
                }
            }
            ui.add_space(4.0);
        }
        if let Some(folder) = export_to {
            let tip = tf!("Save a version: keeps a dated copy of the PDF in {folder}/", folder);
            if ui.add_enabled(self.has_document(), icons::icon_button(Icon::Share)).on_hover_text(tip).clicked() {
                self.input.event = Some(PdfEvent::ExportVersion);
            }
        }
        if !self.selected_text.is_empty() && ui.add(icons::button(Icon::Copy, t("Copy"))).on_hover_text("Ctrl+C").clicked() {
            ctx.copy_text(self.selected_text.clone());
        }
    }

    /// The pages, after the toolbar.
    pub fn pages_view(&mut self, ui: &mut egui::Ui) -> Option<PdfEvent> {
        let ctx = ui.ctx().clone();
        let ToolbarInput { mut ease_to, step, close, event } = std::mem::take(&mut self.input);
        if self.pages.is_empty() {
            if let Some(e) = &self.error {
                ui.colored_label(ui.visuals().error_fg_color, e);
            }
            ui.centered_and_justified(|ui| {
                ui.weak(&self.message);
            });
            return event;
        }
        if close {
            self.find = None;
        } else if step != 0 {
            self.step_find(step);
        }
        if let Some(e) = &self.error {
            ui.colored_label(ui.visuals().error_fg_color, e);
        }
        let mut event = event;
        let mut zoom_to: Option<f32> = None;

        // ----- Layout -----
        let ppp = ctx.pixels_per_point();
        let max_w = self.pages.iter().map(|p| p.width).fold(1.0, f32::max);
        let max_h = self.pages.iter().map(|p| p.height).fold(1.0, f32::max);
        let avail = ui.available_width() - 2.0 * GAP - 14.0;
        let hovered = ui.rect_contains_pointer(ui.available_rect_before_wrap());

        // Zoom: Ctrl+wheel/pinch around the pointer, keyboard, buttons.
        if hovered {
            let z = ui.input(|i| i.zoom_delta());
            if (z - 1.0).abs() > 0.001 {
                zoom_to = Some(self.zoom * z);
            }
            ui.input_mut(|i| {
                if i.consume_key(egui::Modifiers::COMMAND, egui::Key::Num0) {
                    ease_to = Some(1.0);
                }
            });
        }
        let clock = ui.input(|i| i.time);
        if zoom_to.is_some() {
            self.zoom_anim = None;
        } else {
            if let Some(to) = ease_to.map(|z| z.clamp(0.2, 6.0)).filter(|z| (z - self.zoom).abs() > f32::EPSILON) {
                self.zoom_anim = Some((self.zoom, to, clock));
            }
            if let Some((from, to, start)) = self.zoom_anim {
                let t = ((clock - start) as f32 / crate::theme::MOTION).clamp(0.0, 1.0);
                zoom_to = Some(from + (to - from) * egui::emath::easing::quadratic_out(t));
                if t >= 1.0 {
                    self.zoom_anim = None;
                } else {
                    ctx.request_repaint();
                }
            }
        }
        let fit_of = |fit_page: bool| {
            if fit_page {
                (avail / max_w).min((ui.available_height() - 2.0 * GAP) / max_h)
            } else {
                avail / max_w
            }
        };
        if let Some(z) = zoom_to {
            let z = z.clamp(0.2, 6.0);
            if (z - self.zoom).abs() > f32::EPSILON {
                // Keep the point under the pointer (or the view's centre) fixed.
                let old_scale = fit_of(self.fit_page).max(0.05) * self.zoom;
                let new_scale = fit_of(self.fit_page).max(0.05) * z;
                let vp = self.last_viewport;
                let anchor = ctx
                    .pointer_hover_pos()
                    .filter(|_| hovered && vp.is_positive())
                    .map(|p| p - ui.available_rect_before_wrap().min + Vec2::new(0.0, 0.0))
                    .unwrap_or(if vp.is_positive() { vp.size() / 2.0 } else { Vec2::ZERO });
                let ratio = new_scale / old_scale;
                self.pending_offset = Some(((self.last_offset + anchor) * ratio - anchor).max(Vec2::ZERO));
                self.zoom = z;
            }
        }
        let scale = fit_of(self.fit_page).max(0.05) * self.zoom;
        let now = ui.input(|i| i.time);
        if (scale - self.last_scale).abs() > 1e-4 {
            self.last_scale = scale;
            self.scale_changed_at = now;
        }
        let settled = now - self.scale_changed_at >= ZOOM_SETTLE;
        if !settled {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(ZOOM_SETTLE));
        }

        let page_tops: Vec<f32> = self
            .pages
            .iter()
            .scan(GAP, |y, p| {
                let top = *y;
                *y += p.height * scale + GAP;
                Some(top)
            })
            .collect();
        let total_h = page_tops.last().map(|t| t + self.pages.last().unwrap().height * scale + GAP).unwrap_or(0.0);
        let content_w = (max_w * scale + 2.0 * GAP).max(ui.available_width() - 14.0);

        // Page keys while the pointer is over the document.
        if hovered {
            let (up, down, home, end) = ui.input(|i| {
                (i.key_pressed(egui::Key::PageUp), i.key_pressed(egui::Key::PageDown), i.key_pressed(egui::Key::Home), i.key_pressed(egui::Key::End))
            });
            if up && self.current_page > 1 {
                self.goto_page = Some(self.current_page - 1);
            } else if down && self.current_page < self.pages.len() {
                self.goto_page = Some(self.current_page + 1);
            } else if home {
                self.goto_page = Some(1);
            } else if end {
                self.goto_page = Some(self.pages.len());
            }
        }

        let mut area = egui::ScrollArea::both().id_salt(self.id.with("scroll")).auto_shrink(false);
        if let Some(off) = self.pending_offset.take() {
            area = area.scroll_offset(off);
        }
        if let Some(page) = self.goto_page.take() {
            if let Some(top) = page_tops.get(page.wrapping_sub(1)) {
                area = area.vertical_scroll_offset((top - GAP).max(0.0));
                self.current_page = page;
            }
        }

        let mut wanted: Vec<(f32, RenderKey)> = Vec::new();
        let mut words_wanted: Vec<usize> = Vec::new();
        let mut hover_text = false;
        let max_side = ctx.input(|i| i.max_texture_side).min(8192) as f32;
        let reveal = self.reveal.take();

        // egui's edge fade paints the panel colour over the pages.
        ui.spacing_mut().scroll.fade.strength = 0.0;
        let output = area.show_viewport(ui, |ui, viewport| {
            self.last_viewport = viewport;
            let (content, _) = ui.allocate_exact_size(Vec2::new(content_w, total_h), Sense::hover());
            let origin = content.min;
            let center_y = viewport.center().y;
            let snap = |v: f32| (v * ppp).round() / ppp;
            let sel_fill = ui.visuals().selection.bg_fill.gamma_multiply(if self.dark { 0.9 } else { 0.55 });

            for (i, size) in self.pages.clone().iter().enumerate() {
                let page = i + 1;
                let top = page_tops[i];
                // Exact device pixels for this page at this zoom (capped).
                let fit = (max_side / (size.width * scale * ppp)).min(max_side / (size.height * scale * ppp)).min(1.0);
                let px_w = (size.width * scale * ppp * fit).round().max(1.0);
                let px_h = (size.height * scale * ppp * fit).round().max(1.0);
                let (w, h) = (size.width * scale, size.height * scale);
                if top <= center_y && center_y <= top + h + GAP {
                    self.current_page = page;
                }
                // Smooth reveal of a forward-search target.
                if let Some((rp, rr)) = reveal {
                    if rp == page {
                        let y = top + rr.min.y * scale;
                        let visible = viewport.is_positive() && y >= viewport.top() + 20.0 && y + rr.height() * scale <= viewport.bottom() - 30.0;
                        if !visible {
                            let target = Rect::from_min_size(origin + Vec2::new(0.0, y), Vec2::new(1.0, rr.height() * scale));
                            ui.scroll_to_rect_animation(target, Some(egui::Align::Center), egui::style::ScrollAnimation::duration(0.25));
                        }
                    }
                }
                let distance = if top + h < viewport.top() {
                    viewport.top() - (top + h)
                } else if top > viewport.bottom() {
                    top - viewport.bottom()
                } else {
                    0.0
                };
                if distance > viewport.height() * KEEP_VIEWPORTS {
                    self.textures.remove(&page);
                    continue;
                }
                // Render what is on screen or about to be.
                let key = RenderKey { page, width: px_w as u32, height: px_h as u32, version: self.version, dark: self.dark };
                let current = self.textures.get(&page).map(|(_, k)| *k);
                if distance <= viewport.height() && current != Some(key) && (settled || current.is_none()) {
                    wanted.push((distance, key));
                }
                if distance > viewport.height() {
                    continue;
                }
                if distance == 0.0 {
                    words_wanted.push(page);
                }

                let x = snap((content_w - w) / 2.0);
                let min = Pos2::new(snap(origin.x + x), snap(origin.y + top));
                let rect = Rect::from_min_size(min, Vec2::new(w, h));
                let painter = ui.painter_at(rect.expand(6.0));
                painter.rect_filled(rect.translate(Vec2::new(0.0, 2.0)).expand(1.5), 2.0, Color32::from_black_alpha(45));
                painter.rect_filled(rect, 0.0, if self.dark { DARK_PAPER } else { Color32::WHITE });
                if let Some((tex, k)) = self.textures.get(&page) {
                    // At the exact size, draw pixel for pixel; otherwise stretch.
                    let draw = if k.width == key.width && k.height == key.height {
                        Rect::from_min_size(min, Vec2::new(k.width as f32, k.height as f32) / (ppp * fit))
                    } else {
                        rect
                    };
                    painter.image(tex.id(), draw, Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)), Color32::WHITE);
                }
                let to_screen = |p: Pos2| rect.min + p.to_vec2() * scale;
                let to_pts = |p: Pos2| ((p - rect.min) / scale).to_pos2();

                // Selection.
                if let Some(sel) = self.selection.as_ref().filter(|s| s.page == page) {
                    if let Some(words) = self.page_words(page) {
                        let idx = selected_indices(words, sel);
                        let picked: Vec<&Word> = idx.iter().map(|&i| &words[i]).collect();
                        for r in line_rects(&picked) {
                            let r = Rect::from_min_max(to_screen(r.min), to_screen(r.max)).expand2(Vec2::new(1.0, 1.5));
                            painter.rect_filled(r, 2.0, sel_fill);
                        }
                    }
                }
                // Matches of the PDF search; the current one outlined.
                if let Some(find) = &self.find {
                    for (i, (hp, rects)) in find.hits.iter().enumerate() {
                        if *hp != page {
                            continue;
                        }
                        let current = i == find.current;
                        for r in rects {
                            let r = Rect::from_min_max(to_screen(r.min), to_screen(r.max)).expand(1.5);
                            painter.rect_filled(r, 2.0, crate::theme::accent().gamma_multiply(if current { 0.45 } else { 0.22 }));
                            if current {
                                painter.rect_stroke(r, 2.0, egui::Stroke::new(1.5, crate::theme::accent()), egui::StrokeKind::Outside);
                            }
                        }
                    }
                }
                // Forward-search highlight, fading out.
                if let Some((hp, hr, from, until)) = self.highlight {
                    if hp == page && now < until {
                        let t = ((now - from) / (until - from)) as f32;
                        let alpha = if t < 0.1 { t / 0.1 } else { 1.0 - ((t - 0.6) / 0.4).clamp(0.0, 1.0) };
                        let r = Rect::from_min_max(to_screen(hr.min), to_screen(hr.max)).expand(3.0);
                        painter.rect_filled(r, 3.0, crate::theme::accent().gamma_multiply(0.28 * alpha));
                        painter.rect_stroke(r, 3.0, egui::Stroke::new(1.5, crate::theme::accent().gamma_multiply(0.8 * alpha)), egui::StrokeKind::Outside);
                        ctx.request_repaint();
                    }
                }
                // Click pulse.
                if let Some((pp, pt, at)) = self.pulse {
                    let t = (now - at) as f32 / 0.5;
                    if pp == page && t < 1.0 {
                        let c = to_screen(pt);
                        painter.circle_stroke(c, 4.0 + 14.0 * t, egui::Stroke::new(2.0, crate::theme::accent().gamma_multiply(1.0 - t)));
                        ctx.request_repaint();
                    }
                }

                // ----- Interaction -----
                let response = ui.interact(rect, self.id.with(("page", page)), Sense::click_and_drag());
                if let Some(p) = response.hover_pos() {
                    let pt = to_pts(p);
                    if self.page_words(page).map(|w| word_under(w, pt).is_some()).unwrap_or(false) {
                        hover_text = true;
                    }
                }
                if response.triple_clicked() || response.double_clicked() {
                    if let Some(p) = response.interact_pointer_pos() {
                        let mode = if response.triple_clicked() { SelMode::Line } else { SelMode::Word };
                        let pt = to_pts(p);
                        self.selection = Some(Selection { page, a: pt, b: pt, mode });
                        self.update_selected_text();
                        if let Some(e) = self.selection_event() {
                            event = Some(e);
                        }
                    }
                } else if response.clicked() {
                    if let Some(p) = response.interact_pointer_pos() {
                        let pt = to_pts(p);
                        self.selection = None;
                        self.selected_text.clear();
                        self.pulse = Some((page, pt, now));
                        event = Some(PdfEvent::SyncClick { page, x: pt.x as f64, y: pt.y as f64, words: self.words_at(page, pt) });
                    }
                }
                if response.drag_started() {
                    if let Some(p) = response.interact_pointer_pos() {
                        let pt = to_pts(p);
                        self.selection = Some(Selection { page, a: pt, b: pt, mode: SelMode::Range });
                        self.request_words(&ctx, page);
                    }
                }
                if response.dragged() {
                    if let (Some(p), Some(sel)) = (response.interact_pointer_pos(), self.selection.as_mut()) {
                        if sel.page == page {
                            sel.b = to_pts(p);
                        }
                    }
                    self.update_selected_text();
                }
                if response.drag_stopped() {
                    self.update_selected_text();
                    if let Some(e) = self.selection_event().filter(|_| !self.selected_text.is_empty()) {
                        event = Some(e);
                    }
                }
                response.context_menu(|ui| {
                    if ui.add_enabled(!self.selected_text.is_empty(), icons::button(Icon::Copy, t("Copy"))).clicked() {
                        ctx.copy_text(self.selected_text.clone());
                        ui.close();
                    }
                    if ui.add_enabled(!self.selected_text.is_empty(), icons::button(Icon::Code, t("Select in the source"))).clicked() {
                        if let Some(e) = self.selection_event() {
                            event = Some(e);
                        }
                        ui.close();
                    }
                    if ui.button(t("Copy the page's text")).clicked() {
                        if let Some(words) = self.page_words(page) {
                            let all: Vec<&Word> = words.iter().collect();
                            ctx.copy_text(words_text(&all, true));
                        }
                        ui.close();
                    }
                });
            }
        });
        self.last_offset = output.state.offset;
        crate::app::navigate::claim_sideways(&ctx, output.inner_rect, output.content_size.x);

        self.schedule(wanted);
        for page in words_wanted {
            self.request_words(&ctx, page);
        }
        if hover_text {
            ctx.set_cursor_icon(CursorIcon::Text);
        }
        // Ctrl+C with a selection in this viewer.
        if hovered && !self.selected_text.is_empty() && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Copy))) {
            ctx.copy_text(self.selected_text.clone());
        }
        event
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn finds_words_and_phrases() {
        let w = |t: &str, x: f32, y: f32| Word { text: t.into(), x0: x, y0: y, x1: x + 30.0, y1: y + 10.0 };
        let words = [w("A", 0.0, 0.0), w("difusão", 40.0, 0.0), w("de", 80.0, 0.0), w("informação,", 0.0, 20.0), w("Difusão.", 40.0, 20.0)];
        assert_eq!(find_in_words(&words, "difusão").len(), 2, "case-insensitive, inside punctuation");
        let phrase = find_in_words(&words, "difusão de informação");
        assert_eq!(phrase.len(), 1);
        assert_eq!(phrase[0].len(), 3, "one rect per word, across the line break");
        assert!(find_in_words(&words, "   ").is_empty());
        assert!(find_in_words(&words, "rede").is_empty());
        // pdfLaTeX's accents and ligatures, as the PDF gives them back.
        let typeset = [w("difusa˜o", 0.0, 0.0), w("eﬁciente", 40.0, 0.0)];
        assert_eq!(find_in_words(&typeset, "difusão").len(), 1);
        assert_eq!(find_in_words(&typeset, "Eficiente").len(), 1);
    }

    use super::*;

    fn w(text: &str, x0: f32, y0: f32) -> Word {
        Word { text: text.into(), x0, y0, x1: x0 + 8.0 * text.len() as f32, y1: y0 + 10.0 }
    }

    #[test]
    fn text_joins_lines_and_hyphens() {
        let words = [w("uma", 0.0, 0.0), w("pala-", 40.0, 0.0), w("vra", 0.0, 14.0), w("aqui", 40.0, 14.0)];
        let refs: Vec<&Word> = words.iter().collect();
        assert_eq!(words_text(&refs, true), "uma palavra aqui");
        let words = [w("fim", 0.0, 0.0), w("novo", 0.0, 14.0)];
        let refs: Vec<&Word> = words.iter().collect();
        assert_eq!(words_text(&refs, true), "fim\nnovo");
        assert_eq!(words_text(&refs, false), "fim novo");
    }

    #[test]
    fn selection_in_reading_order() {
        let words = [w("a", 0.0, 0.0), w("b", 20.0, 0.0), w("c", 0.0, 14.0), w("d", 20.0, 14.0)];
        // Dragging from "b" back to "a" on the next line down still selects in order.
        let sel = Selection { page: 1, a: Pos2::new(22.0, 5.0), b: Pos2::new(1.0, 19.0), mode: SelMode::Range };
        assert_eq!(selected_indices(&words, &sel), [1, 2]);
        let sel = Selection { page: 1, a: Pos2::new(2.0, 16.0), b: Pos2::ZERO, mode: SelMode::Line };
        assert_eq!(selected_indices(&words, &sel), [2, 3]);
        let rects = line_rects(&[&words[0], &words[1], &words[2]]);
        assert_eq!(rects.len(), 2);
    }
}

