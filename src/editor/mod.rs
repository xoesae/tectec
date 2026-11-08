//! The text editor: an `egui::TextEdit` with a LaTeX layouter, a line-number
//! gutter, the current-line and SyncTeX highlights, the autocomplete popup,
//! the find/replace bar and the spelling suggestions menu.

pub mod completion;
mod hang;
pub mod highlight;
pub mod latex_db;
pub mod lint;
pub mod refs;
pub mod smart;

use crate::i18n::t;
use crate::icons::{self, Icon};
use crate::mathpreview::{self, MathPreview};
use crate::tf;
use crate::spell::SpellCache;
use completion::{Apply, Completions, Item, Kind as ItemKind};
use egui::text::{CCursor, CCursorRange};
use egui::{Align, Color32, FontId, Key, Modifiers, Rect, Shape, Vec2};
use highlight::{Lang, Palette};
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Payload dragged from the images panel onto the editor.
#[derive(Clone, Debug)]
pub struct ImageDrag(pub String);

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Stats {
    pub chars: usize,
    pub lines: usize,
    pub words: usize,
    pub line: usize,
    pub col: usize,
    pub selected: usize,
}

pub struct Popup {
    /// Char index where the replaced text starts.
    from: usize,
    items: Vec<Item>,
    selected: usize,
}

#[derive(Default)]
pub struct FindBar {
    pub open: bool,
    pub query: String,
    pub replace: String,
    pub regex: bool,
    pub case_sensitive: bool,
    focus_query: bool,
    pub error: Option<String>,
    /// The replace row is shown.
    pub replace_open: bool,
    /// The match being looked at (char range). Kept here rather than as the
    /// text selection: egui drops a TextEdit's selection while the query
    /// field has the focus.
    pub current: Option<(usize, usize)>,
}

#[derive(Clone)]
struct SpellMenu {
    range: (usize, usize),
    word: String,
    suggestions: Vec<String>,
}

/// How long the pointer rests on a misspelled word before its suggestions
/// show, in seconds.
const SPELL_HOVER_DELAY: f64 = 0.5;

pub struct TabEditor {
    pub id: egui::Id,
    /// Selection (char indices) to apply before the next frame.
    pub pending_selection: Option<(usize, usize)>,
    pub scroll_to_cursor: bool,
    /// Keep the cursor in view after an edit made outside the TextEdit
    /// (scrolls only as much as needed).
    scroll_to_cursor_soft: bool,
    /// Bring this char into view (centred), whatever the focus.
    scroll_to_char: Option<usize>,
    /// 1-based line to flash (after an inverse search), until this time.
    pub flash_line: Option<(usize, f64)>,
    pub find: FindBar,
    pub request_focus: bool,
    popup: Option<Popup>,
    /// The last layout: its key, the job (kept to keep egui's cache warm)
    /// and the galley.
    /// Also the jobs of the lines laid out again for their hanging indent.
    cache: Option<(u64, egui::text::LayoutJob, Vec<egui::text::LayoutJob>, Arc<egui::Galley>)>,
    words: usize,
    words_hash: u64,
    last_cursor: Option<CCursorRange>,
    cursor_line: usize,
    spell_menu: Option<SpellMenu>,
    /// The suggestions card of the misspelled word the pointer rests on,
    /// with the word's place on screen and the card's (from its last frame).
    spell_hover: Option<(SpellMenu, egui::Rect, Option<egui::Rect>)>,
    /// The writing reviewer's findings for the text as last shown, and its hash.
    issues: Arc<Vec<lint::Issue>>,
    issues_hash: u64,
    /// The hash of the text alone those findings are about.
    issues_text: u64,
    /// The finding right-clicked, for the context menu.
    lint_menu: Option<lint::Issue>,
    pub stats: Stats,
    /// The bracket pair at the cursor (char indices), recomputed when the
    /// cursor or the text moves.
    brackets: Option<(usize, usize)>,
    brackets_stale: bool,
    /// The editor's height last frame: when a panel opens below it, the
    /// cursor is kept in view.
    last_height: f32,
    /// Whether the TextEdit's undoer got `UNDO` yet.
    undo_configured: bool,
    /// An edit was made outside the TextEdit: its result becomes an undo
    /// step too, next frame, so typing after it is undone separately.
    undo_after_edit: bool,
}

/// Undo steps. egui makes one once the text has been still for
/// `stable_time`, or every `auto_save_interval` of nonstop typing, 30 s by
/// default, a whole paragraph per Ctrl+Z. Here a step is a word or a short
/// pause, and each edit made outside the TextEdit gets its own
/// (`undo_point`).
const UNDO: egui::util::undoer::Settings =
    egui::util::undoer::Settings { max_undos: 100, stable_time: 0.5, auto_save_interval: 2.0 };

impl TabEditor {
    pub fn new(id: egui::Id) -> Self {
        Self {
            id,
            pending_selection: None,
            scroll_to_cursor: false,
            scroll_to_cursor_soft: false,
            scroll_to_char: None,
            flash_line: None,
            find: FindBar::default(),
            request_focus: true,
            popup: None,
            cache: None,
            words: 0,
            words_hash: 1,
            last_cursor: None,
            cursor_line: 1,
            spell_menu: None,
            spell_hover: None,
            issues: Arc::default(),
            issues_hash: 0,
            issues_text: 0,
            lint_menu: None,
            stats: Stats::default(),
            brackets: None,
            brackets_stale: true,
            last_height: 0.0,
            undo_configured: false,
            undo_after_edit: false,
        }
    }

    /// The writing reviewer's findings, in text order.
    pub fn issues(&self) -> &[lint::Issue] {
        &self.issues
    }

    /// Selects the next (or previous) finding after the cursor, wrapping
    /// around. False when there is none.
    pub fn go_to_issue(&mut self, ctx: &egui::Context, text: &str, forward: bool) -> bool {
        let (a, b) = self.selection(ctx).unwrap_or((0, 0));
        let (a, b) = (char_to_byte(text, a), char_to_byte(text, b));
        let pick = if forward {
            self.issues.iter().find(|i| i.range.start >= b && i.range.end != b).or(self.issues.first())
        } else {
            self.issues.iter().rev().find(|i| i.range.end <= a && i.range.start != a).or(self.issues.last())
        };
        let Some(issue) = pick else { return false };
        self.pending_selection = Some((byte_to_char(text, issue.range.start), byte_to_char(text, issue.range.end)));
        self.scroll_to_cursor = true;
        self.request_focus = true;
        true
    }

    /// Makes the text as it is now an undo step of its own: called right
    /// before an edit the TextEdit does not see (smart keys, completion,
    /// replace, toolbar), so Ctrl+Z undoes that edit alone.
    pub fn undo_point(&mut self, ctx: &egui::Context, text: &str) {
        undo_point(ctx, self.id, text);
        self.undo_after_edit = true;
    }

    /// Current selection as a sorted char range.
    pub fn selection(&self, ctx: &egui::Context) -> Option<(usize, usize)> {
        let state = egui::TextEdit::load_state(ctx, self.id)?;
        let r = state.cursor.char_range()?;
        let range = r.as_sorted_char_range();
        Some((range.start.0, range.end.0))
    }

    pub fn cursor_line(&self) -> usize {
        self.cursor_line
    }

    /// Moves the cursor to the start of a 1-based line and centers it.
    pub fn go_to_line(&mut self, text: &str, line: usize, flash: Option<f64>) {
        let at = line_start_char(text, line);
        self.pending_selection = Some((at, at));
        self.scroll_to_cursor = true;
        self.request_focus = true;
        if let Some(until) = flash {
            self.flash_line = Some((line.max(1), until));
        }
    }

    #[cfg_attr(not(debug_assertions), allow(dead_code))]
    pub fn popup_open(&self) -> bool {
        self.popup.is_some()
    }

    pub fn find_focus(&mut self) {
        self.find.focus_query = true;
    }

    pub fn invalidate(&mut self) {
        self.cache = None;
        self.words_hash = 1;
    }
}

// ---------- Text helpers (char ↔ byte) ----------

