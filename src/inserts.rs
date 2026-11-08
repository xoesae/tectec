//! Dialogs that build a list or a table from what is typed in a form, with
//! the typeset result beside it, and the LaTeX they insert.

use crate::i18n::{n, t, tn};
use crate::icons::{self, Icon};
use crate::mathpreview::{self, RenderError};
use crate::tf;
use egui::RichText;
use std::sync::mpsc::{channel, Receiver};

/// How long the form has to stay unchanged before the preview is typeset.
const PREVIEW_IDLE: f64 = 0.35;

/// Escapes the characters that would break the LaTeX around a typed value
/// (`&` splits a table cell, `%` comments out the rest of the line, `#` is
/// a macro parameter), unless already escaped. Commands such as
/// `\textbf{…}` and math in `$…$` go through as typed.
pub fn escape(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut prev = '\0';
    for c in text.trim().chars() {
        if matches!(c, '&' | '%' | '#') && prev != '\\' {
            out.push('\\');
        }
        out.push(c);
        prev = c;
    }
    out
}

/// A `\label` is safest as plain ASCII.
pub fn label_slug(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    let ascii: String = s.nfd().filter(|c| c.is_ascii()).collect::<String>().to_lowercase();
    let mut out = String::new();
    for c in ascii.chars() {
        if c.is_ascii_alphanumeric() {
            out.push(c);
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "fig".into()
    } else {
        out
    }
}

/// The typeset preview of a LaTeX fragment: rendered in the background a
/// moment after the fragment stops changing.
#[derive(Default)]
pub struct LivePreview {
    source: String,
    changed_at: f64,
    rendering: Option<(String, Receiver<Result<egui::ColorImage, String>>)>,
    shown: Option<(String, egui::TextureHandle)>,
    error: Option<(String, String)>,
}

impl LivePreview {
    /// Shows the preview of `body`, `height` tall.
    pub fn ui(&mut self, ui: &mut egui::Ui, body: &str, height: f32) {
        let ctx = ui.ctx().clone();
        let now = ui.input(|i| i.time);
        if body != self.source {
            self.source = body.to_string();
            self.changed_at = now;
        }
        if let Some((source, rx)) = &self.rendering {
            if let Ok(result) = rx.try_recv() {
                match result {
                    Ok(img) => {
                        let tex = ctx.load_texture("insert-preview", img, egui::TextureOptions::LINEAR);
                        self.shown = Some((source.clone(), tex));
                        self.error = None;
                    }
                    Err(e) => self.error = Some((source.clone(), e)),
                }
                self.rendering = None;
            }
        }
        let shown_current = self.shown.as_ref().is_some_and(|(s, _)| *s == self.source);
        let failed_current = self.error.as_ref().is_some_and(|(s, _)| *s == self.source);
        if self.rendering.is_none() && !shown_current && !failed_current {
            let wait = PREVIEW_IDLE - (now - self.changed_at);
            if wait <= 0.0 {
                let (tx, rx) = channel();
                let doc = document(&self.source);
                let repaint = ctx.clone();
                std::thread::spawn(move || {
                    let result = mathpreview::render_document(&doc).map_err(|e| match e {
                        RenderError::Tex(Some(m)) => tf!("LaTeX error: {message}", message = m),
                        RenderError::Tex(None) => t("LaTeX could not typeset this.").into(),
                        RenderError::Empty => String::new(),
                        RenderError::Other(e) => e,
                    });
                    let _ = tx.send(result);
                    repaint.request_repaint();
                });
                self.rendering = Some((self.source.clone(), rx));
            } else {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(wait + 0.01));
            }
        }

        let busy = self.rendering.is_some() || (!shown_current && !failed_current);
        heading(ui, t("Preview"), |ui| {
            if busy {
                ui.add(egui::Spinner::new().size(10.0));
            }
        });
        let frame = egui::Frame::new()
            .fill(ui.visuals().extreme_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(16));
        frame.show(ui, |ui| {
            let size = egui::vec2(ui.available_width(), (height - 32.0).max(180.0));
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            match (&self.error, &self.shown) {
                (Some((source, e)), _) if *source == self.source && !e.is_empty() => {
                    let label = egui::Label::new(RichText::new(e).color(ui.visuals().warn_fg_color)).wrap();
                    ui.scope_builder(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)), |ui| {
                        ui.add(label);
                    });
                }
                (_, Some((source, tex))) => {
                    // As large as it was typeset, or smaller to fit.
                    let mut img = mathpreview::display_size(tex);
                    let fit = (rect.width() / img.x).min(rect.height() / img.y).min(1.0);
                    img *= fit;
                    // Faded while it is behind the form.
                    let color = if *source == self.source { ui.visuals().text_color() } else { ui.visuals().weak_text_color() };
                    egui::Image::new((tex.id(), img)).tint(color).paint_at(ui, egui::Rect::from_center_size(rect.center(), img));
                }
                _ => {}
            }
        });
    }
}

/// A small heading in capitals, spaced out: CELLS, PREVIEW.
fn caps(text: &str, color: egui::Color32) -> RichText {
    RichText::new(text.to_uppercase()).small().strong().color(color).extra_letter_spacing(0.8)
}

/// A section's heading row, the same height everywhere so the headings of
/// a dialog's two columns line up: the heading, then what `add` puts after
/// it (a count, a button, pushed right with a right-to-left layout).
fn heading(ui: &mut egui::Ui, text: &str, add: impl FnOnce(&mut egui::Ui)) {
    crate::widgets::bar_of(ui, 28.0, |ui| {
        ui.add(egui::Label::new(caps(text, ui.visuals().weak_text_color())).selectable(false));
        add(ui);
    });
}

/// A hint with `**emphasis**` and `` `code` `` in it, faint around them.
fn hint(ui: &egui::Ui, text: &str) -> egui::text::LayoutJob {
    use egui::text::{LayoutJob, TextFormat};
    let weak = ui.visuals().weak_text_color();
    let strong = ui.visuals().text_color();
    let body = egui::FontId::proportional(12.5);
    let mono = egui::FontId::monospace(12.0);
    let mut job = LayoutJob::default();
    let mut rest = text;
    while !rest.is_empty() {
        let bold = rest.find("**");
        let code = rest.find('`');
        let (at, marker) = match (bold, code) {
            (Some(b), Some(c)) if c < b => (c, "`"),
            (Some(b), _) => (b, "**"),
            (None, Some(c)) => (c, "`"),
            (None, None) => (rest.len(), ""),
        };
        job.append(&rest[..at], 0.0, TextFormat::simple(body.clone(), weak));
        if marker.is_empty() {
            break;
        }
        let inner = &rest[at + marker.len()..];
        let end = inner.find(marker).unwrap_or(inner.len());
        let format = if marker == "`" { TextFormat::simple(mono.clone(), strong) } else { TextFormat::simple(body.clone(), strong) };
        job.append(&inner[..end], 0.0, format);
        rest = inner.get(end + marker.len()..).unwrap_or("");
    }
    job
}

/// A standalone document around a fragment, as wide as an A4 page's text,
/// in Portuguese with the interface ("Tabela 1" under a caption).
fn document(body: &str) -> String {
    let portuguese = crate::i18n::locale() == crate::i18n::Locale::Portuguese && crate::backend::latex::has_file("brazil.ldf");
    let babel = if portuguese { "\\usepackage[brazil]{babel}\n" } else { "" };
    format!(
        "\\documentclass{{article}}\n\\usepackage[a4paper,margin=2.5cm]{{geometry}}\n\\usepackage{{amsmath,amssymb}}\n{babel}\\pagestyle{{empty}}\n\\begin{{document}}\n{body}\n\\end{{document}}\n"
    )
}

