//! Modal dialogs. Each one is a small state struct whose `show` returns
//! `None` while it stays open and `Some(outcome)` once the user decides.

use crate::backend::project::relative;
use crate::backend::references::ManualFields;
use crate::backend::search::Match;
use crate::i18n::{n, t, tn};
use crate::settings::Snippet;
use crate::tf;
use egui::{Key, Modal, RichText};
use std::path::{Path, PathBuf};

fn modal(ctx: &egui::Context, id: &str, width: f32, content: impl FnOnce(&mut egui::Ui)) -> bool {
    let r = Modal::new(egui::Id::new(id)).show(ctx, |ui| {
        ui.set_width(width);
        crate::theme::framed(ui);
        content(ui);
    });
    r.should_close()
}

fn danger_button(ui: &mut egui::Ui, label: &str, danger: bool) -> egui::Response {
    if danger {
        ui.add(egui::Button::new(RichText::new(label).strong().color(egui::Color32::WHITE)).fill(egui::Color32::from_rgb(0xB4, 0x3A, 0x2A)).min_size(egui::vec2(96.0, 32.0)))
    } else {
        ui.add(crate::widgets::primary(label))
    }
}

// ---------- Prompt ----------

pub struct Prompt<A> {
    pub title: String,
    pub message: String,
    pub value: String,
    pub confirm: String,
    pub action: A,
    /// Char range to select when the field first shows (e.g. the name without
    /// its extension).
    pub select: Option<(usize, usize)>,
    first: bool,
}

impl<A> Prompt<A> {
    pub fn new(title: &str, value: &str, confirm: &str, action: A) -> Self {
        Self { title: title.into(), message: String::new(), value: value.into(), confirm: confirm.into(), action, select: None, first: true }
    }

    pub fn message(mut self, m: &str) -> Self {
        self.message = m.into();
        self
    }

    pub fn select(mut self, a: usize, b: usize) -> Self {
        self.select = Some((a, b));
        self
    }

    /// `Some(Some(value))` confirmed, `Some(None)` canceled.
    pub fn show(&mut self, ctx: &egui::Context) -> Option<Option<String>> {
        let mut outcome = None;
        let closed = modal(ctx, "prompt", 420.0, |ui| {
            ui.heading(&self.title);
            if !self.message.is_empty() {
                ui.label(&self.message);
            }
            ui.add_space(6.0);
            let id = egui::Id::new("prompt-field");
            let r = ui.add(crate::widgets::field(&mut self.value).id(id).desired_width(f32::INFINITY));
            if self.first {
                self.first = false;
                r.request_focus();
                let (a, b) = self.select.unwrap_or((0, self.value.chars().count()));
                if let Some(mut state) = egui::TextEdit::load_state(ctx, id) {
                    state.cursor.set_char_range(Some(egui::text::CCursorRange::two(
                        egui::text::CCursor::new(a),
                        egui::text::CCursor::new(b),
                    )));
                    state.store(ctx, id);
                }
            }
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(crate::widgets::primary(&self.confirm)).clicked() || enter {
                    outcome = Some(Some(self.value.clone()));
                }
                if ui.add(crate::widgets::secondary(t("Cancel"))).clicked() {
                    outcome = Some(None);
                }
            });
        });
        if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }
}

// ---------- Confirm ----------

pub struct Confirm<A> {
    pub title: String,
    pub message: String,
    pub confirm: String,
    pub cancel: String,
    pub danger: bool,
    pub action: A,
}

impl<A> Confirm<A> {
    pub fn new(title: &str, message: &str, confirm: &str, action: A) -> Self {
        Self { title: title.into(), message: message.into(), confirm: confirm.into(), cancel: t("Cancel").into(), danger: true, action }
    }

    pub fn cancel_label(mut self, label: &str) -> Self {
        self.cancel = label.into();
        self
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<bool> {
        let mut outcome = None;
        let closed = modal(ctx, "confirm", 440.0, |ui| {
            ui.heading(&self.title);
            ui.add_space(4.0);
            ui.label(&self.message);
            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if danger_button(ui, &self.confirm, self.danger).clicked() {
                    outcome = Some(true);
                }
                if ui.add(crate::widgets::secondary(&self.cancel)).clicked() {
                    outcome = Some(false);
                }
            });
        });
        if closed && outcome.is_none() {
            outcome = Some(false);
        }
        outcome
    }
}

// ---------- New reference ----------