pub fn char_to_byte(s: &str, ci: usize) -> usize {
    s.char_indices().nth(ci).map(|(b, _)| b).unwrap_or(s.len())
}

pub fn byte_to_char(s: &str, bi: usize) -> usize {
    s[..bi.min(s.len())].chars().count()
}

/// Char index of the start of a 1-based line (clamped).
pub fn line_start_char(text: &str, line: usize) -> usize {
    if line <= 1 {
        return 0;
    }
    let mut seen = 1;
    for (ci, c) in text.chars().enumerate() {
        if c == '\n' {
            seen += 1;
            if seen == line {
                return ci + 1;
            }
        }
    }
    text.chars().count()
}

fn count_words(text: &str) -> usize {
    text.split_whitespace().count()
}

/// Replaces the chars `[from, to)` and returns the char index right after
/// the inserted text.
pub fn replace_chars(text: &mut String, from: usize, to: usize, insert: &str) -> usize {
    let (bf, bt) = (char_to_byte(text, from), char_to_byte(text, to.max(from)));
    text.replace_range(bf..bt, insert);
    from + insert.chars().count()
}

/// Inserts a template in place of the selection: `$SEL` becomes the selected
/// text and `$|` marks where the cursor lands.
pub fn insert_block(text: &mut String, sel: (usize, usize), template: &str) -> usize {
    let selected: String = text.chars().skip(sel.0).take(sel.1 - sel.0).collect();
    let body = template.replace("$SEL", &selected);
    let cursor = body.find("$|").map(|b| body[..b].chars().count());
    let body = body.replacen("$|", "", 1);
    let end = replace_chars(text, sel.0, sel.1, &body);
    cursor.map(|c| sel.0 + c).unwrap_or(end)
}

/// Wraps the selection; with none, leaves the cursor in between.
pub fn surround(text: &mut String, sel: (usize, usize), before: &str, after: &str) -> (usize, usize) {
    let selected: String = text.chars().skip(sel.0).take(sel.1 - sel.0).collect();
    replace_chars(text, sel.0, sel.1, &format!("{before}{selected}{after}"));
    let start = sel.0 + before.chars().count();
    (start, start + selected.chars().count())
}

/// A compile diagnostic on one line of the file being edited.
pub struct Diagnostic {
    /// 1-based.
    pub line: usize,
    pub warning: bool,
    /// Short, in the interface's language when there is a summary for it.
    pub message: String,
    /// What to do about it.
    pub hint: Option<String>,
}

pub struct EditorEnv<'a> {
    pub completions: &'a Completions,
    pub spell: Option<&'a mut SpellCache>,
    /// Hover preview of `$...$`/`\[...\]`; `None` where it does not apply.
    pub math: Option<&'a mut MathPreview>,
    pub dark: bool,
    pub typewriter: bool,
    /// The writing reviewer is on.
    pub review: bool,
    /// Wrap at a comfortable line length instead of the window's edge.
    pub measure: bool,
    /// What the reviewer knows about the rest of the document.
    pub review_context: lint::Context,
    pub lang: Lang,
    pub font_size: f32,
    /// The interface's proportional face instead of the monospaced one.
    pub proportional: bool,
    /// Diagnostics of the last compilation for this file.
    pub diagnostics: &'a [Diagnostic],
}

#[derive(Default)]
pub struct EditorResponse {
    pub changed: bool,
    /// The cursor landed on a different line (1-based) this frame.
    pub cursor_line_changed: Option<usize>,
    /// An image from the library was dropped: (relative path, char index).
    pub dropped_image: Option<(String, usize)>,
    /// "Colar imagem" chosen from the context menu, at this char index.
    pub paste_image_at: Option<usize>,
    /// Ctrl+click or F12 on something with a definition.
    pub goto: Option<refs::RefTarget>,
    /// What the pointer rests on, and the rect of the char under it.
    pub hover: Option<(refs::RefTarget, Rect)>,
}

/// Where lines wrap when `EditorEnv::measure` is on, in characters.
const MEASURE_CHARS: f32 = 100.0;

pub(crate) fn text_hash(text: &str, extra: &[u64]) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    extra.hash(&mut h);
    h.finish()
}