/// The bottom of a dialog: a hint on the left, Cancel and its action on the
/// right. `Some(true)` for the action, `Some(false)` for Cancel.
fn footer(ui: &mut egui::Ui, text: &str, confirm: &str) -> Option<bool> {
    let mut out = None;
    let buttons = |ui: &mut egui::Ui, out: &mut Option<bool>| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(crate::widgets::primary(confirm)).clicked() {
                *out = Some(true);
            }
            if ui.add(crate::widgets::secondary(t("Cancel"))).clicked() {
                *out = Some(false);
            }
        });
    };
    // A narrow dialog: the hint above the buttons.
    if ui.available_width() < 560.0 {
        ui.add(egui::Label::new(hint(ui, text)).wrap());
        ui.horizontal(|ui| buttons(ui, &mut out));
        return out;
    }
    ui.horizontal(|ui| {
        let buttons = 300.0;
        let job = hint(ui, text);
        ui.allocate_ui_with_layout(egui::vec2((ui.available_width() - buttons).max(120.0), 0.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.add(egui::Label::new(job).wrap());
        });
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add(crate::widgets::primary(confirm)).clicked() {
                out = Some(true);
            }
            if ui.add(crate::widgets::secondary(t("Cancel"))).clicked() {
                out = Some(false);
            }
        });
    });
    out
}

/// A dialog as a window: a header with its title and ×, the body, and a
/// footer, divided by lines. Dragged by any empty spot, the header first.
/// Returns true when it is closed (its × or Esc).
fn dialog(ctx: &egui::Context, id: &str, title: &str, width: f32, body: impl FnOnce(&mut egui::Ui), foot: impl FnOnce(&mut egui::Ui)) -> bool {
    let width = width.min(ctx.content_rect().width() - 60.0);
    let mut close = false;
    let pad = 24;
    let frame = egui::Frame::window(&ctx.global_style()).inner_margin(0);
    egui::Window::new(title)
        .id(egui::Id::new(id))
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        // Centred across, a little above the middle (its height is only
        // known once drawn).
        .default_pos(ctx.content_rect().center() - egui::vec2(width / 2.0, ctx.content_rect().height() * 0.32))
        .show(ctx, |ui| {
            ui.set_width(width);
            crate::theme::framed(ui);
            ui.spacing_mut().item_spacing.y = 0.0;
            egui::Frame::NONE.inner_margin(egui::Margin { left: pad, right: pad - 10, top: 12, bottom: 12 }).show(ui, |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(RichText::new(title).size(16.0).strong()).selectable(false));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        // A bare ×, framed only under the pointer.
                        let w = &mut ui.visuals_mut().widgets.inactive;
                        w.weak_bg_fill = egui::Color32::TRANSPARENT;
                        w.bg_stroke = egui::Stroke::NONE;
                        if ui.add(icons::icon_button(Icon::X)).on_hover_text(t("Close")).clicked() {
                            close = true;
                        }
                    });
                });
            });
            ui.add(egui::Separator::default().spacing(0.0));
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(pad, 20)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                body(ui);
            });
            ui.add(egui::Separator::default().spacing(0.0));
            egui::Frame::NONE.inner_margin(egui::Margin::symmetric(pad, 16)).show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(10.0, 10.0);
                foot(ui);
            });
        });
    close || ctx.input(|i| i.key_pressed(egui::Key::Escape))
}

/// The form on the left, the preview on the right. Returns the form's
/// height, which the preview takes the next frame.
fn form_and_preview(ui: &mut egui::Ui, form: impl FnOnce(&mut egui::Ui), preview: impl FnOnce(&mut egui::Ui)) -> f32 {
    let gap = 28.0;
    let left = ((ui.available_width() - gap) * 0.62).floor();
    let mut height = 0.0;
    ui.horizontal_top(|ui| {
        height = ui
            .vertical(|ui| {
                ui.set_width(left);
                form(ui);
            })
            .response
            .rect
            .height();
        ui.add_space(gap - ui.spacing().item_spacing.x);
        ui.vertical(|ui| {
            ui.set_width(ui.available_width());
            preview(ui);
        });
    });
    height
}

/// A row of a form: its label in a column of its own, then the field.
fn form_row(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal(|ui| {
        ui.allocate_ui_with_layout(egui::vec2(80.0, 34.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.set_min_width(80.0);
            ui.add(egui::Label::new(RichText::new(label).color(ui.visuals().weak_text_color())).selectable(false));
        });
        add(ui);
    });
}

/// A text field with room around its text, as tall as a form row.
fn roomy(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(egui::Margin::symmetric(12, 8))
}

// ---------- List ----------

#[derive(Clone, Copy, PartialEq)]
enum ListKind {
    Bullets,
    Numbered,
}

impl ListKind {
    fn env(self) -> &'static str {
        match self {
            ListKind::Bullets => "itemize",
            ListKind::Numbered => "enumerate",
        }
    }
}

/// Items, each at a nesting level (0 to 3).
pub struct ListBuilder {
    kind: ListKind,
    items: Vec<(usize, String)>,
    /// The item whose field takes the focus next frame.
    focus: Option<usize>,
    preview: LivePreview,
    /// The form's height last frame, which the preview matches.
    form_height: f32,
}

const MAX_LEVEL: usize = 3;
/// How far each nesting level moves an item in.
const LIST_INDENT: f32 = 28.0;
/// The column of the bullet or number, before the field.
const MARKER_WIDTH: f32 = 18.0;

/// Bullets or numbers, as two segments of one control.
fn kind_control(ui: &mut egui::Ui, kind: &mut ListKind) {
    segmented(ui, 290.0, kind, &[(ListKind::Bullets, t("• Bulleted")), (ListKind::Numbered, t("1. Numbered"))]);
}

/// One choice among a few, as segments of one control, the chosen one lit.
fn segmented<T: Copy + PartialEq>(ui: &mut egui::Ui, width: f32, value: &mut T, options: &[(T, &str)]) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, 34.0), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    ui.painter().rect(rect, 6.0, visuals.extreme_bg_color, visuals.widgets.noninteractive.bg_stroke, egui::StrokeKind::Inside);
    let inner = rect.shrink(4.0);
    let segment = inner.width() / options.len() as f32;
    for (k, (option, label)) in options.iter().enumerate() {
        let r = egui::Rect::from_min_size(inner.min + egui::vec2(segment * k as f32, 0.0), egui::vec2(segment, inner.height()));
        let resp = ui.interact(r, response.id.with(k), egui::Sense::click());
        if resp.clicked() {
            *value = *option;
        }
        let on = *value == *option;
        if on {
            ui.painter().rect_filled(r, 4.0, crate::theme::accent().gamma_multiply(0.28));
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let color = if on { crate::theme::accent() } else { visuals.text_color() };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, *label, egui::FontId::proportional(13.0), color);
    }
}

/// A small icon button beside an item, framed only under the pointer.
fn row_button(ui: &mut egui::Ui, icon: Icon, enabled: bool, tip: &str) -> egui::Response {
    let button = egui::Button::new(icon.image(egui::Color32::WHITE, 13.0))
        .image_tint_follows_text_color(true)
        .frame_when_inactive(false)
        .min_size(egui::vec2(24.0, 24.0));
    ui.add_enabled(enabled, button).on_hover_text(tip)
}

impl Default for ListBuilder {
    fn default() -> Self {
        Self { kind: ListKind::Bullets, items: vec![(0, String::new())], focus: Some(0), preview: LivePreview::default(), form_height: 0.0 }
    }
}

impl ListBuilder {
    /// The list, nested lists inside the item before them.
    pub fn latex(&self) -> String {
        let env = self.kind.env();
        let items: Vec<&(usize, String)> = self.items.iter().filter(|(_, s)| !s.trim().is_empty()).collect();
        let mut out = format!("\\begin{{{env}}}\n");
        let mut level = 0;
        if items.is_empty() {
            out.push_str("  \\item \n");
        }
        for (i, (l, text)) in items.iter().enumerate() {
            // A level deeper than the previous item's only goes one deeper.
            let l = if i == 0 { 0 } else { (*l).min(level + 1) };
            while level < l {
                level += 1;
                out.push_str(&format!("{}\\begin{{{env}}}\n", "  ".repeat(level)));
            }
            while level > l {
                out.push_str(&format!("{}\\end{{{env}}}\n", "  ".repeat(level)));
                level -= 1;
            }
            out.push_str(&format!("{}\\item {}\n", "  ".repeat(level + 1), escape(text)));
        }
        while level > 0 {
            out.push_str(&format!("{}\\end{{{env}}}\n", "  ".repeat(level)));
            level -= 1;
        }
        out.push_str(&format!("\\end{{{env}}}\n"));
        out
    }