pub const BIB_KINDS: &[(&str, &str)] = &[
    ("article", n("Journal article")),
    ("book", n("Book")),
    ("inproceedings", n("Conference paper")),
    ("phdthesis", n("Thesis")),
    ("mastersthesis", n("Dissertation")),
    ("techreport", n("Technical report")),
    ("online", n("Website")),
    ("misc", n("Other")),
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ReferenceSource {
    /// Looked up by its DOI, ISBN or arXiv ID.
    #[default]
    Identifier,
    /// Typed in by hand.
    Manual,
}

pub enum NewReference {
    Import(String),
    Manual(ManualFields),
}

/// One dialog for both ways of adding a reference, switched at the top.
#[derive(Default)]
pub struct AddReference {
    pub source: ReferenceSource,
    identifier: String,
    fields: ManualFields,
    focused: Option<ReferenceSource>,
}

impl AddReference {
    pub fn new(source: ReferenceSource) -> Self {
        Self { source, fields: ManualFields { kind: "article".into(), ..Default::default() }, ..Default::default() }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<Option<NewReference>> {
        use crate::backend::references::{detect_identifier, IdKind};
        let mut outcome = None;
        let closed = modal(ctx, "add-reference", 440.0, |ui| {
            ui.heading(t("New reference"));
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.source, ReferenceSource::Identifier, t("DOI, ISBN or arXiv"));
                ui.selectable_value(&mut self.source, ReferenceSource::Manual, t("Fill in by hand"));
            });
            ui.add_space(10.0);
            let focus = self.focused != Some(self.source);
            self.focused = Some(self.source);
            let label = |ui: &mut egui::Ui, text: &str| {
                ui.label(RichText::new(text).color(ui.visuals().weak_text_color()));
            };
            let ready = match self.source {
                ReferenceSource::Identifier => {
                    let r = ui.add(crate::widgets::field(&mut self.identifier).hint_text("10.1000/xyz123").desired_width(f32::INFINITY));
                    if focus {
                        r.request_focus();
                    }
                    let (kind, _) = detect_identifier(&self.identifier);
                    let found = match kind {
                        IdKind::Doi => Some("DOI"),
                        IdKind::Isbn => Some("ISBN"),
                        IdKind::Arxiv => Some("arXiv"),
                        IdKind::Unknown => None,
                    };
                    ui.add_space(2.0);
                    match found {
                        Some(name) => ui.weak(tf!("{name} recognized. Title, authors and year come from the internet.", name)),
                        None if self.identifier.trim().is_empty() => ui.weak(t("The rest is filled in from the internet.")),
                        None => ui.label(RichText::new(t("Not a DOI, ISBN or arXiv ID.")).color(ui.visuals().warn_fg_color)),
                    };
                    if found.is_some() && r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                        outcome = Some(Some(NewReference::Import(self.identifier.clone())));
                    }
                    found.is_some()
                }
                ReferenceSource::Manual => {
                    label(ui, t("Type"));
                    egui::ComboBox::from_id_salt("bib-kind")
                        .width(ui.available_width())
                        .selected_text(BIB_KINDS.iter().find(|(k, _)| *k == self.fields.kind).map(|(_, l)| t(l)).unwrap_or(&self.fields.kind))
                        .show_ui(ui, |ui| {
                            for (k, l) in BIB_KINDS {
                                ui.selectable_value(&mut self.fields.kind, k.to_string(), t(l));
                            }
                        });
                    ui.add_space(4.0);
                    label(ui, t("Title"));
                    let r = ui.add(crate::widgets::field(&mut self.fields.title).desired_width(f32::INFINITY));
                    if focus {
                        r.request_focus();
                    }
                    ui.add_space(4.0);
                    label(ui, t("Authors"));
                    ui.add(crate::widgets::field(&mut self.fields.author).hint_text(t("Surname, Name and Surname, Name")).desired_width(f32::INFINITY));
                    ui.add_space(4.0);
                    ui.columns(2, |cols| {
                        label(&mut cols[0], t("Year"));
                        cols[0].add(crate::widgets::field(&mut self.fields.year).desired_width(f32::INFINITY));
                        label(&mut cols[1], t("DOI (optional)"));
                        cols[1].add(crate::widgets::field(&mut self.fields.doi).desired_width(f32::INFINITY));
                    });
                    !self.fields.title.trim().is_empty()
                }
            };
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let confirm = match self.source {
                    ReferenceSource::Identifier => t("Look up and add"),
                    ReferenceSource::Manual => t("Add"),
                };
                if ui.add_enabled(ready, crate::widgets::primary(confirm)).clicked() {
                    outcome = Some(Some(match self.source {
                        ReferenceSource::Identifier => NewReference::Import(self.identifier.clone()),
                        ReferenceSource::Manual => NewReference::Manual(self.fields.clone()),
                    }));
                }
                if ui.add(crate::widgets::secondary(t("Cancel"))).clicked() {
                    outcome = Some(None);
                }
            });
        });
        if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }
}

// ---------- Paper metadata ----------