pub fn show(ui: &mut egui::Ui, text: &mut String, st: &mut TabEditor, mut env: EditorEnv<'_>) -> EditorResponse {
    let mut resp = EditorResponse::default();
    let ctx = ui.ctx().clone();
    let font = if env.proportional { FontId::proportional(env.font_size) } else { FontId::monospace(env.font_size) };

    if st.find.open {
        find_bar(ui, text, st);
    }

    // ----- Keys the popup takes before the TextEdit sees them -----
    let mut accept: Option<usize> = None;
    let focused = ctx.memory(|m| m.has_focus(st.id));
    if focused {
        if let Some(p) = &mut st.popup {
            let n = p.items.len();
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::ArrowDown) {
                    p.selected = (p.selected + 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::ArrowUp) {
                    p.selected = (p.selected + n - 1) % n;
                }
                if i.consume_key(Modifiers::NONE, Key::Enter) || i.consume_key(Modifiers::NONE, Key::Tab) {
                    accept = Some(p.selected);
                }
            });
            if ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
                st.popup = None;
            }
        }
    }
    let explicit = focused && ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::Space));
    if focused && ui.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F12)) {
        if let Some((a, _)) = st.selection(&ctx) {
            resp.goto = refs::target_at(text, a);
        }
    }

    if !st.undo_configured {
        st.undo_configured = true;
        let mut state = egui::TextEdit::load_state(&ctx, st.id).unwrap_or_default();
        state.set_undoer(egui::util::undoer::Undoer::with_settings(UNDO));
        state.store(&ctx, st.id);
    }
    // A step ends with each word, and before a paste or a cut.
    if focused {
        let (space, clipboard) = ctx.input(|i| {
            let space = i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if t.starts_with(char::is_whitespace)));
            let clipboard = i.events.iter().any(|e| matches!(e, egui::Event::Paste(_) | egui::Event::Cut));
            (space, clipboard)
        });
        // A space right after another does not end a word.
        let after_word = || {
            st_selection(&ctx, st.id)
                .and_then(|(a, _)| a.checked_sub(1))
                .and_then(|i| text.chars().nth(i))
                .is_some_and(|c| !c.is_whitespace())
        };
        if clipboard || (space && after_word()) {
            undo_point(&ctx, st.id, text);
        }
    }

    // ----- Selection requested from outside -----
    if let Some((a, b)) = st.pending_selection.take() {
        let mut state = egui::TextEdit::load_state(&ctx, st.id).unwrap_or_default();
        state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(a), CCursor::new(b))));
        state.store(&ctx, st.id);
    }
    if std::mem::take(&mut st.undo_after_edit) {
        undo_point(&ctx, st.id, text);
    }
    // A click elsewhere this frame (the one that asked for focus, say a
    // click on the PDF) would make the TextEdit give focus right back up:
    // ask again next frame.
    if st.request_focus && !ctx.input(|i| i.pointer.any_click()) {
        st.request_focus = false;
        ctx.memory_mut(|m| m.request_focus(st.id));
    } else if st.request_focus {
        ctx.request_repaint();
    }

    let height = ui.available_height();
    if st.last_height > 0.0 && (height - st.last_height).abs() > 1.0 && focused {
        st.scroll_to_cursor_soft = true;
    }
    st.last_height = height;

    // ----- LaTeX-aware editing keys, also ahead of the TextEdit -----
    if focused && smart_keys(ui, text, st, env.lang) {
        resp.changed = true;
    }

    // ----- The writing reviewer -----
    if env.review && env.lang == Lang::Tex {
        let plain = text_hash(text, &[]);
        let context_hash = {
            let mut h = std::collections::hash_map::DefaultHasher::new();
            env.review_context.hash(&mut h);
            h.finish()
        };
        let hash = text_hash(text, &[context_hash]);
        if hash != st.issues_hash {
            st.issues = Arc::new(lint::check_document(text, &env.review_context));
            st.issues_hash = hash;
            st.issues_text = plain;
        }
    } else if st.issues_hash != 0 {
        st.issues = Arc::default();
        st.issues_hash = 0;
    }
    // Ctrl+.: fixes what the cursor is on.
    if focused && ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::Period)) {
        let at = st.selection(&ctx).map(|(a, _)| char_to_byte(text, a)).unwrap_or(0);
        let issue = st.issues.iter().find(|i| i.fix.is_some() && i.range.start <= at && at <= i.range.end).cloned();
        if let Some(issue) = issue {
            resp.changed |= apply_fix(&ctx, text, st, &issue);
        }
    }

    let palette = Palette::new(env.dark);
    let spell_version = env.spell.as_ref().map(|s| s.version).unwrap_or(0);
    let lang = env.lang;
    let digits = text.lines().count().max(1).to_string().len().max(3) as f32;
    let char_w = ctx.fonts_mut(|f| f.glyph_width(&font, '0'));
    let gutter = (digits * char_w + 18.0).min(120.0);

    let mut out_state = None;
    let scroll = egui::ScrollArea::vertical().id_salt(st.id.with("scroll")).auto_shrink(false).show(ui, |ui| {
        let bg_idx = ui.painter().add(Shape::Noop);
        let cache = &mut st.cache;
        let spell = &mut env.spell;
        let marks: Vec<(std::ops::Range<usize>, bool)> = st.issues.iter().map(|i| (i.range.clone(), i.minor())).collect();
        let (issues_hash, issues_text) = (st.issues_hash, st.issues_text);
        // Prose is easier to follow in lines of a readable length.
        let max_wrap = if env.measure { char_w * MEASURE_CHARS } else { f32::INFINITY };
        let mut layouter = |ui: &egui::Ui, buf: &dyn egui::TextBuffer, wrap_width: f32| {
            let wrap_width = wrap_width.min(max_wrap);
            let key = text_hash(buf.as_str(), &[wrap_width.to_bits() as u64, env.dark as u64, spell_version, env.font_size.to_bits() as u64, env.proportional as u64, issues_hash]);
            if let Some((k, job, hung, galley)) = cache.as_ref() {
                if *k == key {
                    // Hand egui the same jobs again: they are found in egui's
                    // cache, which only keeps what was used this frame;
                    // otherwise the first keystroke after a pause lays the
                    // whole document out from scratch (~100 ms on a thesis).
                    ui.ctx().fonts_mut(|f| {
                        f.layout_job(job.clone());
                        for j in hung {
                            f.layout_job(j.clone());
                        }
                    });
                    return galley.clone();
                }
            }
            // Findings of an older text would underline the wrong bytes.
            let marks: &[_] = if text_hash(buf.as_str(), &[]) == issues_text { &marks } else { &[] };
            let job = highlight::layout_job(buf.as_str(), lang, font.clone(), &palette, spell.as_deref_mut(), wrap_width, marks);
            let galley = ui.ctx().fonts_mut(|f| f.layout_job(job.clone()));
            let (galley, hung) = hang::apply(ui.ctx(), galley, &job, lang);
            *cache = Some((key, job, hung, galley.clone()));
            galley
        };
        let output = egui::TextEdit::multiline(text)
            .id(st.id)
            .font(font.clone())
            .lock_focus(true)
            .desired_width(f32::INFINITY)
            // The margin has to live on the frame: egui ignores `.margin()`
            // once a frame is given.
            .frame(egui::Frame::NONE.inner_margin(egui::Margin { left: gutter as i8, right: 8, top: 6, bottom: 6 }))
            .margin(egui::Margin { left: gutter as i8, right: 8, top: 6, bottom: 6 })
            .layouter(&mut layouter)
            .show(ui);

        let galley = output.galley.clone();
        let origin = output.galley_pos;
        let response = output.response.response.clone();
        let full = ui.min_rect();

        // Selecting past the top or bottom edge scrolls on, faster the
        // further out; the selection follows, as egui extends it every
        // frame of the drag.
        if response.dragged() {
            if let Some(p) = ctx.pointer_latest_pos() {
                let view = ui.clip_rect();
                let out = if p.y < view.top() { p.y - view.top() } else if p.y > view.bottom() { p.y - view.bottom() } else { 0.0 };
                if out != 0.0 {
                    let dt = ui.input(|i| i.unstable_dt).min(0.05);
                    let speed = (200.0 + out.abs() * 12.0).min(3000.0) * dt;
                    ui.scroll_with_delta_animation(egui::vec2(0.0, -out.signum() * speed), egui::style::ScrollAnimation::none());
                    ctx.request_repaint();
                }
            }
        }

        // Line numbers, current line, flash.
        let cursor_idx = output.cursor_range.map(|r| r.primary.index.0);
        let cursor_row = cursor_idx.map(|ci| galley.layout_from_cursor(CCursor::new(ci)).row);
        let visible = ui.clip_rect();
        let mut line = 1usize;
        let mut new_line = true;
        let mut cursor_line_rows: Option<(f32, f32)> = None;
        let mut current_line_no = 0;
        let flash = st.flash_line.filter(|(_, until)| ctx.input(|i| i.time) < *until);
        let mut flash_rows: Option<(f32, f32)> = None;
        let number_color = ui.visuals().weak_text_color();
        let (error_color, warn_color) = (ui.visuals().error_fg_color, ui.visuals().warn_fg_color);
        let mut diag_by_line: std::collections::HashMap<usize, &Diagnostic> = std::collections::HashMap::new();
        for d in env.diagnostics {
            // An error outranks a warning on the same line.
            let keep = diag_by_line.get(&d.line).map(|o| o.warning && !d.warning).unwrap_or(true);
            if keep {
                diag_by_line.insert(d.line, d);
            }
        }
        let mut diag_rows: Vec<(f32, f32, &Diagnostic)> = Vec::new();
        // Where each flagged line ends (its last row's top and right edge):
        // the message is written there.
        let mut diag_ends: Vec<(f32, f32, &Diagnostic)> = Vec::new();
        let number_font = FontId::monospace(env.font_size * 0.85);
        let painter = ui.painter().clone();
        for (row_idx, row) in galley.rows.iter().enumerate() {
            let y0 = origin.y + row.pos.y;
            let y1 = y0 + row.row.height();
            let this_line = line;
            if Some(row_idx) == cursor_row {
                current_line_no = this_line;
            }
            if new_line && y1 >= visible.top() && y0 <= visible.bottom() {
                let diag = diag_by_line.get(&this_line);
                let color = match diag {
                    Some(d) if d.warning => warn_color,
                    Some(_) => error_color,
                    None => number_color,
                };
                painter.text(egui::pos2(origin.x - 10.0, y0 + 1.0), egui::Align2::RIGHT_TOP, this_line.to_string(), number_font.clone(), color);
                if let Some(d) = diag {
                    painter.circle_filled(egui::pos2(full.left() + 6.0, (y0 + y1) / 2.0), 3.0, color);
                    diag_rows.push((y0, y1, d));
                }
            }
            let last_row = row.ends_with_newline || row_idx + 1 == galley.rows.len();
            if last_row && y1 >= visible.top() && y0 <= visible.bottom() {
                if let Some(d) = diag_by_line.get(&this_line) {
                    diag_ends.push((y0, origin.x + row.rect().max.x, d));
                }
            }
            new_line = row.ends_with_newline;
            if new_line {
                line += 1;
            }
            if let Some((fl, _)) = flash {
                if fl == this_line {
                    flash_rows = Some(flash_rows.map_or((y0, y1), |(a, _)| (a, y1)));
                }
            }
        }
        if current_line_no > 0 {
            // Rows of the cursor's logical line.
            let mut l = 1;
            for row in galley.rows.iter() {
                let y0 = origin.y + row.pos.y;
                let y1 = y0 + row.row.height();
                if l == current_line_no {
                    cursor_line_rows = Some(cursor_line_rows.map_or((y0, y1), |(a, _)| (a, y1)));
                }
                if row.ends_with_newline {
                    l += 1;
                }
            }
        }
        let mut bg = Vec::new();
        // Lines with an error get a band; a warning (often a reference not
        // resolved yet, while writing) keeps to its gutter dot and message.
        for (y0, y1, _) in diag_rows.iter().filter(|(_, _, d)| !d.warning) {
            bg.push(Shape::rect_filled(Rect::from_x_y_ranges(full.x_range(), *y0..=*y1), 0.0, error_color.gamma_multiply(0.10)));
        }
        // The message itself, faint, after the end of the line, readable
        // without reaching for the mouse; cut to the room there is.
        let message_font = FontId::proportional((env.font_size * 0.85).max(10.0));
        for (y, x_end, d) in &diag_ends {
            let x = x_end + char_w * 3.0;
            let room = full.right() - x - 8.0;
            if room < char_w * 8.0 {
                continue;
            }
            let c = if d.warning { warn_color } else { error_color };
            let first_line = d.message.lines().next().unwrap_or_default();
            let mut text = first_line.to_string();
            let fits = |t: &str| ctx.fonts_mut(|f| f.layout_no_wrap(t.to_string(), message_font.clone(), c).size().x) <= room;
            while !text.is_empty() && !fits(&text) {
                let keep = text.chars().count().saturating_sub(4);
                text = text.chars().take(keep).collect::<String>().trim_end().to_string() + "…";
                if keep == 0 {
                    text.clear();
                }
            }
            if !text.is_empty() {
                painter.text(egui::pos2(x, y + 1.0), egui::Align2::LEFT_TOP, text, message_font.clone(), c.gamma_multiply(0.75));
            }
        }
        if let Some(p) = ctx.pointer_hover_pos().filter(|p| p.x < origin.x - 2.0 && p.x >= full.left()) {
            if let Some((_, _, d)) = diag_rows.iter().find(|(y0, y1, _)| p.y >= *y0 && p.y <= *y1) {
                let label = if d.warning { t("Warning") } else { t("Error") };
                let hint = d.hint.as_ref().map(|h| format!("\n{h}")).unwrap_or_default();
                response.clone().on_hover_text_at_pointer(format!("{label}: {}{hint}", d.message));
            }
        }
        // The bracket pair at the cursor.
        if output.cursor_range != st.last_cursor || output.response.response.changed() || st.brackets_stale {
            st.brackets_stale = false;
            st.brackets = output.cursor_range.filter(|r| r.is_empty()).and_then(|r| {
                // Only a window around the cursor: the search reach is bounded
                // anyway, and a thesis is hundreds of thousands of chars.
                let at = r.primary.index.0;
                let lo = at.saturating_sub(20_000);
                let chars: Vec<char> = galley.text().chars().skip(lo).take(40_001).collect();
                smart::matching_bracket(&chars, at - lo).map(|(a, b)| (a + lo, b + lo))
            });
        }
        if let (Some((x, y)), true) = (st.brackets, response.has_focus()) {
            for i in [x, y] {
                let a = galley.pos_from_cursor(CCursor::new(i));
                let b = galley.pos_from_cursor(CCursor::new(i + 1));
                let w = if (b.min.y - a.min.y).abs() < 1.0 { (b.min.x - a.min.x).max(2.0) } else { char_w };
                let r = Rect::from_min_size(a.min, Vec2::new(w, a.height())).translate(origin.to_vec2());
                bg.push(Shape::rect_filled(r, 2.0, crate::theme::accent().gamma_multiply(45.0 / 255.0)));
                bg.push(Shape::rect_stroke(r, 2.0, egui::Stroke::new(1.0, crate::theme::accent().gamma_multiply(160.0 / 255.0)), egui::StrokeKind::Inside));
            }
        }
        if let (Some((a, b)), true) = (cursor_line_rows, response.has_focus()) {
            let c = if env.dark { Color32::from_white_alpha(10) } else { Color32::from_black_alpha(10) };
            bg.push(Shape::rect_filled(Rect::from_x_y_ranges(full.x_range(), a..=b), 0.0, c));
        }
        if let Some((a, b)) = flash_rows {
            let c = Color32::from_rgba_unmultiplied(0xE0, 0x7A, 0x50, 60);
            bg.push(Shape::rect_filled(Rect::from_x_y_ranges(full.x_range(), a..=b), 0.0, c));
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
        // Find matches.
        if st.find.open && !st.find.query.is_empty() {
            if let Ok(re) = find_regex(&st.find) {
                let mut count = 0;
                let current_bytes = st.find.current.map(|(a, b)| (char_to_byte(galley.text(), a), char_to_byte(galley.text(), b)));
                for m in re.find_iter(text_as_str(&galley)) {
                    count += 1;
                    if count > 2000 {
                        break;
                    }
                    let a = galley.pos_from_cursor(CCursor::new(byte_to_char(galley.text(), m.start())));
                    let b = galley.pos_from_cursor(CCursor::new(byte_to_char(galley.text(), m.end())));
                    if a.min.y != b.min.y || a.max.y + origin.y < visible.top() || a.min.y + origin.y > visible.bottom() {
                        continue;
                    }
                    let r = Rect::from_min_max(a.min, egui::pos2(b.max.x.max(a.min.x + 2.0), a.max.y)).translate(origin.to_vec2());
                    let is_current = current_bytes == Some((m.start(), m.end()));
                    if is_current {
                        bg.push(Shape::rect_filled(r.expand(1.0), 2.0, Color32::from_rgba_unmultiplied(0xF0, 0x90, 0x30, 150)));
                        bg.push(Shape::rect_stroke(r.expand(1.0), 2.0, egui::Stroke::new(1.0, Color32::from_rgb(0xF0, 0xA0, 0x40)), egui::StrokeKind::Outside));
                    } else {
                        bg.push(Shape::rect_filled(r, 2.0, Color32::from_rgba_unmultiplied(0xF0, 0xC0, 0x40, 70)));
                    }
                }
            }
        }
        ui.painter().set(bg_idx, Shape::Vec(bg));

        if env.typewriter {
            ui.add_space(ui.clip_rect().height() * 0.5);
        }

        if let Some(ci) = st.scroll_to_char.take() {
            let rect = galley.pos_from_cursor(CCursor::new(ci)).translate(origin.to_vec2());
            ui.scroll_to_rect(rect.expand2(Vec2::new(0.0, 4.0)), Some(Align::Center));
        }
        // Cursor-follow scrolling.
        let moved = output.cursor_range != st.last_cursor;
        if let Some(r) = output.cursor_range {
            if st.scroll_to_cursor || (env.typewriter && moved && response.has_focus()) {
                let rect = galley.pos_from_cursor(r.primary).translate(origin.to_vec2());
                ui.scroll_to_rect(rect.expand2(Vec2::new(0.0, 4.0)), Some(Align::Center));
                st.scroll_to_cursor = false;
            } else if st.scroll_to_cursor_soft {
                let rect = galley.pos_from_cursor(r.primary).translate(origin.to_vec2());
                ui.scroll_to_rect(rect.expand2(Vec2::new(0.0, 4.0)), None);
                st.scroll_to_cursor_soft = false;
            }
        }

        // Image dropped from the library.
        if let Some(payload) = response.dnd_release_payload::<ImageDrag>() {
            if let Some(p) = ctx.pointer_interact_pos() {
                let c = galley.cursor_from_pos(p - origin);
                resp.dropped_image = Some((payload.0.clone(), c.index.0));
            }
        }

        // The char under the pointer, when the pointer is really on text
        // (cursor_from_pos snaps to the nearest char, even past a line's end).
        let char_at = |p: egui::Pos2| {
            let c = galley.cursor_from_pos(p - origin);
            let r = galley.pos_from_cursor(c).translate(origin.to_vec2());
            (r.y_range().contains(p.y) && (r.min.x - p.x).abs() <= char_w * 1.5).then_some((c.index.0, r))
        };
        let hover_char = if lang == Lang::Tex && !output.response.response.changed() && response.hovered() {
            ctx.pointer_hover_pos().and_then(char_at)
        } else {
            None
        };

        // Math preview under the pointer. Hidden while typing, like the old
        // tooltip's hideOnChange.
        let mut math_hit = false;
        if let Some(math) = env.math.as_deref_mut() {
            let hit = hover_char.and_then(|(i, _)| {
                let m = mathpreview::find_math_at(galley.text(), i)?;
                let a = galley.pos_from_cursor(CCursor::new(m.from)).translate(origin.to_vec2());
                Some((m, a))
            });
            math_hit = hit.is_some();
            let color = ui.visuals().text_color();
            math.hover(&ctx, hit.as_ref().map(|(m, r)| (m, *r)), color);
        }
        // Otherwise what a \ref, \cite or command points at, for the hover card.
        if !math_hit {
            resp.hover = hover_char.and_then(|(i, r)| refs::target_at(galley.text(), i).map(|t| (t, r)));
        }
        // What the reviewer says about the text under the pointer (unless a
        // misspelled word's card is open there).
        if !math_hit && resp.hover.is_none() && st.spell_hover.is_none() {
            let at = hover_char.map(|(i, _)| char_to_byte(galley.text(), i));
            if let Some(issue) = at.and_then(|at| st.issues.iter().find(|i| i.range.start <= at && at < i.range.end)) {
                response.clone().on_hover_ui_at_pointer(|ui| {
                    ui.set_max_width(360.0);
                    ui.label(issue.message());
                    if let Some(fix) = issue.fix_label() {
                        ui.label(egui::RichText::new(format!("{fix}  ·  Ctrl+.")).small().color(ui.visuals().weak_text_color()));
                    }
                });
            }
        }
        // A misspelled word under the resting pointer: its suggestions in a
        // card that stays while the pointer goes to it, as in the right-click
        // menu.
        let pointer = ctx.pointer_hover_pos();
        if let Some((_, word_rect, card)) = &st.spell_hover {
            let near = |r: &egui::Rect, p: egui::Pos2| r.expand(6.0).contains(p);
            let keep = pointer.is_some_and(|p| near(word_rect, p) || card.is_some_and(|c| near(&c, p)));
            if !keep || output.response.response.changed() || response.secondary_clicked() {
                st.spell_hover = None;
            }
        }
        if st.spell_hover.is_none() && !math_hit && resp.hover.is_none() {
            if let (Some((i, _)), Some(spell)) = (hover_char, env.spell.as_deref_mut()) {
                let gtext = galley.text();
                let byte = char_to_byte(gtext, i);
                // A finding about this very spot wins; one about the whole
                // sentence (a long one) does not hide the spelling.
                let on_issue = st.issues.iter().any(|x| !x.minor() && x.range.start <= byte && byte < x.range.end);
                if let Some((a, b)) = highlight::word_at(gtext, byte).filter(|_| !on_issue) {
                    let word = gtext[a..b].to_string();
                    let region = highlight::language_at(gtext, byte);
                    if spell.is_misspelled_in(&word, region) {
                        let rest = ctx.input(|i| i.pointer.time_since_last_movement()) as f64;
                        if rest >= SPELL_HOVER_DELAY {
                            let range = (byte_to_char(gtext, a), byte_to_char(gtext, b));
                            let r0 = galley.pos_from_cursor(CCursor::new(range.0)).translate(origin.to_vec2());
                            let r1 = galley.pos_from_cursor(CCursor::new(range.1)).translate(origin.to_vec2());
                            // The word's own row (a wrapped word ends on the next).
                            let rect = if (r0.min.y - r1.min.y).abs() < 1.0 { r0.union(r1) } else { r0 };
                            st.spell_hover = Some((SpellMenu { range, suggestions: spell.suggestions_in(&word, region), word }, rect, None));
                        } else {
                            ctx.request_repaint_after(std::time::Duration::from_secs_f64(SPELL_HOVER_DELAY - rest + 0.02));
                        }
                    }
                }
            }
        }

        // Ctrl+click: go to definition.
        if response.clicked() && ctx.input(|i| i.modifiers.command) {
            if let Some((i, _)) = ctx.pointer_interact_pos().and_then(char_at) {
                resp.goto = refs::target_at(galley.text(), i);
            }
        }

        // Spelling suggestions on right click.
        if response.secondary_clicked() {
            st.spell_menu = None;
            st.lint_menu = ctx.pointer_interact_pos().and_then(|p| {
                let at = char_to_byte(text, galley.cursor_from_pos(p - origin).index.0);
                st.issues.iter().find(|i| i.range.start <= at && at < i.range.end).cloned()
            });
            if let (Some(p), Some(spell)) = (ctx.pointer_interact_pos(), env.spell.as_deref_mut()) {
                let c = galley.cursor_from_pos(p - origin);
                let byte = char_to_byte(text, c.index.0);
                if let Some((a, b)) = highlight::word_at(text, byte) {
                    let word = text[a..b].to_string();
                    let region = highlight::language_at(text, byte);
                    if spell.is_misspelled_in(&word, region) {
                        st.spell_menu = Some(SpellMenu {
                            range: (byte_to_char(text, a), byte_to_char(text, b)),
                            suggestions: spell.suggestions_in(&word, region),
                            word,
                        });
                    }
                }
            }
        }
        let mut replace_word: Option<((usize, usize), String)> = None;
        let mut ignore_word: Option<String> = None;
        let mut fix: Option<lint::Issue> = None;
        if let Some((menu, rect, _)) = st.spell_hover.clone() {
            let area = egui::Area::new(st.id.with("spell-hover"))
                .order(egui::Order::Foreground)
                .fixed_pos(rect.left_bottom() + egui::vec2(0.0, 4.0))
                .show(&ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.set_max_width(300.0);
                        ui.label(egui::RichText::new(tf!("\"{word}\" is not in the dictionary", word = menu.word)).small().color(ui.visuals().weak_text_color()));
                        if menu.suggestions.is_empty() {
                            ui.weak(t("No suggestions"));
                        }
                        ui.horizontal_wrapped(|ui| {
                            for s in &menu.suggestions {
                                if ui.button(s).clicked() {
                                    replace_word = Some((menu.range, s.clone()));
                                }
                            }
                        });
                        if ui.add(egui::Button::new(egui::RichText::new(tf!("Ignore \"{word}\"", word = menu.word)).small()).frame(false)).clicked() {
                            ignore_word = Some(menu.word.clone());
                        }
                    });
                });
            if replace_word.is_some() || ignore_word.is_some() {
                st.spell_hover = None;
            } else if let Some(h) = &mut st.spell_hover {
                h.2 = Some(area.response.rect);
            }
        }
        response.context_menu(|ui| {
            if let Some(issue) = &st.lint_menu {
                ui.set_max_width(340.0);
                ui.label(egui::RichText::new(issue.message()).small().color(ui.visuals().weak_text_color()));
                if let Some(label) = issue.fix_label() {
                    if ui.add(icons::button(Icon::Check, label)).clicked() {
                        fix = Some(issue.clone());
                        ui.close();
                    }
                }
                ui.separator();
            }
            if let Some(menu) = &st.spell_menu {
                if menu.suggestions.is_empty() {
                    ui.weak(t("No suggestions"));
                }
                for s in &menu.suggestions {
                    if ui.button(s).clicked() {
                        replace_word = Some((menu.range, s.clone()));
                        ui.close();
                    }
                }
                if ui.button(tf!("Ignore \"{word}\"", word = menu.word)).clicked() {
                    ignore_word = Some(menu.word.clone());
                    ui.close();
                }
                ui.separator();
            }
            if ui.button(t("Copy")).clicked() {
                if let Some((a, b)) = st_selection(&ctx, st.id) {
                    ctx.copy_text(text.chars().skip(a).take(b - a).collect());
                }
                ui.close();
            }
            if ui.button(t("Paste image from the clipboard")).clicked() {
                resp.paste_image_at = Some(cursor_idx.unwrap_or(0));
                ui.close();
            }
            if ui.button(t("Find and replace…")).clicked() {
                st.find.open = true;
                st.find.focus_query = true;
                ui.close();
            }
        });
        if let Some(((a, b), word)) = replace_word {
            st.undo_point(&ctx, text);
            let end = replace_chars(text, a, b, &word);
            st.pending_selection = Some((end, end));
            resp.changed = true;
        }
        if let (Some(w), Some(spell)) = (ignore_word, env.spell.as_deref_mut()) {
            spell.ignore(&w);
        }
        if let Some(issue) = fix {
            resp.changed |= apply_fix(&ctx, text, st, &issue);
        }

        if output.response.response.changed() {
            resp.changed = true;
        }
        out_state = Some((output.cursor_range, galley, origin, response, current_line_no));
    });
    let _ = scroll;

    let Some((cursor_range, galley, origin, response, current_line_no)) = out_state else { return resp };

    // ----- Autocomplete -----
    // Only take the popup when an option was accepted: a tuple pattern would
    // evaluate `take()` every frame and drop the popup right after it opens.
    let accepted = accept.and_then(|i| st.popup.take().map(|p| (i, p)));
    if let Some((i, p)) = accepted {
        if let (Some(item), Some(r)) = (p.items.get(i), cursor_range) {
            let at = r.primary.index.0;
            st.undo_point(ui.ctx(), text);
            let new_cursor = apply_completion(text, p.from, at, &item.apply);
            st.pending_selection = Some((new_cursor, new_cursor));
            resp.changed = true;
        }
    } else if (resp.changed || explicit) && response.has_focus() && env.lang == Lang::Tex {
        st.popup = None;
        if let Some(r) = cursor_range.filter(|r| r.is_empty()) {
            let at = r.primary.index.0;
            let byte = char_to_byte(text, at);
            let line_start = text[..byte].rfind('\n').map(|b| b + 1).unwrap_or(0);
            let before = &text[line_start..byte];
            let target = completion::context_at(before);
            if let Some(target) = target {
                if let Some(items) = env.completions.options(&target, explicit) {
                    let from = at - target.typed.chars().count();
                    st.popup = Some(Popup { from, items, selected: 0 });
                }
            }
        }
    } else if cursor_range != st.last_cursor && !resp.changed {
        // Clicking or arrowing away closes the popup.
        if let (Some(p), Some(r)) = (&st.popup, cursor_range) {
            let at = r.primary.index.0;
            let byte = char_to_byte(text, at);
            let from_b = char_to_byte(text, p.from);
            if at < p.from || text[from_b.min(byte)..byte].contains(['\n', '}', ' ']) {
                st.popup = None;
            }
        }
    }
    if !response.has_focus() && st.popup.is_some() && !st.request_focus {
        // Focus went elsewhere (another panel, a dialog): the popup goes too.
        let over_popup = ctx.layer_id_at(ctx.pointer_hover_pos().unwrap_or_default())
            .map(|l| l.id == st.id.with("completion"))
            .unwrap_or(false);
        if !over_popup {
            st.popup = None;
        }
    }
    if let (Some(p), Some(r)) = (&mut st.popup, cursor_range) {
        let rect = galley.pos_from_cursor(r.primary).translate(origin.to_vec2());
        let mut clicked = None;
        // Below the cursor, or above it when the window ends first.
        const POPUP_H: f32 = 320.0;
        let below = rect.bottom() + POPUP_H < ctx.content_rect().bottom();
        let (pos, pivot) = if below {
            (rect.left_bottom() + Vec2::new(0.0, 2.0), egui::Align2::LEFT_TOP)
        } else {
            (rect.left_top() - Vec2::new(0.0, 2.0), egui::Align2::LEFT_BOTTOM)
        };
        egui::Area::new(st.id.with("completion"))
            .order(egui::Order::Foreground)
            .pivot(pivot)
            .fixed_pos(pos)
            .show(&ctx, |ui| {
                egui::Frame::popup(ui.style()).inner_margin(egui::Margin::same(4)).show(ui, |ui| {
                    ui.set_min_width(320.0);
                    ui.set_max_width(520.0);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    egui::ScrollArea::vertical().max_height(250.0).show(ui, |ui| {
                        // The selection glides from row to row: drawn under
                        // the rows, which leave their own fill to it.
                        let highlight = ui.painter().add(egui::Shape::Noop);
                        let fill = ui.visuals().selection.bg_fill;
                        let mut first_top = None;
                        let mut selected_rect = None;
                        for (i, item) in p.items.iter().enumerate().take(200) {
                            let selected = i == p.selected;
                            let (icon, color) = kind_icon(item.kind);
                            let mut job = egui::text::LayoutJob::default();
                            let mono = FontId::monospace(13.0);
                            job.append(&item.label, 0.0, egui::TextFormat { font_id: mono, color: ui.visuals().text_color(), ..Default::default() });
                            if !item.detail.is_empty() {
                                let detail: String = item.detail.chars().take(60).collect();
                                job.append(&detail, 12.0, egui::TextFormat { font_id: FontId::proportional(12.0), color: ui.visuals().weak_text_color(), ..Default::default() });
                            }
                            let row = egui::Button::new((icon.tinted(color), job)).selected(selected).frame_when_inactive(false);
                            let r = ui.add(row.min_size(Vec2::new(ui.available_width(), 0.0)));
                            first_top.get_or_insert(r.rect.top());
                            if selected {
                                selected_rect = Some(r.rect);
                                r.scroll_to_me(None);
                            }
                            if r.clicked() {
                                clicked = Some(i);
                            }
                        }
                        if let (Some(top), Some(rect)) = (first_top, selected_rect) {
                            let glide = ui.ctx().animate_value_with_time(st.id.with(("completion-selection", p.from)), rect.top() - top, crate::theme::MOTION);
                            let rect = rect.translate(Vec2::new(0.0, glide - (rect.top() - top)));
                            let radius = ui.visuals().widgets.inactive.corner_radius;
                            ui.painter().set(highlight, egui::Shape::rect_filled(rect, radius, fill));
                        }
                    });
                    // What the selected option inserts, or where it comes from.
                    if let Some(item) = p.items.get(p.selected).filter(|i| !i.info.is_empty()) {
                        ui.separator();
                        ui.add(egui::Label::new(egui::RichText::new(&item.info).small().weak()).wrap());
                    }
                });
            });
        if let Some(i) = clicked {
            let item = p.items[i].clone();
            undo_point(ui.ctx(), st.id, text);
            st.undo_after_edit = true;
            let new_cursor = apply_completion(text, p.from, r.primary.index.0, &item.apply);
            st.pending_selection = Some((new_cursor, new_cursor));
            st.request_focus = true;
            st.popup = None;
            resp.changed = true;
        }
    }

    // ----- Cursor line and stats -----
    let cursor_moved = cursor_range != st.last_cursor;
    if cursor_moved || resp.changed {
        if current_line_no > 0 && current_line_no != st.cursor_line {
            st.cursor_line = current_line_no;
            resp.cursor_line_changed = Some(current_line_no);
        }
    }
    st.last_cursor = cursor_range;
    // Whole-document counts only when the text changed, the cursor's
    // column only when it moved: both scan the text.
    let text_changed = resp.changed || st.words_hash == 1;
    if text_changed {
        let h = text_hash(text, &[]);
        if h != st.words_hash {
            st.words_hash = h;
            st.words = if env.lang == Lang::Tex { highlight::prose_words(text) } else { count_words(text) };
            st.stats.chars = text.chars().count();
            st.stats.lines = text.split('\n').count();
        }
    }
    if text_changed || cursor_moved {
        let (col, selected) = match cursor_range {
            Some(r) => {
                let at = r.primary.index.0;
                let byte = char_to_byte(text, at);
                let line_start = text[..byte].rfind('\n').map(|b| b + 1).unwrap_or(0);
                let range = r.as_sorted_char_range();
                (text[line_start..byte].chars().count() + 1, range.end.0 - range.start.0)
            }
            None => (1, 0),
        };
        st.stats.col = col;
        st.stats.selected = selected;
    }
    st.stats.words = st.words;
    st.stats.line = st.cursor_line;
    resp
}