    /// `Some(Some(latex))` to insert, `Some(None)` when canceled.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Option<String>> {
        let mut outcome = None;
        let body = self.latex();
        let mut preview = std::mem::take(&mut self.preview);
        let height = self.form_height;
        let mut form_height = height;
        let closed = dialog(
            ctx,
            "list-builder",
            t("Insert list"),
            960.0,
            |ui| {
                form_height = form_and_preview(
                    ui,
                    |ui| {
                        heading(ui, t("Type"), |_| {});
                        kind_control(ui, &mut self.kind);
                        ui.add_space(4.0);
                        self.items_ui(ui);
                    },
                    |ui| preview.ui(ui, &body, height),
                );
            },
            |ui| {
                if let Some(ok) = footer(ui, t("**Enter** adds an item; **‹ ›** nest it. LaTeX such as `\\textbf{…}` and `$x^2$` works."), t("Insert list")) {
                    outcome = Some(ok.then(|| body.clone()));
                }
            },
        );
        self.form_height = form_height;
        self.preview = preview;
        if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }

    fn items_ui(&mut self, ui: &mut egui::Ui) {
        let mut add_after: Option<usize> = None;
        let mut remove: Option<usize> = None;
        let mut shift: Option<(usize, isize)> = None;
        let count = self.items.len();
        let weak = ui.visuals().weak_text_color();
        heading(ui, t("Items"), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(tn("{n} item", "{n} items", count)).small().color(weak)).selectable(false));
            });
        });
        egui::ScrollArea::vertical().max_height(340.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.set_max_width(ui.available_width());
            for (i, (level, text)) in self.items.iter_mut().enumerate() {
                ui.horizontal(|ui| {
                    ui.add_space(*level as f32 * LIST_INDENT);
                    let marker = match self.kind {
                        ListKind::Bullets => RichText::new(["•", "◦", "▪", "·"][*level]).size(11.0),
                        ListKind::Numbered => RichText::new(format!("{}.", i + 1)).small(),
                    };
                    ui.add_sized([MARKER_WIDTH, 34.0], egui::Label::new(marker.color(weak)).selectable(false));
                    // The buttons at the right end, the field in what is left.
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if row_button(ui, Icon::X, count > 1, t("Remove")).clicked() {
                            remove = Some(i);
                        }
                        if row_button(ui, Icon::ChevronRight, i > 0 && *level < MAX_LEVEL, t("Nested in the item above")).clicked() {
                            shift = Some((i, 1));
                        }
                        if row_button(ui, Icon::ChevronLeft, *level > 0, t("Less indented")).clicked() {
                            shift = Some((i, -1));
                        }
                        ui.add_space(8.0);
                        let field = ui.scope(|ui| {
                            // The field keeps its edge.
                            ui.visuals_mut().widgets.inactive.bg_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
                            ui.add(roomy(text).desired_width(ui.available_width()).hint_text(t("Item")))
                        })
                        .inner;
                        if self.focus == Some(i) {
                            field.request_focus();
                        }
                        if field.lost_focus() && ui.input(|inp| inp.key_pressed(egui::Key::Enter)) {
                            add_after = Some(i);
                        }
                        if field.has_focus() && text.is_empty() && count > 1 && ui.input(|inp| inp.key_pressed(egui::Key::Backspace)) {
                            remove = Some(i);
                        }
                    });
                });
            }
        });
        // Under the fields, lined up with them.
        ui.horizontal(|ui| {
            ui.add_space(MARKER_WIDTH);
            let add = egui::Button::new(RichText::new(format!("+  {}", t("Add item")))).min_size(egui::vec2(0.0, 30.0));
            if ui.add(add).clicked() {
                add_after = Some(self.items.len() - 1);
            }
        });
        self.focus = None;
        if let Some(i) = add_after {
            let level = self.items[i].0;
            self.items.insert(i + 1, (level, String::new()));
            self.focus = Some(i + 1);
        }
        if let Some(i) = remove {
            self.items.remove(i);
            self.focus = Some(i.saturating_sub(1));
        }
        if let Some((i, d)) = shift {
            let l = &mut self.items[i].0;
            *l = (*l as isize + d).clamp(0, MAX_LEVEL as isize) as usize;
        }
    }
}

// ---------- Math ----------

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum MathKind {
    /// `$…$`, inside a sentence.
    Inline,
    /// `\[…\]`, on a line of its own.
    Display,
    /// `equation`, numbered, with a `\label` to refer to it.
    Numbered,
}

/// The palette under the formula: what is clicked goes in at the cursor;
/// `$|` is where the cursor lands. (glyph shown, LaTeX inserted)
const MATH_GROUPS: &[(&str, &[(&str, &str)])] = &[
    (
        n("Structures"),
        &[
            ("a⁄b", "\\frac{$|}{}"),
            ("√", "\\sqrt{$|}"),
            ("xⁿ", "^{$|}"),
            ("xᵢ", "_{$|}"),
            ("Σ", "\\sum_{$|}^{}"),
            ("∫", "\\int_{$|}^{}"),
            ("lim", "\\lim_{$| \\to }"),
            ("( )", "\\left( $| \\right)"),
            ("[ ]", "\\begin{bmatrix} $| & \\\\  & \\end{bmatrix}"),
            ("{ ", "\\begin{cases} $| & \\text{se } \\\\  & \\text{caso contrário} \\end{cases}"),
            ("x̄", "\\bar{$|}"),
            ("x⃗", "\\vec{$|}"),
        ],
    ),
    (
        n("Greek"),
        &[
            ("α", "\\alpha"), ("β", "\\beta"), ("γ", "\\gamma"), ("δ", "\\delta"), ("ε", "\\varepsilon"), ("θ", "\\theta"),
            ("λ", "\\lambda"), ("μ", "\\mu"), ("π", "\\pi"), ("ρ", "\\rho"), ("σ", "\\sigma"), ("τ", "\\tau"),
            ("φ", "\\varphi"), ("ω", "\\omega"), ("Δ", "\\Delta"), ("Σ", "\\Sigma"), ("Ω", "\\Omega"), ("Φ", "\\Phi"),
        ],
    ),
    (
        n("Operators"),
        &[
            ("±", "\\pm"), ("×", "\\times"), ("·", "\\cdot"), ("÷", "\\div"), ("∞", "\\infty"), ("∂", "\\partial"),
            ("∇", "\\nabla"), ("∏", "\\prod_{$|}^{}"), ("∮", "\\oint"), ("∪", "\\cup"), ("∩", "\\cap"), ("…", "\\dots"),
        ],
    ),
    (
        n("Relations"),
        &[
            ("≤", "\\leq"), ("≥", "\\geq"), ("≠", "\\neq"), ("≈", "\\approx"), ("≡", "\\equiv"), ("∝", "\\propto"),
            ("∈", "\\in"), ("∉", "\\notin"), ("⊂", "\\subset"), ("⊆", "\\subseteq"), ("∀", "\\forall"), ("∃", "\\exists"),
        ],
    ),
    (
        n("Arrows"),
        &[("→", "\\to"), ("←", "\\leftarrow"), ("↔", "\\leftrightarrow"), ("⇒", "\\Rightarrow"), ("⇐", "\\Leftarrow"), ("⇔", "\\Leftrightarrow"), ("↦", "\\mapsto")],
    ),
];

pub struct MathBuilder {
    pub kind: MathKind,
    formula: String,
    label: String,
    /// The formula being edited in the text: its char indices and what was
    /// there, which the result replaces.
    pub replaces: Option<(usize, usize, String)>,
    first: bool,
    preview: LivePreview,
    form_height: f32,
}

impl MathBuilder {
    pub fn new(kind: MathKind, formula: &str, replaces: Option<(usize, usize, String)>) -> Self {
        Self { kind, formula: formula.trim().to_string(), label: String::new(), replaces, first: true, preview: LivePreview::default(), form_height: 0.0 }
    }

    /// The LaTeX for the text.
    pub fn latex(&self) -> String {
        let f = self.formula.trim();
        match self.kind {
            MathKind::Inline => format!("${f}$"),
            MathKind::Display => format!("\\[\n  {f}\n\\]\n"),
            MathKind::Numbered => {
                let label = label_slug(self.label.trim());
                let label = if self.label.trim().is_empty() { String::new() } else { format!("\n  \\label{{eq:{label}}}") };
                format!("\\begin{{equation}}\n  {f}{label}\n\\end{{equation}}\n")
            }
        }
    }