pub struct PaperMeta {
    pub name: String,
    pub path: std::path::PathBuf,
    pub cite_key: String,
    pub summary: String,
}

impl PaperMeta {
    pub fn show(&mut self, ctx: &egui::Context, keys: &[String]) -> Option<Option<(String, String)>> {
        let mut outcome = None;
        let closed = modal(ctx, "paper-meta", 460.0, |ui| {
            ui.heading(format!("Paper: {}", self.name));
            ui.label(t(".bib key"));
            egui::ComboBox::from_id_salt("paper-key")
                .selected_text(if self.cite_key.is_empty() { t("(none)") } else { &self.cite_key })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.cite_key, String::new(), t("(none)"));
                    for k in keys {
                        ui.selectable_value(&mut self.cite_key, k.clone(), k);
                    }
                });
            ui.label(t("Summary"));
            ui.add(crate::widgets::area(&mut self.summary).desired_rows(5).desired_width(f32::INFINITY));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add(crate::widgets::primary(t("Save"))).clicked() {
                    outcome = Some(Some((self.cite_key.clone(), self.summary.clone())));
                }
                if ui.add(crate::widgets::secondary(t("Cancel"))).clicked() {
                    outcome = Some(None);
                }
            });
        });
        if closed && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }
}

// ---------- Snippets ----------

#[derive(Default)]
pub struct SnippetManager {
    editing: Option<(Option<u64>, String, String, String)>,
    error: Option<String>,
}

impl SnippetManager {
    /// Edits `snippets` in place; returns (still open, changed).
    pub fn show(&mut self, ctx: &egui::Context, snippets: &mut Vec<Snippet>, builtin: &[String]) -> (bool, bool) {
        let mut open = true;
        let mut changed = false;
        let closed = modal(ctx, "snippets", 560.0, |ui| {
            ui.heading(t("Snippets"));
            ui.weak(t("A snippet shows in the autocomplete when you type its trigger (starting with \\). Use | to mark where the cursor lands."));
            ui.add_space(6.0);
            egui::ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
                if snippets.is_empty() {
                    ui.weak(t("No snippet yet."));
                }
                let mut remove = None;
                for s in snippets.iter() {
                    ui.horizontal(|ui| {
                        ui.monospace(&s.trigger);
                        ui.weak(&s.description);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(t("Delete")).clicked() {
                                remove = Some(s.id);
                            }
                            if ui.small_button(t("Edit")).clicked() {
                                self.editing = Some((Some(s.id), s.trigger.clone(), s.body.clone(), s.description.clone()));
                                self.error = None;
                            }
                        });
                    });
                }
                if let Some(id) = remove {
                    snippets.retain(|s| s.id != id);
                    changed = true;
                }
            });
            ui.separator();
            match &mut self.editing {
                None => {
                    if ui.button(t("New snippet")).clicked() {
                        self.editing = Some((None, "\\".into(), String::new(), String::new()));
                        self.error = None;
                    }
                }
                Some((id, trigger, body, desc)) => {
                    egui::Grid::new("snippet-edit").num_columns(2).show(ui, |ui| {
                        ui.label(t("Trigger"));
                        ui.add(crate::widgets::field(trigger));
                        ui.end_row();
                        ui.label(t("Description"));
                        ui.add(crate::widgets::field(desc));
                        ui.end_row();
                        ui.label(t("Body"));
                        ui.add(crate::widgets::area(body).code_editor().desired_rows(4).desired_width(380.0));
                        ui.end_row();
                    });
                    if let Some(e) = &self.error {
                        ui.colored_label(ui.visuals().error_fg_color, e);
                    }
                    let mut done = false;
                    ui.horizontal(|ui| {
                        if ui.button(RichText::new(t("Save snippet")).strong()).clicked() {
                            let t = trigger.trim().to_string();
                            let valid = t.len() > 1 && t.starts_with('\\') && t[1..].chars().all(|c| c.is_ascii_alphabetic());
                            if !valid {
                                self.error = Some(crate::i18n::t("The trigger must be \\ followed only by letters.").into());
                            } else if builtin.contains(&t) {
                                self.error = Some(crate::i18n::t("That trigger is already a built-in command.").into());
                            } else if snippets.iter().any(|s| s.trigger == t && Some(s.id) != *id) {
                                self.error = Some(crate::i18n::t("A snippet with that trigger already exists.").into());
                            } else if body.is_empty() {
                                self.error = Some(crate::i18n::t("The body cannot be empty.").into());
                            } else {
                                match id.and_then(|id| snippets.iter_mut().find(|s| s.id == id)) {
                                    Some(s) => {
                                        s.trigger = t;
                                        s.body = body.clone();
                                        s.description = desc.trim().into();
                                    }
                                    None => {
                                        let next = snippets.iter().map(|s| s.id).max().unwrap_or(0) + 1;
                                        snippets.push(Snippet { id: next, trigger: t, body: body.clone(), description: desc.trim().into() });
                                    }
                                }
                                changed = true;
                                done = true;
                            }
                        }
                        if ui.add(crate::widgets::secondary(t("Cancel"))).clicked() {
                            done = true;
                        }
                    });
                    if done {
                        self.editing = None;
                    }
                }
            }
            ui.add_space(6.0);
            if ui.add(crate::widgets::secondary(t("Close"))).clicked() {
                open = false;
            }
        });
        (open && !closed, changed)
    }
}