/// Applies a finding's fix as its own undo step; the cursor lands after it.
fn apply_fix(ctx: &egui::Context, text: &mut String, st: &mut TabEditor, issue: &lint::Issue) -> bool {
    let Some((range, insert)) = &issue.fix else { return false };
    if range.end > text.len() || !text.is_char_boundary(range.start) || !text.is_char_boundary(range.end) {
        return false;
    }
    st.undo_point(ctx, text);
    let (from, to) = (byte_to_char(text, range.start), byte_to_char(text, range.end));
    let end = replace_chars(text, from, to, insert);
    st.pending_selection = Some((end, end));
    st.request_focus = true;
    true
}

/// Enter, Tab, Backspace between a pair, the openers `{ [ ( $`, and the
/// line shortcuts (Ctrl+/, Alt+↑/↓, Ctrl+Shift+D); see `smart`. Consumes
/// what it handles so the TextEdit never sees it. True when the text changed.
fn smart_keys(ui: &mut egui::Ui, text: &mut String, st: &mut TabEditor, lang: Lang) -> bool {
    let ctx = ui.ctx().clone();
    let Some(mut sel) = st_selection(&ctx, st.id) else { return false };
    let tex = lang == Lang::Tex;
    let pairs = tex;
    let mut edits: Vec<smart::Edit> = Vec::new();

    // Typed text: when a frame carries an opener/closer, this frame's text
    // events are applied here, in order, so pairing and plain typing interleave
    // correctly.
    let special = |t: &str| t.chars().any(|c| matches!(c, '{' | '[' | '(' | '}' | ']' | ')' | '$'));
    if pairs && ui.input(|i| i.events.iter().any(|e| matches!(e, egui::Event::Text(t) if special(t)))) {
        let typed: Vec<String> = ui.input_mut(|i| {
            let mut typed = Vec::new();
            i.events.retain(|e| match e {
                egui::Event::Text(t) => {
                    typed.push(t.clone());
                    false
                }
                _ => true,
            });
            typed
        });
        st.undo_point(&ctx, text);
        let mut changed = false;
        for c in typed.iter().flat_map(|t| t.chars()) {
            let (a, b) = (sel.0.min(sel.1), sel.0.max(sel.1));
            let e = smart::on_char(text, sel, c)
                .unwrap_or_else(|| smart::Edit { from: a, to: b, insert: c.to_string(), sel: (a + 1, a + 1) });
            changed |= e.from != e.to || !e.insert.is_empty();
            replace_chars(text, e.from, e.to, &e.insert);
            sel = e.sel;
        }
        set_selection(&ctx, st.id, sel);
        return changed;
    }

    ui.input_mut(|i| {
        if i.consume_key(Modifiers::NONE, Key::Enter) {
            edits.push(smart::on_enter(text, sel));
        } else if i.consume_key(Modifiers::NONE, Key::Tab) {
            edits.push(smart::indent(text, sel, false));
        } else if i.consume_key(Modifiers::SHIFT, Key::Tab) {
            edits.push(smart::indent(text, sel, true));
        } else if pairs && i.consume_key(Modifiers::COMMAND, Key::Slash) {
            edits.push(smart::toggle_comment(text, sel, if tex { "%" } else { "//" }));
        } else if i.consume_key(Modifiers::COMMAND | Modifiers::SHIFT, Key::D) {
            edits.push(smart::duplicate_lines(text, sel));
        } else if i.key_pressed(Key::ArrowUp) && i.modifiers.matches_exact(Modifiers::ALT) {
            if let Some(e) = smart::move_lines(text, sel, true) {
                i.consume_key(Modifiers::ALT, Key::ArrowUp);
                edits.push(e);
            }
        } else if i.key_pressed(Key::ArrowDown) && i.modifiers.matches_exact(Modifiers::ALT) {
            if let Some(e) = smart::move_lines(text, sel, false) {
                i.consume_key(Modifiers::ALT, Key::ArrowDown);
                edits.push(e);
            }
        } else if pairs && i.key_pressed(Key::Backspace) && i.modifiers.is_none() {
            if let Some(e) = smart::on_backspace(text, sel) {
                i.consume_key(Modifiers::NONE, Key::Backspace);
                edits.push(e);
            }
        }
    });
    let Some(e) = edits.pop() else { return false };
    st.undo_point(&ctx, text);
    replace_chars(text, e.from, e.to, &e.insert);
    set_selection(&ctx, st.id, e.sel);
    st.scroll_to_cursor_soft = true;
    true
}