    /// `Some(Some(latex))` to insert, `Some(None)` when canceled.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Option<String>> {
        let mut outcome = None;
        // The preview shows the formula on a line of its own, whatever the
        // kind: a formula in a sentence would be tiny there.
        let body = if self.formula.trim().is_empty() { String::new() } else { format!("\\[ {} \\]", self.formula.trim()) };
        let mut preview = std::mem::take(&mut self.preview);
        let height = self.form_height;
        let mut form_height = height;
        let editing = self.replaces.is_some();
        let empty = self.formula.trim().is_empty();
        let latex = self.latex();
        let title = if editing { t("Edit formula") } else { t("Insert formula") };
        let closed = dialog(
            ctx,
            "math-builder",
            title,
            980.0,
            |ui| {
                form_height = form_and_preview(
                    ui,
                    |ui| self.form_ui(ui),
                    |ui| preview.ui(ui, &body, height),
                );
            },
            |ui| {
                let confirm = if editing { t("Replace") } else { t("Insert formula") };
                if let Some(ok) = footer(ui, t("Type LaTeX, or click the symbols. **Ctrl+Enter** inserts."), confirm) {
                    // Nothing typed: nothing to insert.
                    outcome = Some(ok.then(|| latex.clone()).filter(|_| !empty));
                }
            },
        );
        if !self.formula.trim().is_empty() && ctx.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Enter)) {
            outcome = Some(Some(latex));
        }
        self.form_height = form_height;
        self.preview = preview;
        if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        heading(ui, t("Where"), |_| {});
        segmented(ui, 420.0, &mut self.kind, &[(MathKind::Inline, t("In the sentence")), (MathKind::Display, t("On its own line")), (MathKind::Numbered, t("Numbered"))]);
        ui.add_space(4.0);
        heading(ui, t("Formula"), |_| {});
        let id = egui::Id::new("math-builder-formula");
        let field = ui.scope(|ui| {
            ui.visuals_mut().widgets.inactive.bg_stroke = ui.visuals().widgets.noninteractive.bg_stroke;
            ui.add(
                egui::TextEdit::multiline(&mut self.formula)
                    .id(id)
                    .font(egui::TextStyle::Monospace)
                    .code_editor()
                    .desired_rows(3)
                    .desired_width(f32::INFINITY)
                    .margin(egui::Margin::symmetric(12, 10))
                    .hint_text("E = mc^2"),
            )
        });
        if std::mem::take(&mut self.first) {
            field.inner.request_focus();
        }
        if self.kind == MathKind::Numbered {
            form_row(ui, t("Label"), |ui| {
                ui.label(RichText::new("eq:").monospace().color(ui.visuals().weak_text_color()));
                ui.add(roomy(&mut self.label).hint_text(t("e.g. energy")).desired_width(ui.available_width()));
            });
        }
        ui.add_space(4.0);
        // The symbols, a row per group.
        for (group, symbols) in MATH_GROUPS {
            heading(ui, t(group), |_| {});
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
                for (glyph, latex) in *symbols {
                    let button = egui::Button::new(RichText::new(*glyph).size(15.0)).min_size(egui::vec2(34.0, 30.0));
                    let tip = latex.replace("$|", "…");
                    if ui.add(button).on_hover_text(RichText::new(tip).monospace()).clicked() {
                        insert_at_cursor(ui.ctx(), id, &mut self.formula, latex);
                    }
                }
            });
        }
    }
}

/// Puts `template` at the cursor of the text field `id` (over its
/// selection), a space after a command that a letter would run into, and
/// leaves the cursor at its `$|` (or after it).
fn insert_at_cursor(ctx: &egui::Context, id: egui::Id, text: &mut String, template: &str) {
    let mut state = egui::TextEdit::load_state(ctx, id).unwrap_or_default();
    let n = text.chars().count();
    let (a, b) = state.cursor.char_range().map(|r| {
        let r = r.as_sorted_char_range();
        (r.start.0.min(n), r.end.0.min(n))
    }).unwrap_or((n, n));
    let (inserted, at) = place_template(text, a, b, template);
    *text = inserted;
    state.cursor.set_char_range(Some(egui::text::CCursorRange::one(egui::text::CCursor::new(at))));
    state.store(ctx, id);
    ctx.memory_mut(|m| m.request_focus(id));
}

/// `text` with chars `a..b` replaced by `template`, and where the cursor
/// goes (`$|`, or the end of what went in).
fn place_template(text: &str, a: usize, b: usize, template: &str) -> (String, usize) {
    let mut piece = template.to_string();
    // A command name followed by a letter would read as a longer name.
    let next = text.chars().nth(b);
    if template.ends_with(|c: char| c.is_ascii_alphabetic()) && template.starts_with('\\') && next.is_some_and(|c| c.is_ascii_alphabetic()) {
        piece.push(' ');
    }
    let cursor_in = piece.find("$|").map(|i| piece[..i].chars().count());
    let piece = piece.replace("$|", "");
    let head: String = text.chars().take(a).collect();
    let tail: String = text.chars().skip(b).collect();
    let at = a + cursor_in.unwrap_or(piece.chars().count());
    (format!("{head}{piece}{tail}"), at)
}

// ---------- Image ----------

/// Where a figure floats to.
#[derive(Clone, Copy, PartialEq)]
enum Placement {
    Here,
    Top,
    Auto,
}

impl Placement {
    fn spec(self) -> &'static str {
        match self {
            Placement::Here => "h",
            Placement::Top => "t",
            Placement::Auto => "htbp",
        }
    }
}

/// An image picked in the dialog: one of the project's, or a file from
/// elsewhere, copied into the project when inserted.
#[derive(Clone)]
pub struct PickedImage {
    pub path: std::path::PathBuf,
    /// What `\includegraphics{}` takes, for an image already in the project.
    pub rel_path: Option<String>,
    name: String,
}

/// Picks an image and sets its caption, label, width and placement.
pub struct ImageBuilder {
    /// The project's images, to pick from.
    images: Vec<crate::backend::images::Image>,
    filter: String,
    pub picked: Option<PickedImage>,
    caption: String,
    label: String,
    /// The label follows the image's name until it is edited.
    label_edited: bool,
    /// The section the cursor is in: the label starts with it, so two
    /// "diagrama.png" in different chapters do not collide.
    section: Option<String>,
    /// Width, in percent of the line.
    width: u32,
    placement: Placement,
    float_wrapper: bool,
    focus_caption: bool,
    /// Where an image from outside the project is copied, `/`-separated.
    folder: String,
}

/// Size of an image's tile in the picker.
const TILE: egui::Vec2 = egui::vec2(112.0, 84.0);

impl ImageBuilder {
    pub fn new(images: Vec<crate::backend::images::Image>, section: Option<String>, folder: &std::path::Path) -> Self {
        Self {
            images,
            filter: String::new(),
            picked: None,
            caption: String::new(),
            label: String::new(),
            label_edited: false,
            section,
            width: 80,
            placement: Placement::Auto,
            float_wrapper: true,
            focus_caption: false,
            folder: folder.to_string_lossy().replace('\\', "/"),
        }
    }

    fn pick(&mut self, picked: PickedImage) {
        if !self.label_edited {
            let stem = picked.name.rsplit_once('.').map_or(picked.name.as_str(), |(s, _)| s);
            self.label = match &self.section {
                Some(s) => format!("{}-{}", label_slug(s), label_slug(stem)),
                None => label_slug(stem),
            };
        }
        self.picked = Some(picked);
        self.focus_caption = true;
    }

    /// The figure for the image at `rel_path`.
    pub fn latex(&self, rel_path: &str) -> String {
        let width = if self.width >= 100 { "\\linewidth".to_string() } else { format!("{:.2}\\linewidth", self.width as f32 / 100.0) };
        let graphic = format!("\\includegraphics[width={width}]{{{rel_path}}}");
        if !self.float_wrapper {
            return format!("{graphic}\n");
        }
        let mut out = format!("\\begin{{figure}}[{}]\n  \\centering\n  {graphic}\n", self.placement.spec());
        if !self.caption.trim().is_empty() {
            out.push_str(&format!("  \\caption{{{}}}\n", escape(&self.caption)));
        }
        if !self.label.trim().is_empty() {
            out.push_str(&format!("  \\label{{fig:{}}}\n", self.label.trim()));
        }
        out.push_str("\\end{figure}\n");
        out
    }