// ---------- Shortcuts ----------

pub fn shortcuts(ctx: &egui::Context, groups: &[(&str, Vec<(String, String)>)]) -> bool {
    let mut open = true;
    let closed = modal(ctx, "shortcuts", 480.0, |ui| {
        ui.heading(t("Keyboard shortcuts"));
        egui::ScrollArea::vertical().max_height(480.0).show(ui, |ui| {
            let small = egui::TextStyle::Small.resolve(ui.style());
            for (header, items) in groups {
                ui.add_space(8.0);
                ui.label(egui::RichText::new(header.to_uppercase()).small().strong().weak());
                ui.add_space(2.0);
                for (label, accel) in items {
                    // Label on the left; each key of the shortcut as a chip,
                    // right-aligned so every group lines up.
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width() - 8.0, 24.0), egui::Sense::hover());
                    ui.painter().text(rect.left_center() + egui::vec2(4.0, 0.0), egui::Align2::LEFT_CENTER, label, egui::TextStyle::Body.resolve(ui.style()), ui.visuals().text_color());
                    let mut x = rect.right() - 4.0;
                    for key in accel.rsplit(' ') {
                        let galley = ui.fonts_mut(|f| f.layout_no_wrap(key.to_string(), small.clone(), ui.visuals().text_color()));
                        let w = galley.size().x + 10.0;
                        let chip = egui::Rect::from_min_size(egui::pos2(x - w, rect.center().y - 9.0), egui::vec2(w, 18.0));
                        if key == "/" {
                            ui.painter().galley(chip.center() - galley.size() / 2.0, galley, ui.visuals().weak_text_color());
                        } else {
                            ui.painter().rect_filled(chip, 4.0, ui.visuals().widgets.inactive.bg_fill);
                            ui.painter().rect_stroke(chip, 4.0, egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color), egui::StrokeKind::Inside);
                            ui.painter().galley(chip.center() - galley.size() / 2.0, galley, ui.visuals().text_color());
                        }
                        x -= w + 4.0;
                    }
                }
            }
        });
        if ui.add(crate::widgets::secondary(t("Close"))).clicked() {
            open = false;
        }
    });
    open && !closed
}

// ---------- Command palette ----------

pub struct PaletteItem<C> {
    pub label: String,
    /// Where it belongs: the heading over it, and shown beside it in results.
    pub group: String,
    pub icon: crate::icons::Icon,
    pub accel: Option<String>,
    /// For a setting that is on or off.
    pub checked: Option<bool>,
    pub cmd: C,
    /// Remembered among the recent commands under this key.
    pub id: Option<String>,
    /// Too many to list (sections, references): found by searching only.
    pub searched_only: bool,
    /// Its matches are listed apart, under the group's heading (the
    /// project's files), not mixed with the commands.
    pub sectioned: bool,
    /// Added to its search score: what was used lately ranks higher.
    pub boost: i32,
}

enum PaletteRow {
    Header(String),
    /// An item, with the chars of its label the search matched.
    Item(usize, Vec<usize>),
}

/// How many recent commands head the list.
pub const PALETTE_RECENT: usize = 5;

#[derive(Default)]
pub struct CommandPalette {
    query: String,
    selected: usize,
    first: bool,
    /// Lists the references to cite instead of the commands.
    pub citations: bool,
}

impl CommandPalette {
    pub fn new() -> Self {
        Self { first: true, ..Default::default() }
    }

    pub fn citations() -> Self {
        Self { first: true, citations: true, ..Default::default() }
    }