fn undo_point(ctx: &egui::Context, id: egui::Id, text: &str) {
    let Some(mut state) = egui::TextEdit::load_state(ctx, id) else { return };
    let Some(range) = state.cursor.char_range() else { return };
    let mut undoer = state.undoer();
    undoer.add_undo(&(range, text.to_string()));
    state.set_undoer(undoer);
    state.store(ctx, id);
}

fn set_selection(ctx: &egui::Context, id: egui::Id, sel: (usize, usize)) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    state.cursor.set_char_range(Some(CCursorRange::two(CCursor::new(sel.0), CCursor::new(sel.1))));
    state.store(ctx, id);
}

fn st_selection(ctx: &egui::Context, id: egui::Id) -> Option<(usize, usize)> {
    let r = egui::TextEdit::load_state(ctx, id)?.cursor.char_range()?;
    let range = r.as_sorted_char_range();
    Some((range.start.0, range.end.0))
}

fn text_as_str(g: &egui::Galley) -> &str {
    g.text()
}

/// Applies a completion replacing `[from, at)` and returns the new cursor.
fn apply_completion(text: &mut String, from: usize, at: usize, apply: &Apply) -> usize {
    let next_is_brace = text.chars().nth(at) == Some('}');
    match apply {
        Apply::Text(s) => replace_chars(text, from, at, s),
        Apply::Template(s, cursor) => {
            replace_chars(text, from, at, s);
            from + cursor
        }
        Apply::Environment(name, body) => {
            let byte = char_to_byte(text, from);
            let line_start = text[..byte].rfind('\n').map(|b| b + 1).unwrap_or(0);
            let indent: String = text[line_start..].chars().take_while(|c| *c == ' ' || *c == '\t').collect();
            let full = environment_text(name, body, &indent);
            let (full, cursor) = split_marker(&full);
            let to = if next_is_brace { at + 1 } else { at };
            replace_chars(text, from, to, &full);
            from + cursor
        }
        Apply::EndName(name) => {
            let to = if next_is_brace { at + 1 } else { at };
            replace_chars(text, from, to, &format!("{name}}}"))
        }
    }
}