    /// `Some(true)` to insert the picked image, `Some(false)` when canceled.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<bool> {
        let mut outcome = None;
        // The preview shows the form as it was at the start of the frame.
        let preview = FigurePreview {
            picked: self.picked.clone(),
            caption: if self.float_wrapper { self.caption.trim().to_string() } else { String::new() },
            width: self.width,
        };
        let ready = self.picked.is_some();
        let text = tf!("Images from outside the project are copied to **{folder}/**. The caption takes LaTeX.", folder = self.folder);
        let closed = dialog(
            ctx,
            "image-builder",
            t("Insert image"),
            960.0,
            |ui| {
                form_and_preview(ui, |ui| self.form_ui(ui), |ui| preview.ui(ui));
            },
            |ui| {
                if let Some(ok) = footer(ui, &text, t("Insert image")) {
                    // Nothing to insert before an image is picked.
                    if !ok || ready {
                        outcome = Some(ok);
                    }
                }
            },
        );
        if closed && outcome.is_none() {
            outcome = Some(false);
        }
        outcome
    }

    fn form_ui(&mut self, ui: &mut egui::Ui) {
        heading(ui, t("Image"), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(icons::button(Icon::FolderOpen, t("From the computer…")).small()).clicked() {
                    let file = rfd::FileDialog::new()
                        .set_title(t("Choose an image"))
                        .add_filter(t("Images"), &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "pdf", "eps"])
                        .pick_file();
                    if let Some(path) = file {
                        // One of the project's, picked through the dialog:
                        // used where it is.
                        let known = self.images.iter().find(|i| std::fs::canonicalize(&i.path).ok() == std::fs::canonicalize(&path).ok());
                        let picked = match known {
                            Some(img) => PickedImage { path: img.path.clone(), rel_path: Some(img.rel_path.clone()), name: img.name.clone() },
                            None => PickedImage { name: crate::backend::project::file_name(&path), path, rel_path: None },
                        };
                        self.pick(picked);
                    }
                }
                if self.images.len() > 6 {
                    ui.add(crate::widgets::field(&mut self.filter).hint_text(t("Filter")).desired_width(140.0));
                }
            });
        });
        self.picker_ui(ui);

        ui.add_space(4.0);
        crate::widgets::checkbox(ui, &mut self.float_wrapper, t("As a figure, with caption and label"));
        if self.float_wrapper {
            form_row(ui, t("Caption"), |ui| {
                let r = ui.add(roomy(&mut self.caption).desired_width(ui.available_width()).hint_text(t("What the image shows")));
                if std::mem::take(&mut self.focus_caption) {
                    r.request_focus();
                }
            });
            form_row(ui, t("Label"), |ui| {
                if label_field(ui, "fig:", &mut self.label, t("my-figure")).changed() {
                    self.label_edited = true;
                }
            });
            form_row(ui, t("Position"), |ui| {
                let options = [(Placement::Here, t("Here")), (Placement::Top, t("Top of the page")), (Placement::Auto, t("Automatic"))];
                segmented(ui, ui.available_width(), &mut self.placement, &options);
            });
        }
        form_row(ui, t("Width"), |ui| {
            ui.spacing_mut().slider_width = (ui.available_width() - 70.0).max(80.0);
            crate::widgets::slider(ui, egui::Slider::new(&mut self.width, 10..=100).step_by(5.0).suffix(" %"));
        });
    }

    /// The project's images as tiles; the one picked framed in the accent.
    fn picker_ui(&mut self, ui: &mut egui::Ui) {
        let visuals = ui.visuals().clone();
        let filter = self.filter.to_lowercase();
        let mut chosen = None;
        // An image from outside comes first: it is the one picked.
        let outside = self.picked.as_ref().filter(|p| p.rel_path.is_none()).cloned();
        let shown: Vec<PickedImage> = outside
            .into_iter()
            .chain(
                self.images
                    .iter()
                    .filter(|i| filter.is_empty() || i.rel_path.to_lowercase().contains(&filter))
                    .map(|i| PickedImage { path: i.path.clone(), rel_path: Some(i.rel_path.clone()), name: i.name.clone() }),
            )
            .collect();
        egui::Frame::new()
            .fill(visuals.extreme_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if shown.is_empty() {
                    ui.add_sized([ui.available_width(), TILE.y], egui::Label::new(RichText::new(t("No image in the project yet. Pick one from the computer.")).color(visuals.weak_text_color())));
                    return;
                }
                egui::ScrollArea::vertical().max_height(TILE.y * 2.0 + 40.0).auto_shrink([false, true]).show(ui, |ui| {
                    ui.horizontal_wrapped(|ui| {
                        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
                        for img in &shown {
                            let picked = self.picked.as_ref().is_some_and(|p| p.path == img.path);
                            let (rect, resp) = ui.allocate_exact_size(TILE + egui::vec2(0.0, 18.0), egui::Sense::click());
                            let thumb = egui::Rect::from_min_size(rect.min, TILE);
                            let hover = ui.ctx().animate_bool_with_time(resp.id, resp.hovered(), crate::theme::MOTION);
                            ui.painter().rect_filled(thumb, 6.0, visuals.faint_bg_color.lerp_to_gamma(visuals.widgets.hovered.weak_bg_fill, hover));
                            thumbnail(ui, &img.path, thumb.shrink(4.0));
                            if picked {
                                ui.painter().rect_stroke(thumb, 6.0, egui::Stroke::new(2.0, crate::theme::accent()), egui::StrokeKind::Inside);
                            }
                            let name = egui::Rect::from_min_max(egui::pos2(rect.left(), thumb.bottom() + 2.0), rect.max);
                            let color = if picked { crate::theme::accent() } else { visuals.weak_text_color() };
                            let galley = ui.painter().layout(img.name.clone(), egui::FontId::proportional(11.0), color, name.width());
                            let galley = if galley.rows.len() > 1 {
                                ui.painter().layout_no_wrap(ellipsize(&img.name, 16), egui::FontId::proportional(11.0), color)
                            } else {
                                galley
                            };
                            ui.painter().galley(egui::pos2(name.center().x - galley.size().x / 2.0, name.top()), galley, color);
                            let tip = img.rel_path.clone().unwrap_or_else(|| img.path.display().to_string());
                            if resp.on_hover_text(tip).on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                                chosen = Some(img.clone());
                            }
                        }
                    });
                });
            });
        if let Some(img) = chosen {
            self.pick(img);
        }
    }

}

/// What the image dialog's preview draws.
struct FigurePreview {
    picked: Option<PickedImage>,
    /// Empty when there is none.
    caption: String,
    width: u32,
}

impl FigurePreview {
    /// The image at its width on a page's line, its caption under it.
    fn ui(&self, ui: &mut egui::Ui) {
        let visuals = ui.visuals().clone();
        heading(ui, t("Preview"), |_| {});
        egui::Frame::new()
            .fill(visuals.extreme_bg_color)
            .stroke(visuals.widgets.noninteractive.bg_stroke)
            .corner_radius(8)
            .inner_margin(egui::Margin::same(16))
            .show(ui, |ui| {
                let (area, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 300.0), egui::Sense::hover());
                let Some(picked) = &self.picked else {
                    let painter = ui.painter();
                    Icon::Image.image(visuals.weak_text_color(), 32.0).paint_at(ui, egui::Rect::from_center_size(area.center() - egui::vec2(0.0, 14.0), egui::vec2(32.0, 32.0)));
                    painter.text(area.center() + egui::vec2(0.0, 18.0), egui::Align2::CENTER_CENTER, t("Pick an image"), egui::FontId::proportional(13.0), visuals.weak_text_color());
                    return;
                };
                // The line's width, faint, and the image across its share.
                let line = area.shrink2(egui::vec2(8.0, 0.0));
                let caption = !self.caption.is_empty();
                let room = egui::Rect::from_min_max(line.min, egui::pos2(line.right(), line.bottom() - if caption { 40.0 } else { 0.0 }));
                let size = ui.ctx().try_load_texture(&uri(&picked.path), egui::TextureOptions::LINEAR, egui::SizeHint::default()).ok().and_then(|t| t.size());
                let width = line.width() * self.width as f32 / 100.0;
                let shown = match size {
                    Some(s) if s.x > 0.0 => {
                        let mut v = egui::vec2(width, width * s.y / s.x);
                        if v.y > room.height() {
                            v *= room.height() / v.y;
                        }
                        v
                    }
                    _ => egui::vec2(width, room.height().min(width * 0.6)),
                };
                let top = room.center().y - shown.y / 2.0;
                let image = egui::Rect::from_center_size(egui::pos2(line.center().x, top + shown.y / 2.0), shown);
                let painter = ui.painter();
                painter.vline(line.left(), line.y_range(), egui::Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.25)));
                painter.vline(line.right(), line.y_range(), egui::Stroke::new(1.0, visuals.weak_text_color().gamma_multiply(0.25)));
                thumbnail(ui, &picked.path, image);
                if caption {
                    let text = tf!("Figure 1: {caption}", caption = self.caption);
                    let galley = ui.painter().layout(text, egui::FontId::proportional(12.0), visuals.text_color(), line.width() - 24.0);
                    ui.painter().galley(egui::pos2(line.center().x - galley.size().x / 2.0, image.bottom() + 10.0), galley, visuals.text_color());
                }
            });
    }
}