    /// The rows for the query: with nothing typed, the recent commands and
    /// then every command under its group's heading; else the matches, best
    /// first.
    fn rows<C>(&self, items: &[PaletteItem<C>], recent: &[String]) -> Vec<PaletteRow> {
        let recent_rank = |i: &PaletteItem<C>| i.id.as_ref().and_then(|id| recent.iter().position(|r| r == id));
        if self.query.trim().is_empty() {
            let mut rows = Vec::new();
            let mut shown = vec![false; items.len()];
            if !self.citations {
                let mut recents: Vec<(usize, usize)> = items.iter().enumerate().filter_map(|(i, it)| recent_rank(it).map(|r| (r, i))).collect();
                recents.sort();
                if !recents.is_empty() {
                    rows.push(PaletteRow::Header(t("Recent").into()));
                }
                for (_, i) in recents.into_iter().take(PALETTE_RECENT) {
                    shown[i] = true;
                    rows.push(PaletteRow::Item(i, vec![]));
                }
            }
            let mut groups: Vec<&str> = Vec::new();
            for it in items.iter().filter(|it| !it.searched_only) {
                if !groups.contains(&it.group.as_str()) {
                    groups.push(&it.group);
                }
            }
            for g in groups {
                let members: Vec<usize> = (0..items.len()).filter(|&i| !shown[i] && !items[i].searched_only && items[i].group == g).collect();
                if members.is_empty() {
                    continue;
                }
                if !self.citations {
                    rows.push(PaletteRow::Header(g.to_string()));
                }
                rows.extend(members.into_iter().map(|i| PaletteRow::Item(i, vec![])));
            }
            return rows;
        }
        let mut found: Vec<(i32, usize, Vec<usize>)> = items
            .iter()
            .enumerate()
            .filter_map(|(i, it)| {
                let extra = format!("{} {}", it.group, it.accel.as_deref().unwrap_or(""));
                let (score, hits) = crate::fuzzy::score(&self.query, &it.label, &extra)?;
                // What was run lately comes first among equals.
                let bonus = recent_rank(it).map(|r| (PALETTE_RECENT as i32 - r as i32).max(1) * 40).unwrap_or(0);
                Some((score + bonus + it.boost, i, hits))
            })
            .collect();
        found.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        let (apart, mixed): (Vec<_>, Vec<_>) = found.into_iter().partition(|(_, i, _)| items[*i].sectioned);
        if apart.is_empty() {
            return mixed.into_iter().take(80).map(|(_, i, hits)| PaletteRow::Item(i, hits)).collect();
        }
        // Each section under its heading, the commands under theirs; the
        // block with the best match first.
        let mut blocks: Vec<(i32, String, Vec<(i32, usize, Vec<usize>)>)> = Vec::new();
        for f in apart {
            let group = &items[f.1].group;
            match blocks.iter_mut().find(|b| &b.1 == group) {
                Some(b) => b.2.push(f),
                None => blocks.push((f.0, group.clone(), vec![f])),
            }
        }
        if !mixed.is_empty() {
            blocks.push((mixed[0].0, t("Commands").into(), mixed));
        }
        blocks.sort_by(|a, b| b.0.cmp(&a.0));
        let mut rows = Vec::new();
        for (_, title, members) in blocks {
            rows.push(PaletteRow::Header(title));
            rows.extend(members.into_iter().take(50).map(|(_, i, hits)| PaletteRow::Item(i, hits)));
        }
        rows
    }