/// What follows `\begin{` for an environment: the name, its arguments, the
/// body indented one level under `indent`, and the `\end`. The first `|`
/// marks the cursor; without one it lands on the body line.
fn environment_text(name: &str, body: &str, indent: &str) -> String {
    let (args, rest) = if body.starts_with('{') || body.starts_with('[') {
        body.split_once('\n').unwrap_or((body, ""))
    } else {
        ("", body)
    };
    let inner = format!("{indent}  ");
    let lines = if rest.is_empty() {
        if args.contains('|') { inner.clone() } else { format!("{inner}|") }
    } else {
        rest.split('\n').map(|l| if l.is_empty() { String::new() } else { format!("{inner}{l}") }).collect::<Vec<_>>().join("\n")
    };
    format!("{name}}}{args}\n{lines}\n{indent}\\end{{{name}}}")
}

/// Removes the first `|` and returns where it was, in chars.
fn split_marker(s: &str) -> (String, usize) {
    match s.find('|') {
        Some(b) => (s.replacen('|', "", 1), s[..b].chars().count()),
        None => (s.to_string(), s.chars().count()),
    }
}

fn kind_icon(kind: ItemKind) -> (Icon, Color32) {
    match kind {
        ItemKind::Command => (Icon::Code, Color32::from_rgb(0xE8, 0x8B, 0x6A)),
        ItemKind::Math => (Icon::Sigma, Color32::from_rgb(0x8F, 0xC9, 0xA3)),
        ItemKind::Environment => (Icon::Braces, Color32::from_rgb(0xC3, 0x9B, 0xE0)),
        ItemKind::Label => (Icon::Tag, Color32::from_rgb(0x9C, 0xC2, 0xE6)),
        ItemKind::Citation => (Icon::BookOpenText, Color32::from_rgb(0xE0, 0xB8, 0x6A)),
        ItemKind::File => (Icon::FileImage, Color32::from_rgb(0x9C, 0xC2, 0xE6)),
        ItemKind::Package => (Icon::Package, Color32::from_rgb(0xB5, 0xA9, 0x9A)),
        ItemKind::Snippet => (Icon::Zap, Color32::from_rgb(0xF0, 0xC0, 0x40)),
        ItemKind::Project => (Icon::Star, crate::theme::accent()),
    }
}