fn uri(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

/// The image at `path` fitted inside `rect`; a file icon for what egui
/// cannot draw (PDF, EPS).
fn thumbnail(ui: &egui::Ui, path: &std::path::Path, rect: egui::Rect) {
    let drawable = !matches!(crate::backend::project::ext_lower(path).as_str(), "pdf" | "eps");
    if drawable {
        // paint_at stretches the picture over its rect: fit it first.
        let size = ui.ctx().try_load_texture(&uri(path), egui::TextureOptions::LINEAR, egui::SizeHint::default()).ok().and_then(|t| t.size());
        let fitted = match size {
            Some(s) if s.x > 0.0 && s.y > 0.0 => {
                let scale = (rect.width() / s.x).min(rect.height() / s.y);
                egui::Rect::from_center_size(rect.center(), s * scale)
            }
            _ => rect,
        };
        egui::Image::new(uri(path)).show_loading_spinner(true).paint_at(ui, fitted);
        return;
    }
    let color = ui.visuals().weak_text_color();
    Icon::FileImage.image(color, 28.0).paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(28.0, 28.0)));
}

/// `name` cut to `max` characters, with an ellipsis in the middle.
fn ellipsize(name: &str, max: usize) -> String {
    let chars: Vec<char> = name.chars().collect();
    if chars.len() <= max {
        return name.to_string();
    }
    let half = (max - 1) / 2;
    format!("{}…{}", chars[..half].iter().collect::<String>(), chars[chars.len() - half..].iter().collect::<String>())
}

// ---------- Table ----------

#[derive(Clone, Copy, PartialEq)]
enum Align {
    Left,
    Center,
    Right,
}

impl Align {
    fn spec(self) -> char {
        match self {
            Align::Left => 'l',
            Align::Center => 'c',
            Align::Right => 'r',
        }
    }
}

const PICK_ROWS: usize = 8;
const PICK_COLS: usize = 8;
const MAX_ROWS: usize = 60;
const MAX_COLS: usize = 12;

/// First the size, picked on a grid; then the cells, with the table typeset
/// beside them. Cells can be merged across columns (`\multicolumn`).
pub struct TableBuilder {
    picking: bool,
    rows: usize,
    cols: usize,
    cells: Vec<Vec<String>>,
    /// How many columns the cell starting there takes; 0 where a cell to
    /// its left covers it.
    spans: Vec<Vec<usize>>,
    align: Vec<Align>,
    header_row: bool,
    all_borders: bool,
    float_wrapper: bool,
    caption: String,
    label: String,
    focus: Option<(usize, usize)>,
    /// The cell merges and splits act on: the last one typed in.
    selected: Option<(usize, usize)>,
    preview: LivePreview,
    /// The form's height last frame, which the preview matches.
    form_height: f32,
}

impl Default for TableBuilder {
    fn default() -> Self {
        Self {
            picking: true,
            rows: 3,
            cols: 3,
            cells: Vec::new(),
            spans: Vec::new(),
            align: Vec::new(),
            header_row: true,
            all_borders: true,
            float_wrapper: true,
            caption: String::new(),
            label: String::new(),
            focus: None,
            selected: None,
            preview: LivePreview::default(),
            form_height: 0.0,
        }
    }
}

impl TableBuilder {
    /// Cells, spans and alignments sized to `rows` × `cols`, keeping what was
    /// typed; a merged cell past the last column shrinks.
    fn fit(&mut self) {
        self.cells.resize_with(self.rows, Vec::new);
        self.spans.resize_with(self.rows, Vec::new);
        for (row, spans) in self.cells.iter_mut().zip(&mut self.spans) {
            row.resize_with(self.cols, String::new);
            spans.resize(self.cols, 1);
            let mut c = 0;
            while c < self.cols {
                let n = spans[c].max(1).min(self.cols - c);
                spans[c] = n;
                for k in 1..n {
                    spans[c + k] = 0;
                }
                c += n;
            }
        }
        self.align.resize(self.cols, Align::Left);
        if self.selected.is_some_and(|(r, c)| r >= self.rows || c >= self.cols) {
            self.selected = None;
        }
    }

    /// The column where the cell covering (`r`, `c`) starts.
    fn start_of(&self, r: usize, c: usize) -> usize {
        (0..=c).rev().find(|&s| self.spans[r][s] > 0).unwrap_or(0)
    }

    /// Merges the cell at (`r`, `c`) with the one to its right; their texts
    /// are joined.
    fn merge_right(&mut self, r: usize, c: usize) {
        let c = self.start_of(r, c);
        let next = c + self.spans[r][c];
        if next >= self.cols {
            return;
        }
        let other = std::mem::take(&mut self.cells[r][next]);
        let text = &mut self.cells[r][c];
        if !other.trim().is_empty() {
            if !text.trim().is_empty() {
                text.push(' ');
            }
            text.push_str(other.trim());
        }
        self.spans[r][c] += self.spans[r][next];
        self.spans[r][next] = 0;
        self.fit();
    }

    /// Splits a merged cell back into one cell per column.
    fn split(&mut self, r: usize, c: usize) {
        let c = self.start_of(r, c);
        for k in 0..self.spans[r][c] {
            self.spans[r][c + k] = 1;
        }
    }

    fn tabular(&self, indent: &str) -> String {
        let cells: Vec<String> = self.align.iter().map(|a| a.spec().to_string()).collect();
        let spec = if self.all_borders { format!("|{}|", cells.join("|")) } else { cells.join("") };
        let hline = format!("{indent}  \\hline\n");
        let mut out = format!("{indent}\\begin{{tabular}}{{{spec}}}\n");
        if self.all_borders {
            out.push_str(&hline);
        }
        for r in 0..self.rows {
            let header = self.header_row && r == 0;
            let mut parts = Vec::new();
            let mut c = 0;
            while c < self.cols {
                let n = self.spans[r][c].max(1);
                let mut text = escape(&self.cells[r][c]);
                if header && !text.is_empty() {
                    text = format!("\\textbf{{{text}}}");
                }
                if n > 1 {
                    let a = self.align[c].spec();
                    let spec = match (self.all_borders, c == 0) {
                        (true, true) => format!("|{a}|"),
                        (true, false) => format!("{a}|"),
                        (false, _) => a.to_string(),
                    };
                    text = format!("\\multicolumn{{{n}}}{{{spec}}}{{{text}}}");
                }
                parts.push(text);
                c += n;
            }
            out.push_str(&format!("{indent}  {} \\\\\n", parts.join(" & ")));
            if self.all_borders || (header && self.rows > 1) {
                out.push_str(&hline);
            }
        }
        out.push_str(&format!("{indent}\\end{{tabular}}\n"));
        out
    }

    pub fn latex(&self) -> String {
        if !self.float_wrapper {
            return self.tabular("");
        }
        let label: String = self.label.split_whitespace().collect::<Vec<_>>().join("-");
        let label = if label.is_empty() { String::new() } else { format!("  \\label{{tab:{label}}}\n") };
        format!(
            "\\begin{{table}}[htbp]\n  \\centering\n  \\caption{{{}}}\n{label}{}\\end{{table}}\n",
            escape(&self.caption),
            self.tabular("  ")
        )
    }