    pub fn show<C: Clone>(&mut self, ctx: &egui::Context, items: &[PaletteItem<C>], recent: &[String]) -> Option<Option<C>> {
        let rows = self.rows(items, recent);
        let picks: Vec<usize> = rows.iter().filter_map(|r| if let PaletteRow::Item(i, _) = r { Some(*i) } else { None }).collect();
        if self.selected >= picks.len() {
            self.selected = picks.len().saturating_sub(1);
        }
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, Key::Enter),
            )
        });
        if down && !picks.is_empty() {
            self.selected = (self.selected + 1) % picks.len();
        }
        if up && !picks.is_empty() {
            self.selected = (self.selected + picks.len() - 1) % picks.len();
        }
        let mut outcome = None;
        if enter {
            outcome = Some(picks.get(self.selected).map(|&i| items[i].cmd.clone()));
        }
        let searching = !self.query.trim().is_empty();
        let modal = Modal::new(egui::Id::new("palette")).area(
            Modal::default_area(egui::Id::new("palette")).anchor(egui::Align2::CENTER_TOP, egui::vec2(0.0, 80.0)),
        );
        let r = modal.show(ctx, |ui| {
            ui.set_width(560.0);
            let hint = if self.citations { t("Cite: search by title, author or key…") } else { t("Search commands, files, sections, references…") };
            // As wide as the rows below, which leave room for the scroll bar.
            let field = crate::widgets::field(&mut self.query).hint_text(hint).margin(egui::Margin::symmetric(10, 8)).desired_width(ui.available_width() - 10.0);
            let r = ui.add(field);
            ui.add_space(6.0);
            if self.first {
                self.first = false;
                r.request_focus();
            }
            if r.changed() {
                self.selected = 0;
            }
            let body = egui::TextStyle::Body.resolve(ui.style());
            let small = egui::TextStyle::Small.resolve(ui.style());
            let (text, weak, strong) = (ui.visuals().text_color(), ui.visuals().weak_text_color(), ui.visuals().strong_text_color());
            egui::ScrollArea::vertical().max_height(400.0).show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                let origin = ui.cursor().top();
                // The selection glides from row to row; a new query puts it
                // in place at once.
                let highlight = ui.painter().add(egui::Shape::Noop);
                let mut selected_rect = None;
                let mut pick = 0;
                for row in &rows {
                    let width = ui.available_width() - 10.0;
                    match row {
                        PaletteRow::Header(title) => {
                            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 26.0), egui::Sense::hover());
                            ui.painter().text(egui::pos2(rect.left() + 10.0, rect.bottom() - 6.0), egui::Align2::LEFT_BOTTOM, title, small.clone(), weak);
                        }
                        PaletteRow::Item(i, hits) => {
                            let item = &items[*i];
                            let (rect, resp) = ui.allocate_exact_size(egui::vec2(width, 32.0), egui::Sense::click());
                            let selected = pick == self.selected;
                            if selected {
                                selected_rect = Some(rect);
                                if up || down {
                                    resp.scroll_to_me(None);
                                }
                            } else if resp.hovered() {
                                ui.painter().rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
                            }
                            let icon_rect = egui::Rect::from_center_size(egui::pos2(rect.left() + 20.0, rect.center().y), egui::vec2(16.0, 16.0));
                            item.icon.image(if selected { strong } else { weak }, crate::icons::SIZE).paint_at(ui, icon_rect);
                            // The label, what the search matched in it brighter.
                            let mut job = egui::text::LayoutJob::default();
                            for (k, c) in item.label.chars().enumerate() {
                                let hit = hits.binary_search(&k).is_ok();
                                let format = egui::TextFormat {
                                    font_id: body.clone(),
                                    color: if hit { crate::theme::accent() } else { text },
                                    valign: egui::Align::Center,
                                    ..Default::default()
                                };
                                job.append(&c.to_string(), 0.0, format);
                            }
                            // Right to left: the shortcut, the on/off mark, the group.
                            let mut right = rect.right() - 10.0;
                            if let Some(a) = &item.accel {
                                right = keys(ui, a, right, rect.center().y, &small) - 8.0;
                            }
                            if item.checked == Some(true) {
                                let check = egui::Rect::from_center_size(egui::pos2(right - 8.0, rect.center().y), egui::vec2(16.0, 16.0));
                                crate::icons::show_at(ui, crate::icons::Icon::Check, check, crate::theme::accent());
                                right -= 24.0;
                            }
                            if searching && !self.citations && !item.sectioned {
                                let g = ui.painter().text(egui::pos2(right, rect.center().y), egui::Align2::RIGHT_CENTER, &item.group, small.clone(), weak);
                                right = g.left() - 12.0;
                            }
                            job.wrap = egui::text::TextWrapping::truncate_at_width((right - rect.left() - 40.0).max(40.0));
                            let galley = ui.fonts_mut(|f| f.layout_job(job));
                            ui.painter().galley(egui::pos2(rect.left() + 38.0, rect.center().y - galley.size().y / 2.0), galley, text);
                            if resp.clicked() {
                                outcome = Some(Some(item.cmd.clone()));
                            }
                            pick += 1;
                        }
                    }
                }
                if let Some(rect) = selected_rect {
                    let glide = ui.ctx().animate_value_with_time(egui::Id::new(("palette-selection", &self.query)), rect.top() - origin, crate::theme::MOTION);
                    let at = egui::Rect::from_min_size(egui::pos2(rect.left(), origin + glide), rect.size());
                    ui.painter().set(highlight, egui::Shape::rect_filled(at, 5.0, ui.visuals().selection.bg_fill));
                }
                if picks.is_empty() {
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        ui.add_space(10.0);
                        ui.weak(if self.citations { t("No reference found.") } else { t("Nothing found. Try fewer letters.") });
                    });
                    ui.add_space(10.0);
                }
            });
            // How to drive it from the keyboard.
            ui.add_space(6.0);
            ui.separator();
            ui.horizontal(|ui| {
                ui.add_space(4.0);
                ui.spacing_mut().item_spacing.x = 14.0;
                for (k, what) in [("↑↓", t("to move")), ("↵", t("to run")), ("Esc", t("to close"))] {
                    ui.label(RichText::new(format!("{k}  {what}")).small().weak());
                }
            });
        });
        if r.should_close() && outcome.is_none() {
            outcome = Some(None);
        }
        outcome
    }
}