fn find_regex(f: &FindBar) -> Result<regex::Regex, regex::Error> {
    let pattern = if f.regex { f.query.clone() } else { regex::escape(&f.query) };
    regex::RegexBuilder::new(&pattern).case_insensitive(!f.case_sensitive).multi_line(true).build()
}

fn find_bar(ui: &mut egui::Ui, text: &mut String, st: &mut TabEditor) {
    let ctx = ui.ctx().clone();
    let mut go: Option<bool> = None; // Some(forward)
    let mut replace_one = false;
    let mut replace_all = false;
    let mut retyped = false;
    let sel_now = st.selection(&ctx).unwrap_or((0, 0));
    egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 5)).fill(ui.visuals().panel_fill).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 3.0;
        ui.horizontal(|ui| {
            let caret = if st.find.replace_open { Icon::ChevronDown } else { Icon::ChevronRight };
            if ui.add(icons::icon_button(caret).frame(false)).on_hover_text(t("Show/hide replace")).clicked() {
                st.find.replace_open = !st.find.replace_open;
            }
            let q = ui.add(egui::TextEdit::singleline(&mut st.find.query).hint_text(t("Search")).desired_width(220.0));
            if st.find.focus_query {
                q.request_focus();
                st.find.focus_query = false;
            }
            retyped = q.changed();
            if q.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                go = Some(!ui.input(|i| i.modifiers.shift));
                q.request_focus();
            }
            ui.toggle_value(&mut st.find.case_sensitive, "Aa").on_hover_text(t("Match case"));
            ui.toggle_value(&mut st.find.regex, ".*").on_hover_text(t("Regular expression"));
            // "3 de 35": where the current selection sits among the matches.
            match find_regex(&st.find) {
                Ok(re) if !st.find.query.is_empty() => {
                    st.find.error = None;
                    let cur = st.find.current.unwrap_or(sel_now);
                    let (a, b) = (char_to_byte(text, cur.0), char_to_byte(text, cur.1));
                    let mut total = 0;
                    let mut current = None;
                    for (k, m) in re.find_iter(text).enumerate() {
                        total += 1;
                        if m.start() == a && m.end() == b {
                            current = Some(k + 1);
                        }
                    }
                    let label = match (total, current) {
                        (0, _) => t("no results").to_string(),
                        (total, Some(c)) => tf!("{current} of {total}", current = c, total),
                        (total, None) => crate::i18n::tn("{n} result", "{n} results", total),
                    };
                    let rt = egui::RichText::new(label).small();
                    ui.label(if total == 0 { rt.color(ui.visuals().warn_fg_color) } else { rt.weak() });
                }
                Err(e) => {
                    st.find.error = Some(e.to_string());
                    ui.colored_label(ui.visuals().error_fg_color, t("invalid regex"));
                }
                _ => {}
            }
            if ui.add(icons::icon_button(Icon::ArrowUp)).on_hover_text(t("Previous (Shift+Enter)")).clicked() {
                go = Some(false);
            }
            if ui.add(icons::icon_button(Icon::ArrowDown)).on_hover_text(t("Next (Enter)")).clicked() {
                go = Some(true);
            }
            if ui.add(icons::icon_button(Icon::X)).on_hover_text(t("Close (Esc)")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                st.find.open = false;
                st.request_focus = true;
                // Leave the match selected, ready to be typed over.
                if let Some(cur) = st.find.current.take() {
                    st.pending_selection = Some(cur);
                }
            }
        });
        if st.find.replace_open {
            ui.horizontal(|ui| {
                ui.add_space(18.0);
                ui.add(egui::TextEdit::singleline(&mut st.find.replace).hint_text(t("Replace with")).desired_width(220.0));
                replace_one = ui.button(t("Replace")).on_hover_text(t("Replaces the selected occurrence and goes to the next")).clicked();
                replace_all = ui.button(t("Replace all")).clicked();
            });
        }
    });
    if st.find.query.is_empty() {
        st.find.current = None;
        return;
    }
    let Ok(re) = find_regex(&st.find) else { return };
    // Where searching continues from: the current match, else the cursor.
    let cur = st.find.current.unwrap_or(sel_now);
    // Typing searches as you go, from where the current match starts.
    if retyped && go.is_none() {
        st.find.current = Some((cur.0, cur.0));
        go = Some(true);
    }
    let cur = st.find.current.unwrap_or(sel_now);
    if replace_all {
        let replacement = st.find.replace.clone();
        let new = if st.find.regex {
            re.replace_all(text, replacement.as_str()).into_owned()
        } else {
            re.replace_all(text, regex::NoExpand(&replacement)).into_owned()
        };
        if new != *text {
            st.undo_point(ui.ctx(), text);
            *text = new;
            st.invalidate();
        }
        st.find.current = None;
        return;
    }
    if replace_one {
        // Replace the current match, then move on to the next.
        let (bs, be) = (char_to_byte(text, cur.0), char_to_byte(text, cur.1));
        if let Some(m) = re.find_at(text, bs).filter(|m| m.start() == bs && m.end() == be && be > bs) {
            let rep = if st.find.regex {
                let caps = re.captures_at(text, m.start()).unwrap();
                let mut dst = String::new();
                caps.expand(&st.find.replace, &mut dst);
                dst
            } else {
                st.find.replace.clone()
            };
            st.undo_point(ui.ctx(), text);
            let end = replace_chars(text, cur.0, cur.1, &rep);
            st.invalidate();
            st.find.current = Some((end, end));
        }
        go = Some(true);
    }
    if let Some(forward) = go {
        let cur = st.find.current.unwrap_or(sel_now);
        let from = char_to_byte(text, if forward { cur.1 } else { cur.0 });
        let m = if forward {
            re.find_at(text, from.min(text.len())).or_else(|| re.find(text))
        } else {
            re.find_iter(text).filter(|m| m.start() < from).last().or_else(|| re.find_iter(text).last())
        };
        if let Some(m) = m.filter(|m| m.end() > m.start()) {
            let (a, b) = (byte_to_char(text, m.start()), byte_to_char(text, m.end()));
            st.find.current = Some((a, b));
            st.scroll_to_char = Some(a);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn environment_bodies() {
        let (t, c) = split_marker(&environment_text("equation", "|", "  "));
        assert_eq!(t, "equation}\n    \n  \\end{equation}");
        assert_eq!(&t[..c], "equation}\n    ");
        let (t, c) = split_marker(&environment_text("tabular", "{|}\n", ""));
        assert_eq!(t, "tabular}{}\n  \n\\end{tabular}");
        assert_eq!(c, "tabular}{".len());
        let (t, _) = split_marker(&environment_text("figure", "[htbp]\n\\centering\n\\caption{|}", ""));
        assert_eq!(t, "figure}[htbp]\n  \\centering\n  \\caption{}\n\\end{figure}");
    }

    #[test]
    fn completion_applies_environment() {
        let mut text = "  \\begin{fig}".to_string();
        let at = text.chars().count() - 1; // before the closing brace
        let from = at - 3;
        let c = apply_completion(&mut text, from, at, &Apply::Environment("itemize".into(), "\\item |".into()));
        assert_eq!(text, "  \\begin{itemize}\n    \\item \n  \\end{itemize}");
        assert_eq!(text.chars().take(c).collect::<String>(), "  \\begin{itemize}\n    \\item ");
    }
}