    /// `Some(Some(latex))` to insert, `Some(None)` when canceled.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Option<String>> {
        if self.picking {
            return self.pick(ctx);
        }
        let mut outcome = None;
        let latex = self.latex();
        // The preview without the float: a caption needs no page.
        let body = latex.replace("[htbp]", "[h]");
        let mut preview = std::mem::take(&mut self.preview);
        let height = self.form_height;
        let mut form_height = height;
        let closed = dialog(
            ctx,
            "table-builder",
            t("Insert table"),
            1200.0,
            |ui| form_height = form_and_preview(ui, |ui| self.cells_ui(ui), |ui| preview.ui(ui, &body, height)),
            |ui| {
                let text = t("**Enter** goes to the cell below. To span columns, select a cell and use **Merge** (or right-click it). LaTeX such as `\\textbf{…}` and `$x^2$` works; `& % #` are escaped.");
                if let Some(ok) = footer(ui, text, t("Insert table")) {
                    outcome = Some(ok.then(|| latex.clone()));
                }
            },
        );
        self.form_height = form_height;
        self.preview = preview;
        if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }

    /// The size, picked by hovering a grid and clicking.
    fn pick(&mut self, ctx: &egui::Context) -> Option<Option<String>> {
        let mut outcome = None;
        let mut chosen = false;
        let mut next = false;
        let size = self.size_label();
        let closed = dialog(
            ctx,
            "table-size",
            t("Insert table"),
            360.0,
            |ui| {
                ui.horizontal(|ui| {
                    ui.add(egui::Label::new(caps(t("Size"), ui.visuals().weak_text_color())).selectable(false));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(RichText::new(&size).color(ui.visuals().weak_text_color())).selectable(false));
                    });
                });
                let cell = 28.0;
                egui::Grid::new("table-size-grid").spacing([5.0, 5.0]).min_col_width(cell).show(ui, |ui| {
                    for r in 0..PICK_ROWS {
                        for c in 0..PICK_COLS {
                            let on = r < self.rows && c < self.cols;
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(cell, cell), egui::Sense::click());
                            let fill = if on { crate::theme::accent().gamma_multiply(0.35) } else { ui.visuals().extreme_bg_color };
                            let stroke = if on { egui::Stroke::new(1.0, crate::theme::accent().gamma_multiply(0.7)) } else { ui.visuals().widgets.inactive.bg_stroke };
                            ui.painter().rect(rect, 5.0, fill, stroke, egui::StrokeKind::Inside);
                            if resp.hovered() {
                                self.rows = r + 1;
                                self.cols = c + 1;
                            }
                            if resp.clicked() {
                                chosen = true;
                            }
                        }
                        ui.end_row();
                    }
                });
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut self.rows).range(1..=MAX_ROWS).prefix(t("rows: ")));
                    ui.add(egui::DragValue::new(&mut self.cols).range(1..=MAX_COLS).prefix(t("columns: ")));
                });
            },
            |ui| {
                if let Some(ok) = footer(ui, t("Click a square, or set the numbers."), t("Next")) {
                    if ok {
                        next = true;
                    } else {
                        outcome = Some(None);
                    }
                }
            },
        );
        if chosen || next {
            self.picking = false;
            self.fit();
            self.focus = Some((0, 0));
        } else if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }

    /// "4 rows × 5 columns".
    fn size_label(&self) -> String {
        format!("{} × {}", tn("{n} row", "{n} rows", self.rows), tn("{n} column", "{n} columns", self.cols))
    }

    fn cells_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 22.0;
            crate::widgets::checkbox(ui, &mut self.header_row, t("First row as a header"));
            crate::widgets::checkbox(ui, &mut self.all_borders, t("Borders on every cell"));
            crate::widgets::checkbox(ui, &mut self.float_wrapper, t("With caption and label"));
        });
        if self.float_wrapper {
            form_row(ui, t("Caption"), |ui| {
                ui.add(roomy(&mut self.caption).desired_width(ui.available_width()).hint_text(t("What the table shows")));
            });
            form_row(ui, t("Label"), |ui| {
                label_field(ui, "tab:", &mut self.label, t("my-table"));
            });
        }
        ui.add_space(6.0);
        let size = self.size_label();
        heading(ui, t("Cells"), |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add(egui::Label::new(RichText::new(&size).color(ui.visuals().weak_text_color())).selectable(false));
            });
        });

        let gap = 8.0;
        let width = ((ui.available_width() - 2.0 - gap * (self.cols as f32 - 1.0)) / self.cols as f32).max(84.0);
        let focus = self.focus.take();
        let mut merge: Option<(usize, usize)> = None;
        let mut split: Option<(usize, usize)> = None;
        let accent = crate::theme::accent();
        egui::ScrollArea::both().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
            ui.spacing_mut().item_spacing = egui::vec2(gap, gap);
            // Each column's alignment.
            ui.horizontal(|ui| {
                for c in 0..self.cols {
                    align_control(ui, &mut self.align[c], width, c);
                }
            });
            for r in 0..self.rows {
                ui.horizontal(|ui| {
                    let mut c = 0;
                    while c < self.cols {
                        let n = self.spans[r][c].max(1);
                        let w = width * n as f32 + gap * (n as f32 - 1.0);
                        let hint = if self.header_row && r == 0 { t("Heading") } else { "" };
                        let field = ui.add(roomy(&mut self.cells[r][c]).desired_width(w).hint_text(hint));
                        if focus == Some((r, c)) {
                            field.request_focus();
                        }
                        if field.has_focus() {
                            self.selected = Some((r, c));
                        }
                        if self.selected == Some((r, c)) {
                            ui.painter().rect_stroke(field.rect, 5.0, egui::Stroke::new(1.5, accent), egui::StrokeKind::Inside);
                        }
                        // Enter goes down a row, as in a spreadsheet.
                        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && r + 1 < self.rows {
                            self.focus = Some((r + 1, self.start_of(r + 1, c)));
                        }
                        let last = c + n >= self.cols;
                        crate::menu::context(&field, |ui| {
                            if ui.add_enabled(!last, crate::menu::item(t("Merge with the cell on the right"))).clicked() {
                                merge = Some((r, c));
                                ui.close();
                            }
                            if ui.add_enabled(n > 1, crate::menu::item(t("Split cells"))).clicked() {
                                split = Some((r, c));
                                ui.close();
                            }
                        });
                        c += n;
                    }
                });
            }
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().button_padding = egui::vec2(11.0, 6.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            if ui.add_enabled(self.rows < MAX_ROWS, icons::button(Icon::Plus, t("Row"))).clicked() {
                self.rows += 1;
            }
            if ui.add_enabled(self.cols < MAX_COLS, icons::button(Icon::Plus, t("Column"))).clicked() {
                self.cols += 1;
            }
            ui.add(egui::Separator::default().spacing(14.0));
            if ui.add_enabled(self.rows > 1, icons::button(Icon::Minus, t("Last row"))).clicked() {
                self.rows -= 1;
            }
            if ui.add_enabled(self.cols > 1, icons::button(Icon::Minus, t("Last column"))).clicked() {
                self.cols -= 1;
            }
            ui.add(egui::Separator::default().spacing(14.0));
            let sel = self.selected;
            let can_merge = sel.is_some_and(|(r, c)| c + self.spans[r][c].max(1) < self.cols);
            if ui.add_enabled(can_merge, icons::button(Icon::Columns, t("Merge →"))).on_hover_text(t("Merge the selected cell with the one on its right (\\multicolumn)")).clicked() {
                merge = sel;
            }
            let can_split = sel.is_some_and(|(r, c)| self.spans[r][c] > 1);
            if ui.add_enabled(can_split, egui::Button::new(t("Split"))).on_hover_text(t("Split the selected merged cell")).clicked() {
                split = sel;
            }
        });
        if let Some((r, c)) = merge {
            self.merge_right(r, c);
            self.focus = Some((r, self.start_of(r, c)));
        }
        if let Some((r, c)) = split {
            self.split(r, c);
            self.focus = Some((r, c));
        }
        self.fit();
    }
}