/// A shortcut ("Ctrl+Shift+P") as keycaps, right-aligned at `right`.
/// Returns where they begin.
fn keys(ui: &egui::Ui, accel: &str, right: f32, y: f32, font: &egui::FontId) -> f32 {
    let mut x = right;
    let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
    for key in accel.split('+').rev() {
        let galley = ui.painter().layout_no_wrap(key.to_string(), font.clone(), ui.visuals().weak_text_color());
        let size = egui::vec2((galley.size().x + 10.0).max(20.0), 20.0);
        let rect = egui::Rect::from_min_size(egui::pos2(x - size.x, y - size.y / 2.0), size);
        ui.painter().rect(rect, 4.0, ui.visuals().extreme_bg_color, stroke, egui::StrokeKind::Inside);
        ui.painter().galley(rect.center() - galley.size() / 2.0, galley, ui.visuals().weak_text_color());
        x = rect.left() - 3.0;
    }
    x
}

// ---------- Project search ----------

pub enum SearchAction {
    Search(String),
    GoTo(Match),
    ReplaceAll(String, String),
}

#[derive(Default)]
pub struct ProjectSearch {
    pub query: String,
    pub replace: String,
    pub results: Vec<Match>,
    pub searched: Option<String>,
    pub summary: Option<String>,
    first: bool,
    /// When the query was last edited: the search runs once typing pauses.
    typed_at: Option<f64>,
}

/// "1 arquivo", "3 arquivos".
/// `text` with every case-insensitive occurrence of `query` in bold accent.
fn highlighted(ui: &egui::Ui, text: &str, query: &str) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let body = egui::TextStyle::Monospace.resolve(ui.style());
    let normal = egui::TextFormat { font_id: body.clone(), color: ui.visuals().text_color(), ..Default::default() };
    let hit = egui::TextFormat {
        font_id: body,
        color: ui.visuals().strong_text_color(),
        background: crate::theme::accent().gamma_multiply(0.35),
        ..Default::default()
    };
    let lower = text.to_lowercase();
    let q = query.to_lowercase();
    // Lowercasing can change byte lengths; only highlight when it did not.
    if q.is_empty() || lower.len() != text.len() {
        job.append(text, 0.0, normal);
        return job;
    }
    let mut at = 0;
    while let Some(i) = lower[at..].find(&q) {
        let s = at + i;
        job.append(&text[at..s], 0.0, normal.clone());
        job.append(&text[s..s + q.len()], 0.0, hit.clone());
        at = s + q.len();
    }
    job.append(&text[at..], 0.0, normal);
    job
}

impl ProjectSearch {
    /// Puts the cursor in the query field on the next frame.
    pub fn focus(&mut self) {
        self.first = true;
    }

    /// The search panel, in the sidebar: it stays open while results are
    /// opened and edited.
    pub fn ui(&mut self, ui: &mut egui::Ui, root: Option<&Path>) -> Option<SearchAction> {
        let mut action = None;
        {
            ui.horizontal(|ui| {
                let r = ui.add(crate::widgets::field(&mut self.query).hint_text(t("Search (case-insensitive)")).desired_width(ui.available_width() - 70.0));
                if self.first {
                    self.first = false;
                    r.request_focus();
                }
                let now = ui.input(|i| i.time);
                if r.changed() {
                    self.typed_at = Some(now);
                }
                let enter = r.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter));
                let settled = self.typed_at.map(|t| now - t >= 0.25).unwrap_or(false);
                if ui.button(t("Search")).clicked() || enter || (settled && self.query.trim().chars().count() >= 2) {
                    self.typed_at = None;
                    action = Some(SearchAction::Search(self.query.clone()));
                } else if self.typed_at.is_some() {
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(260));
                }
            });
            ui.add(crate::widgets::field(&mut self.replace).hint_text(t("Replace with")).desired_width(f32::INFINITY));
            let can = !self.results.is_empty() && self.searched.as_deref() == Some(self.query.as_str());
            if ui.add_enabled(can, egui::Button::new(t("Replace all"))).clicked() {
                action = Some(SearchAction::ReplaceAll(self.query.clone(), self.replace.clone()));
            }
            if let Some(s) = &self.summary {
                ui.label(s);
            }
            if let Some(q) = &self.searched {
                let files: std::collections::HashSet<_> = self.results.iter().map(|m| &m.file).collect();
                if self.results.is_empty() {
                    ui.weak(tf!("No occurrence of \"{query}\" in the project.", query = q));
                } else {
                    ui.weak(
                        tn("{n} occurrence of \"{query}\"", "{n} occurrences of \"{query}\"", self.results.len()).replace("{query}", q)
                            + " "
                            + &tn("in {n} file", "in {n} files", files.len()),
                    );
                }
            }
            ui.separator();
            egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
                let mut last_file = None;
                for m in &self.results {
                    if last_file != Some(&m.file) {
                        ui.add_space(4.0);
                        ui.strong(relative(&m.file, root));
                        last_file = Some(&m.file);
                    }
                    let text: String = format!("{:>4}  {}", m.line, m.text.trim().chars().take(140).collect::<String>());
                    let job = highlighted(ui, &text, self.searched.as_deref().unwrap_or(""));
                    let button = egui::Button::selectable(false, job).truncate().min_size(egui::vec2(ui.available_width(), 0.0));
                    if ui.add(button).on_hover_text(m.text.trim()).clicked() {
                        action = Some(SearchAction::GoTo(m.clone()));
                    }
                }
            });
        }
        action
    }
}

// ---------- Local history ----------

/// Browses a file's local history (see `backend::history`) and restores a
/// version.
pub struct HistoryDialog {
    pub file: PathBuf,
    versions: Vec<crate::backend::history::Version>,
    /// (added, removed) lines against the current text, per version.
    summaries: Vec<(usize, usize)>,
    selected: usize,
    preview: String,
}

pub enum HistoryAction {
    Restore(String),
    Close,
}

fn when(ms: u64) -> String {
    use chrono::{Local, TimeZone};
    let Some(at) = Local.timestamp_millis_opt(ms as i64).single() else { return "?".into() };
    let today = Local::now().date_naive();
    if at.date_naive() == today {
        tf!("today, {time}", time = at.format("%H:%M"))
    } else if Some(at.date_naive()) == today.pred_opt() {
        tf!("yesterday, {time}", time = at.format("%H:%M"))
    } else {
        // A strftime pattern: the date in the order the language writes it.
        at.format(t("%m/%d/%Y, %H:%M")).to_string()
    }
}

impl HistoryDialog {
    pub fn new(root: &Path, file: &Path, current: &str) -> Self {
        use crate::backend::history;
        let versions = history::list(root, file);
        let summaries = versions
            .iter()
            .map(|v| history::read(v).map(|old| history::change_summary(&old, current)).unwrap_or((0, 0)))
            .collect();
        let preview = versions.first().and_then(|v| history::read(v).ok()).unwrap_or_default();
        Self { file: file.to_path_buf(), versions, summaries, selected: 0, preview }
    }

    pub fn show(&mut self, ctx: &egui::Context) -> Option<HistoryAction> {
        let mut action = None;
        let name = crate::backend::project::file_name(&self.file);
        let closed = modal(ctx, "local-history", 820.0, |ui| {
            ui.heading(tf!("Local history: {name}", name));
            ui.weak(t("One version every ~2 minutes of work, kept in .tectec/history. Restoring can be undone too."));
            ui.add_space(6.0);
            if self.versions.is_empty() {
                ui.label(t("No versions of this file yet. They appear as it is saved."));
            } else {
                let mut pick = None;
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(250.0);
                        egui::ScrollArea::vertical().id_salt("history-list").max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                            for (i, v) in self.versions.iter().enumerate() {
                                let (added, removed) = self.summaries[i];
                                let change = match added + removed {
                                    0 => t("same as the current text").to_string(),
                                    n => tn("{n} line different from the current one", "{n} lines different from the current one", n),
                                };
                                let mut job = egui::text::LayoutJob::default();
                                let body = egui::TextStyle::Body.resolve(ui.style());
                                let small = egui::TextStyle::Small.resolve(ui.style());
                                job.append(&when(v.time), 0.0, egui::TextFormat { font_id: body, color: ui.visuals().text_color(), ..Default::default() });
                                job.append(&format!("\n{change}"), 0.0, egui::TextFormat { font_id: small, color: ui.visuals().weak_text_color(), ..Default::default() });
                                let r = ui.add(egui::Button::selectable(i == self.selected, job).right_text("").min_size(egui::vec2(ui.available_width(), 0.0)));
                                if r.clicked() {
                                    pick = Some(i);
                                }
                            }
                        });
                    });
                    ui.add_space(10.0);
                    let size = egui::vec2(ui.available_width(), 420.0);
                    ui.allocate_ui(size, |ui| {
                        ui.set_min_size(size);
                        egui::ScrollArea::both().id_salt("history-preview").max_height(420.0).auto_shrink([false, true]).show(ui, |ui| {
                            ui.add(egui::Label::new(RichText::new(&self.preview).monospace().size(12.0)).extend());
                        });
                    });
                });
                if let Some(i) = pick {
                    self.selected = i;
                    self.preview = crate::backend::history::read(&self.versions[i]).unwrap_or_default();
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.add_enabled(!self.versions.is_empty(), crate::widgets::primary(t("Restore this version"))).clicked() {
                    action = Some(HistoryAction::Restore(self.preview.clone()));
                }
                if ui.add(crate::widgets::secondary(t("Close"))).clicked() {
                    action = Some(HistoryAction::Close);
                }
            });
        });
        if closed && action.is_none() {
            action = Some(HistoryAction::Close);
        }
        action
    }
}