/// A column's alignment: three segments in a box, the chosen one lit.
fn align_control(ui: &mut egui::Ui, align: &mut Align, width: f32, column: usize) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 28.0), egui::Sense::hover());
    let visuals = ui.visuals().clone();
    ui.painter().rect(rect, 6.0, visuals.extreme_bg_color, visuals.widgets.inactive.bg_stroke, egui::StrokeKind::Inside);
    let inner = rect.shrink(3.0);
    let segment = inner.width() / 3.0;
    for (k, (value, glyph, tip)) in [(Align::Left, "⇤", t("Left")), (Align::Center, "↔", t("Centered")), (Align::Right, "⇥", t("Right"))].into_iter().enumerate() {
        let r = egui::Rect::from_min_size(inner.min + egui::vec2(segment * k as f32, 0.0), egui::vec2(segment, inner.height()));
        let resp = ui.interact(r, ui.id().with(("align", column, k)), egui::Sense::click()).on_hover_text(tip);
        if resp.clicked() {
            *align = value;
        }
        let on = *align == value;
        if on {
            ui.painter().rect_filled(r, 4.0, crate::theme::accent().gamma_multiply(0.28));
        } else if resp.hovered() {
            ui.painter().rect_filled(r, 4.0, visuals.widgets.hovered.weak_bg_fill);
        }
        let color = if on { crate::theme::accent() } else if resp.hovered() { visuals.text_color() } else { visuals.weak_text_color() };
        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph, egui::FontId::proportional(13.0), color);
    }
}

/// The label's field: `prefix` (`tab:`, `fig:`) fixed in front, the name
/// after it.
fn label_field(ui: &mut egui::Ui, prefix: &str, label: &mut String, hint: &str) -> egui::Response {
    let visuals = ui.visuals().clone();
    let fill = visuals.text_edit_bg_color.unwrap_or(visuals.extreme_bg_color);
    egui::Frame::new().fill(fill).stroke(visuals.widgets.inactive.bg_stroke).corner_radius(visuals.widgets.inactive.corner_radius).show(ui, |ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.horizontal(|ui| {
            let prefix = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)).show(ui, |ui| {
                ui.add(egui::Label::new(RichText::new(prefix).monospace().color(visuals.weak_text_color())).selectable(false));
            });
            let r = prefix.response.rect;
            ui.painter().vline(r.right(), r.y_range(), visuals.widgets.inactive.bg_stroke);
            ui.add(
                egui::TextEdit::singleline(label)
                    // The margin lives on the frame: egui ignores `.margin()` with one.
                    .frame(egui::Frame::NONE.inner_margin(egui::Margin::symmetric(12, 8)))
                    .font(egui::TextStyle::Monospace)
                    .desired_width(ui.available_width())
                    .hint_text(hint),
            )
        })
        .inner
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn math_kinds() {
        let mut m = MathBuilder::new(MathKind::Inline, " E = mc^2 ", None);
        assert_eq!(m.latex(), "$E = mc^2$");
        m.kind = MathKind::Display;
        assert_eq!(m.latex(), "\\[\n  E = mc^2\n\\]\n");
        m.kind = MathKind::Numbered;
        assert_eq!(m.latex(), "\\begin{equation}\n  E = mc^2\n\\end{equation}\n");
        m.label = "Energia de repouso".into();
        assert_eq!(m.latex(), "\\begin{equation}\n  E = mc^2\n  \\label{eq:energia-de-repouso}\n\\end{equation}\n");
    }

    #[test]
    fn symbols_go_in_at_the_cursor() {
        // Over the selection, the cursor inside the first braces.
        assert_eq!(place_template("a + b", 4, 5, "\\frac{$|}{}"), ("a + \\frac{}{}".into(), 10));
        // A command before a letter gets a space.
        assert_eq!(place_template("x", 0, 0, "\\alpha"), ("\\alpha x".into(), 7));
        assert_eq!(place_template("2", 1, 1, "\\pi"), ("2\\pi".into(), 4));
    }

    #[test]
    fn escapes_what_breaks_the_markup() {
        assert_eq!(escape(" 50% & 3# "), "50\\% \\& 3\\#");
        assert_eq!(escape("já \\% escapado"), "já \\% escapado");
        assert_eq!(escape("\\textbf{x} e $x^2$"), "\\textbf{x} e $x^2$");
    }

    #[test]
    fn label_slugs() {
        assert_eq!(label_slug("Introdução ao Método"), "introducao-ao-metodo");
        assert_eq!(label_slug("!!"), "fig");
    }

    #[test]
    fn figures() {
        let mut b = ImageBuilder::new(vec![], Some("Resultados".into()), std::path::Path::new("images"));
        b.pick(PickedImage { path: "/x/images/gráfico_1.png".into(), rel_path: Some("images/gráfico_1.png".into()), name: "gráfico_1.png".into() });
        b.caption = "Erro de 5% por grupo".into();
        assert_eq!(
            b.latex("images/gráfico_1.png"),
            "\\begin{figure}[htbp]\n  \\centering\n  \\includegraphics[width=0.80\\linewidth]{images/gráfico_1.png}\n  \\caption{Erro de 5\\% por grupo}\n  \\label{fig:resultados-grafico-1}\n\\end{figure}\n"
        );
        b.width = 100;
        b.float_wrapper = false;
        assert_eq!(b.latex("a.png"), "\\includegraphics[width=\\linewidth]{a.png}\n");
    }

    #[test]
    fn nested_lists() {
        let mut b = ListBuilder::default();
        b.items = vec![(0, "Um".into()), (1, "Um.a".into()), (2, "Fundo".into()), (0, "Dois".into()), (0, "".into())];
        assert_eq!(
            b.latex(),
            "\\begin{itemize}\n  \\item Um\n  \\begin{itemize}\n    \\item Um.a\n    \\begin{itemize}\n      \\item Fundo\n    \\end{itemize}\n  \\end{itemize}\n  \\item Dois\n\\end{itemize}\n"
        );
        b.kind = ListKind::Numbered;
        b.items = vec![(2, "Só".into())];
        assert_eq!(b.latex(), "\\begin{enumerate}\n  \\item Só\n\\end{enumerate}\n", "the first item cannot be nested");
    }

    #[test]
    fn merged_cells() {
        let mut b = TableBuilder { rows: 2, cols: 3, float_wrapper: false, header_row: false, ..Default::default() };
        b.fit();
        b.cells = vec![vec!["Grupo".into(), "x".into(), "".into()], vec!["a".into(), "b".into(), "c".into()]];
        b.merge_right(0, 0);
        b.merge_right(0, 0);
        assert_eq!(b.spans[0], vec![3, 0, 0]);
        assert!(b.latex().contains("  \\multicolumn{3}{|l|}{Grupo x} \\\\\n"), "{}", b.latex());
        b.split(0, 2);
        assert_eq!(b.spans[0], vec![1, 1, 1]);
        // A merge in the middle of a row, borders off.
        b.all_borders = false;
        b.align[1] = Align::Center;
        b.merge_right(1, 1);
        assert!(b.latex().contains("  a & \\multicolumn{2}{c}{b c} \\\\\n"), "{}", b.latex());
        // Fewer columns: the merged cell shrinks with them.
        b.cols = 2;
        b.fit();
        assert_eq!(b.spans[1], vec![1, 1]);
    }

    #[test]
    fn filled_tables() {
        let mut b = TableBuilder { rows: 2, cols: 2, float_wrapper: false, ..Default::default() };
        b.fit();
        b.cells = vec![vec!["Nome".into(), "Valor".into()], vec!["a & b".into(), "10%".into()]];
        b.align[1] = Align::Right;
        assert_eq!(
            b.latex(),
            "\\begin{tabular}{|l|r|}\n  \\hline\n  \\textbf{Nome} & \\textbf{Valor} \\\\\n  \\hline\n  a \\& b & 10\\% \\\\\n  \\hline\n\\end{tabular}\n"
        );
        b.float_wrapper = true;
        b.caption = "Resultados".into();
        b.label = "meus resultados".into();
        let t = b.latex();
        assert!(t.starts_with("\\begin{table}[htbp]\n  \\centering\n  \\caption{Resultados}\n  \\label{tab:meus-resultados}\n  \\begin{tabular}{|l|r|}"), "{t}");
    }
}
