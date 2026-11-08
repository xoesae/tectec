//! The sidebar views: file tree (with favorites and the "go to file"
//! filter), outline, references, the knowledge hub (notes, TODOs, papers)
//! and the image library.

use super::actions::Cmd;
use super::App;
use crate::backend::project::{self, Kind, Node};
use crate::editor::ImageDrag;
use crate::i18n::{t, tn};
use crate::icons::{self, Icon};
use crate::settings::SidebarView;
use crate::tf;
use egui::RichText;
use std::path::PathBuf;

enum RefAction {
    New,
    Cite(String),
    Show(PathBuf, String),
    /// Before that key, or to the end of the file.
    Move(PathBuf, String, Option<String>),
    /// The key, and whether the text cites it.
    Remove(PathBuf, String, bool),
}

enum ImageAction {
    Add,
    Insert(String),
    GoTo(PathBuf, usize),
    Open(PathBuf),
    Reveal(PathBuf),
    /// The image, and how many places use it.
    Remove(PathBuf, usize),
}

/// A reference being dragged to another place in its .bib.
struct RefDrag {
    file: PathBuf,
    key: String,
}

enum TreeAction {
    Open(PathBuf, Kind),
    SetMain(PathBuf),
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Favorite(PathBuf),
    Rename(PathBuf),
    Delete(PathBuf),
    Reveal(PathBuf),
    Toggle(PathBuf),
}

impl App {
    pub fn sidebar(&mut self, ui: &mut egui::Ui) {
        let index = SidebarView::ALL.iter().position(|(v, _, _)| *v == self.s.sidebar_view).unwrap_or(0);
        let (shown, rise) = view_switch(ui, index);
        // The title, its text in line with the text of the search field
        // right under it (the field's margin is 8 points).
        crate::widgets::bar_of(ui, 28.0, |ui| {
            ui.add_space(8.0);
            ui.scope(|ui| {
                ui.multiply_opacity(shown);
                ui.strong(self.s.sidebar_view.label());
            });
        });
        if self.root.is_none() {
            ui.vertical_centered(|ui| {
                ui.add_space(20.0);
                ui.weak(t("No project open."));
                if ui.button(t("Open project…")).clicked() {
                    self.run(Cmd::OpenProject);
                }
            });
            return;
        }
        let body = ui.available_rect_before_wrap().translate(egui::vec2(0.0, rise));
        ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
            ui.multiply_opacity(shown);
            match self.s.sidebar_view {
                SidebarView::Tree => self.tree_view(ui),
                SidebarView::Outline => self.outline_view(ui),
                SidebarView::References => self.references_view(ui),
                SidebarView::Knowledge => self.knowledge_view(ui),
                SidebarView::Images => self.images_view(ui),
                SidebarView::Search => self.search_view(ui),
            }
        });
    }

    // ---------- Tree ----------

    fn tree_view(&mut self, ui: &mut egui::Ui) {
        let mut actions = Vec::new();
        let r = ui.add(crate::widgets::field(&mut self.tree_filter).hint_text(t("Go to file… (Ctrl+Shift+N)")).desired_width(f32::INFINITY));
        if self.tree_filter_focus {
            self.tree_filter_focus = false;
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        if r.has_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.tree_filter.clear();
        }
        let Some(tree) = self.tree.clone() else { return };

        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            if !self.tree_filter.trim().is_empty() {
                let q = crate::textmatch::folded(&self.tree_filter);
                let mut files = Vec::new();
                tree.files(&mut files);
                let matches: Vec<Node> = files
                    .into_iter()
                    .filter(|f| crate::textmatch::folded(&project::relative(&f.path, self.root.as_deref())).contains(&q))
                    .take(200)
                    .collect();
                if matches.is_empty() {
                    ui.weak(t("No file found."));
                }
                if enter {
                    if let Some(first) = matches.first() {
                        actions.push(TreeAction::Open(first.path.clone(), first.kind));
                        self.tree_filter.clear();
                    }
                }
                for f in &matches {
                    let rel = project::relative(&f.path, self.root.as_deref());
                    if ui.add(icons::selectable(false, icon(f), rel)).clicked() {
                        actions.push(TreeAction::Open(f.path.clone(), f.kind));
                        self.tree_filter.clear();
                    }
                }
            } else {
                let favorites: Vec<Node> = self.s.favorites.iter().filter_map(|p| tree.find(p).cloned()).collect();
                if !favorites.is_empty() {
                    ui.weak(t("FAVORITES"));
                    for f in &favorites {
                        let r = ui.add(icons::selectable(false, Icon::Star, f.name.as_str())).on_hover_text(project::relative(&f.path, self.root.as_deref()));
                        if r.clicked() {
                            if f.is_dir {
                                actions.push(TreeAction::Toggle(f.path.clone()));
                            } else {
                                actions.push(TreeAction::Open(f.path.clone(), f.kind));
                            }
                        }
                        crate::menu::context(&r, |ui| {
                            if ui.add(crate::menu::item(t("Remove from favorites"))).clicked() {
                                actions.push(TreeAction::Favorite(f.path.clone()));
                                ui.close();
                            }
                        });
                    }
                    ui.separator();
                }
                self.tree_node(ui, &tree, 0, &mut actions);
            }
        });

        for a in actions {
            match a {
                TreeAction::Open(p, k) => {
                    self.open_file(&p, k);
                }
                TreeAction::SetMain(p) => self.set_main_file(p, false),
                TreeAction::NewFile(d) => self.prompt_new_file(d),
                TreeAction::NewFolder(d) => self.prompt_new_folder(d),
                TreeAction::Favorite(p) => self.s.toggle_favorite(&p),
                TreeAction::Rename(p) => self.prompt_rename(p),
                TreeAction::Delete(p) => self.confirm_delete(p),
                TreeAction::Reveal(p) => self.reveal(&p),
                TreeAction::Toggle(p) => {
                    if !self.expanded.remove(&p) {
                        self.expanded.insert(p);
                    }
                }
            }
        }
    }

    fn tree_node(&self, ui: &mut egui::Ui, node: &Node, depth: usize, actions: &mut Vec<TreeAction>) {
        let is_root = Some(&node.path) == self.root.as_ref();
        if !is_root {
            let open = self.expanded.contains(&node.path);
            let active = self.active_tab().map(|t| t.path == node.path).unwrap_or(false);
            let is_main = self.main_file.as_ref() == Some(&node.path);
            // The arrow turns to point down as the folder opens.
            let turn = if node.is_dir { ui.ctx().animate_bool_with_time(egui::Id::new(("tree-arrow", &node.path)), open, crate::theme::MOTION) } else { 0.0 };
            let glyph = if node.is_dir && open { Icon::FolderOpen } else { icon(node) };
            let dirty = self.tabs.iter().any(|t| t.dirty && t.path == node.path);
            let mut label = node.name.clone();
            if dirty {
                label.push_str(" ●");
            }
            let mut text = RichText::new(label);
            if is_main {
                text = text.strong();
            }
            let r = ui
                .horizontal(|ui| {
                    ui.add_space(depth as f32 * 12.0);
                    // A row-wide label, a favorite's star at its right end.
                    let weak = ui.visuals().weak_text_color();
                    let mut right: Vec<egui::Atom<'_>> = vec![];
                    if self.s.is_favorite(&node.path) {
                        right.push(Icon::Star.image(weak, 11.0).into());
                    }
                    // Files get an invisible arrow, so names line up with folders'.
                    let arrow = Icon::ChevronRight
                        .tinted(if node.is_dir { ui.visuals().weak_text_color() } else { egui::Color32::TRANSPARENT })
                        .rotate(turn * std::f32::consts::FRAC_PI_2, egui::Vec2::splat(0.5));
                    // The icon in the name's colour (the SVGs are white, which
                    // the light theme would not show); the main file's in the
                    // accent.
                    let glyph = glyph.tinted(if is_main { crate::theme::accent() } else { ui.visuals().text_color() });
                    let button = egui::Button::selectable(active, (arrow, glyph, text))
                        .image_tint_follows_text_color(false)
                        .right_text(egui::Atoms::from(right))
                        .truncate()
                        .min_size(egui::vec2(ui.available_width(), 0.0));
                    let r = ui.add(button);
                    r
                })
                .inner;
            let r = if is_main { r.on_hover_text(t("Main file: the one compiled into the preview")) } else { r };
            if r.clicked() {
                if node.is_dir {
                    actions.push(TreeAction::Toggle(node.path.clone()));
                } else {
                    actions.push(TreeAction::Open(node.path.clone(), node.kind));
                }
            }
            self.node_menu(&r, node, false, actions);
            if !node.is_dir || !open {
                return;
            }
        } else {
            // Right-clicking the empty area acts on the root.
            let bg = ui.interact(ui.available_rect_before_wrap(), ui.id().with("tree-bg"), egui::Sense::click());
            self.node_menu(&bg, node, true, actions);
        }
        for child in &node.children {
            self.tree_node(ui, child, if is_root { 0 } else { depth + 1 }, actions);
        }
    }

    fn node_menu(&self, r: &egui::Response, node: &Node, is_root: bool, actions: &mut Vec<TreeAction>) {
        crate::menu::context(&r, |ui| {
            let target_dir = if node.is_dir { node.path.clone() } else { node.path.parent().unwrap_or(&node.path).to_path_buf() };
            if !node.is_dir {
                if ui.add(crate::menu::item(t("Open"))).clicked() {
                    actions.push(TreeAction::Open(node.path.clone(), node.kind));
                    ui.close();
                }
                if project::is_document_source(&node.path) && ui.add(crate::menu::item(t("Set as main file"))).clicked() {
                    actions.push(TreeAction::SetMain(node.path.clone()));
                    ui.close();
                }
                crate::menu::separator(ui);
            }
            if ui.add(crate::menu::item(t("New file…"))).clicked() {
                actions.push(TreeAction::NewFile(target_dir.clone()));
                ui.close();
            }
            if ui.add(crate::menu::item(t("New folder…"))).clicked() {
                actions.push(TreeAction::NewFolder(target_dir));
                ui.close();
            }
            crate::menu::separator(ui);
            let fav = if self.s.is_favorite(&node.path) { t("Remove from favorites") } else { t("Add to favorites") };
            if ui.add_enabled(!is_root, crate::menu::item(fav)).clicked() {
                actions.push(TreeAction::Favorite(node.path.clone()));
                ui.close();
            }
            crate::menu::separator(ui);
            if ui.add_enabled(!is_root, crate::menu::item(t("Rename…"))).clicked() {
                actions.push(TreeAction::Rename(node.path.clone()));
                ui.close();
            }
            if ui.add_enabled(!is_root, crate::menu::item(t("Delete")).danger()).clicked() {
                actions.push(TreeAction::Delete(node.path.clone()));
                ui.close();
            }
            crate::menu::separator(ui);
            if ui.add(crate::menu::item(t("Reveal in the file manager"))).clicked() {
                actions.push(TreeAction::Reveal(node.path.clone()));
                ui.close();
            }
        });
    }

    // ---------- Outline ----------

    fn outline_view(&mut self, ui: &mut egui::Ui) {
        if let Some(hint) = &self.outline_hint {
            ui.weak(hint);
            return;
        }
        let (rows, current) = self.outline_rows();
        if rows.is_empty() {
            ui.weak(t("No section in this file."));
            return;
        }
        if let Some(words) = self.document_words() {
            // Level with the rows' text, which starts after the button padding.
            ui.horizontal(|ui| {
                ui.add_space(ui.spacing().button_padding.x);
                ui.add(egui::Label::new(RichText::new(tf!("{words} words in the document", words = crate::i18n::number(words))).weak()).truncate());
            });
            ui.add_space(2.0);
        }
        let min_level = rows.iter().map(|r| r.entry.level).min().unwrap_or(0);
        let active = self.active_tab().map(|t| t.path.clone());
        let mut go = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for (i, row) in rows.iter().enumerate() {
                let e = &row.entry;
                ui.horizontal(|ui| {
                    ui.add_space((e.level - min_level) as f32 * 14.0);
                    let mut text = RichText::new(&e.title);
                    if e.starred {
                        text = text.italics();
                    }
                    if e.level <= min_level {
                        text = text.strong();
                    }
                    // Truncated, or the longest title would set the panel's minimum width.
                    let mut button = egui::Button::selectable(current == Some(i), text).truncate().min_size(egui::vec2(ui.available_width(), 0.0));
                    if let Some(words) = row.words {
                        button = button.right_text(RichText::new(crate::i18n::number(words)).small().weak());
                    }
                    let place = if Some(&row.file) == active.as_ref() {
                        tf!("line {line}", line = e.line)
                    } else {
                        format!("{} · {}", project::relative(&row.file, self.root.as_deref()), tf!("line {line}", line = e.line))
                    };
                    let tip = match row.words {
                        Some(words) => format!("\\{} · {place} · {}", e.kind, crate::i18n::tn("{n} word", "{n} words", words)),
                        None => format!("\\{} · {place}", e.kind),
                    };
                    if ui.add(button).on_hover_text(tip).clicked() {
                        go = Some((row.file.clone(), e.line));
                    }
                });
            }
        });
        if let Some((file, line)) = go {
            self.go_to(&file, line, false);
        }
    }

    // ---------- References ----------

    fn references_view(&mut self, ui: &mut egui::Ui) {
        use crate::settings::RefSort;
        let mut action = None;
        crate::widgets::bar(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(icons::icon_button(Icon::Plus)).on_hover_text(t("New reference…")).clicked() {
                    action = Some(RefAction::New);
                }
                let sort = crate::menu::dropdown(ui, icons::icon_button(Icon::ArrowUpDown), |ui| {
                        for (sort, label) in RefSort::ALL {
                            if ui.add(crate::menu::item(t(label)).checked(self.s.ref_sort == sort)).clicked() {
                                self.s.ref_sort = sort;
                                ui.close();
                            }
                        }
                    })
                    .0;
                sort.on_hover_text(tf!("Order: {order}", order = self.s.ref_sort.label()));
                ui.add(crate::widgets::field(&mut self.ref_filter).hint_text(t("Search references")).desired_width(ui.available_width()));
            });
        });
        let Some(refs) = &self.refs else {
            ui.weak(t("Loading…"));
            return;
        };
        if refs.is_empty() {
            ui.add_space(8.0);
            ui.weak(t("No reference yet. Add one by its DOI, ISBN or arXiv ID, or fill it in by hand."));
            ui.add_space(4.0);
            if ui.button(t("New reference…")).clicked() {
                self.run(Cmd::NewReference(crate::dialogs::ReferenceSource::Identifier));
            }
            return;
        }

        // The count and the "not cited" filter.
        let unused = refs.iter().filter(|e| !e.used).count();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let small = |s: &str| RichText::new(s).small();
            ui.label(small(&tn("{n} reference", "{n} references", refs.len())).weak());
            if unused > 0 {
                ui.label(small("·").weak());
                let label = tn("{n} not cited", "{n} not cited", unused);
                let r = ui.selectable_label(self.ref_unused_only, small(&label)).on_hover_text(t("Show only the references the text does not cite"));
                if r.clicked() {
                    self.ref_unused_only = !self.ref_unused_only;
                }
            }
        });
        ui.add_space(2.0);

        let q = crate::textmatch::folded(&self.ref_filter);
        let mut shown: Vec<&crate::backend::references::Entry> = refs
            .iter()
            .filter(|e| !self.ref_unused_only || !e.used)
            .filter(|e| q.is_empty() || [&e.key, &e.title, &e.author, &e.year].iter().any(|f| crate::textmatch::folded(f).contains(&q)))
            .collect();
        let folded = |s: &str| crate::textmatch::folded(s);
        match self.s.ref_sort {
            RefSort::File => {}
            RefSort::Author => shown.sort_by_cached_key(|e| (folded(&e.author), e.year.clone())),
            RefSort::Year => shown.sort_by(|a, b| b.year.cmp(&a.year).then_with(|| a.author.cmp(&b.author))),
            RefSort::Title => shown.sort_by_cached_key(|e| folded(&e.title)),
        }
        // Dragging changes the order in the .bib, so only in its order.
        let reorder = self.s.ref_sort == RefSort::File;
        if shown.is_empty() {
            ui.weak(t("No reference found."));
        }

        let dragged = egui::DragAndDrop::payload::<RefDrag>(ui.ctx());
        let mut drop_line: Option<f32> = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for e in &shown {
                // The next entry of the same file, in the file's order: where a
                // drop on this row's lower half goes before.
                let next_in_file = || refs.iter().skip_while(|o| !(o.key == e.key && o.file == e.file)).skip(1).find(|o| o.file == e.file).map(|o| o.key.clone());
                let width = ui.available_width();
                // Painted once the row's size is known, under its text.
                let background = ui.painter().add(egui::Shape::Noop);
                let r = ui
                    .scope_builder(egui::UiBuilder::new(), |ui| {
                        ui.set_width(width);
                        egui::Frame::new().inner_margin(egui::Margin { left: 14, right: 6, top: 5, bottom: 5 }).show(ui, |ui| {
                            ui.set_width(width - 20.0 - 24.0);
                            ui.spacing_mut().item_spacing.y = 2.0;
                            let title = if e.title.is_empty() { e.key.as_str() } else { e.title.as_str() };
                            ui.label(RichText::new(title).color(ui.visuals().strong_text_color()));
                            let byline = [crate::backend::references::short_authors(&e.author), e.year.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
                            ui.horizontal_wrapped(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                if !byline.is_empty() {
                                    ui.label(RichText::new(byline).small().weak());
                                }
                                ui.label(RichText::new(&e.key).small().monospace().weak());
                                if e.duplicate {
                                    ui.label(RichText::new(t("duplicate?")).small().color(ui.visuals().error_fg_color))
                                        .on_hover_text(t("Another entry has the same DOI or a very similar title."));
                                }
                            });
                        });
                    })
                    .response;
                let rect = r.rect;
                let id = ui.id().with(("ref", &e.key, &e.file));
                let r = ui.interact(rect, id, if reorder { egui::Sense::click_and_drag() } else { egui::Sense::click() });
                let is_dragged = dragged.as_ref().is_some_and(|d| d.key == e.key && d.file == e.file);
                if is_dragged {
                    ui.painter().set(background, egui::Shape::rect_filled(rect, 4.0, ui.visuals().selection.bg_fill.gamma_multiply(0.5)));
                } else if r.hovered() && dragged.is_none() {
                    ui.painter().set(background, egui::Shape::rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill));
                }
                // Not cited in the text: a dot in the margin, by the title.
                if !e.used {
                    let dot = egui::pos2(rect.left() + 6.0, rect.top() + 5.0 + ui.text_style_height(&egui::TextStyle::Body) / 2.0);
                    ui.painter().circle_filled(dot, 2.5, ui.visuals().warn_fg_color);
                    let area = egui::Rect::from_center_size(dot, egui::vec2(10.0, 14.0));
                    ui.interact(area, id.with("unused"), egui::Sense::hover()).on_hover_text(t("Not cited in the text yet"));
                }
                // The remove button, over the row's top right corner.
                if dragged.is_none() && ui.ctx().pointer_hover_pos().is_some_and(|p| rect.contains(p)) {
                    let trash = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.top() + 16.0), egui::vec2(22.0, 22.0));
                    let tr = ui.interact(trash, id.with("remove"), egui::Sense::click()).on_hover_text(t("Remove from the .bib"));
                    if tr.hovered() {
                        ui.painter().rect_filled(trash, 4.0, ui.visuals().widgets.active.bg_fill);
                    }
                    icons::show_at(ui, Icon::Trash, trash.shrink(4.0), if tr.hovered() { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() });
                    if tr.clicked() {
                        action = Some(RefAction::Remove(e.file.clone(), e.key.clone(), e.used));
                    }
                }
                let r = r.on_hover_text(if reorder { t("Click to cite at the cursor. Drag to change the order in the .bib.") } else { t("Click to cite at the cursor") });
                if r.clicked() && action.is_none() {
                    action = Some(RefAction::Cite(e.key.clone()));
                }
                if r.drag_started() {
                    egui::DragAndDrop::set_payload(ui.ctx(), RefDrag { file: e.file.clone(), key: e.key.clone() });
                }
                if r.dragged() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                }
                // A drop on another row of the same file: before it (upper
                // half) or after it (lower half).
                if let (Some(d), Some(p)) = (dragged.as_ref(), ui.ctx().pointer_hover_pos()) {
                    if rect.contains(p) && d.file == e.file && d.key != e.key {
                        let upper = p.y < rect.center().y;
                        drop_line = Some(if upper { rect.top() } else { rect.bottom() });
                        if ui.input(|i| i.pointer.any_released()) {
                            let before = if upper { Some(e.key.clone()) } else { next_in_file() };
                            if before.as_deref() != Some(d.key.as_str()) {
                                action = Some(RefAction::Move(d.file.clone(), d.key.clone(), before));
                            }
                        }
                    }
                }
                crate::menu::context(&r, |ui| {
                    if ui.add(crate::menu::item(t("Cite at the cursor"))).clicked() {
                        action = Some(RefAction::Cite(e.key.clone()));
                        ui.close();
                    }
                    if ui.add(crate::menu::item(t("Copy key"))).clicked() {
                        ui.ctx().copy_text(e.key.clone());
                        ui.close();
                    }
                    if ui.add(crate::menu::item(tf!("Show in {file}", file = project::file_name(&e.file)))).clicked() {
                        action = Some(RefAction::Show(e.file.clone(), e.key.clone()));
                        ui.close();
                    }
                    if reorder {
                        crate::menu::separator(ui);
                        let same_file: Vec<&String> = refs.iter().filter(|o| o.file == e.file).map(|o| &o.key).collect();
                        let at = same_file.iter().position(|k| **k == e.key).unwrap_or(0);
                        if ui.add_enabled(at > 0, crate::menu::item(t("Move up"))).clicked() {
                            action = Some(RefAction::Move(e.file.clone(), e.key.clone(), Some(same_file[at - 1].clone())));
                            ui.close();
                        }
                        if ui.add_enabled(at + 1 < same_file.len(), crate::menu::item(t("Move down"))).clicked() {
                            action = Some(RefAction::Move(e.file.clone(), e.key.clone(), same_file.get(at + 2).map(|k| (*k).clone())));
                            ui.close();
                        }
                    }
                    crate::menu::separator(ui);
                    if ui.add(crate::menu::item(t("Remove…")).danger()).clicked() {
                        action = Some(RefAction::Remove(e.file.clone(), e.key.clone(), e.used));
                        ui.close();
                    }
                });
            }
            if let Some(y) = drop_line {
                let x = ui.min_rect().x_range();
                ui.painter().hline(x, y, egui::Stroke::new(2.0, crate::theme::accent()));
            }
        });
        if dragged.is_some() && ui.input(|i| i.pointer.any_released()) {
            egui::DragAndDrop::clear_payload(ui.ctx());
        }
        match action {
            Some(RefAction::New) => self.run(Cmd::NewReference(crate::dialogs::ReferenceSource::Identifier)),
            Some(RefAction::Cite(k)) => self.insert_citation(&k),
            Some(RefAction::Show(f, k)) => self.show_reference(&f, &k),
            Some(RefAction::Move(f, k, before)) => self.move_reference(&f, &k, before.as_deref()),
            Some(RefAction::Remove(f, k, used)) => self.confirm_remove_reference(f, k, used),
            None => {}
        }
    }

    // ---------- Knowledge hub ----------

    fn knowledge_view(&mut self, ui: &mut egui::Ui) {
        let mut open: Option<(PathBuf, Kind)> = None;
        let mut goto: Option<(PathBuf, usize)> = None;
        let mut meta = None;
        let mut remove = None;
        let mut run = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            let (add, body) = section(ui, "notes", t("Notes"), self.notes.len(), Some(t("New note…")));
            if add {
                run = Some(Cmd::NewNote);
            }
            body.body(|ui| {
                if self.notes.is_empty() {
                    ui.weak(t("No notes yet. Notes are Markdown files kept with the project."));
                }
                for n in &self.notes {
                    if ui.add(icons::selectable(false, Icon::NotebookPen, n.title.as_str())).clicked() {
                        open = Some((n.path.clone(), Kind::Text));
                    }
                }
            });
            section(ui, "todos", "TODOs", self.todos.len(), None).1.body(|ui| {
                if self.todos.is_empty() {
                    ui.weak(t("Nothing pending. Use % TODO: in the .tex or - [ ] in a note."));
                }
                for todo in &self.todos {
                    let place = format!("{}:{}", project::relative(&todo.file, self.root.as_deref()), todo.line);
                    let mark = if todo.from_code { Icon::Code } else { Icon::Square };
                    if ui.add(icons::selectable(false, mark, todo.text.as_str())).on_hover_text(place).clicked() {
                        goto = Some((todo.file.clone(), todo.line));
                    }
                }
            });
            let (add, body) = section(ui, "papers", t("Papers"), self.papers.len(), Some(t("Attach PDFs…")));
            if add {
                run = Some(Cmd::AddPapers);
            }
            body.body(|ui| {
                if self.papers.is_empty() {
                    ui.weak(t("No papers yet. Attach the PDFs you read to keep their keys and summaries here."));
                }
                for p in &self.papers {
                    let r = ui
                        .vertical(|ui| {
                            let place = project::relative(&p.path, self.root.as_deref());
                            icons::label(ui, Icon::FileText, p.name.as_str(), None).on_hover_text(format!("{place} · {}", human_size(p.size)));
                            if !p.cite_key.is_empty() {
                                ui.weak(format!("\\cite{{{}}}", p.cite_key));
                            }
                            if !p.summary.is_empty() {
                                ui.weak(p.summary.chars().take(160).collect::<String>());
                            }
                        })
                        .response;
                    let r = ui.interact(r.rect, ui.id().with(("paper", &p.path)), egui::Sense::click());
                    if r.clicked() {
                        open = Some((p.path.clone(), Kind::Pdf));
                    }
                    crate::menu::context(&r, |ui| {
                        if ui.add(crate::menu::item(t("Open"))).clicked() {
                            open = Some((p.path.clone(), Kind::Pdf));
                            ui.close();
                        }
                        if ui.add(crate::menu::item(t("Edit key and summary…"))).clicked() {
                            meta = Some((p.name.clone(), p.path.clone(), p.cite_key.clone(), p.summary.clone()));
                            ui.close();
                        }
                        if ui.add(crate::menu::item(t("Remove")).danger()).clicked() {
                            remove = Some((p.name.clone(), p.path.clone()));
                            ui.close();
                        }
                    });
                }
            });
        });
        if let Some((p, k)) = open {
            self.open_file(&p, k);
        }
        if let Some((f, l)) = goto {
            self.go_to(&f, l, false);
        }
        if let Some((n, p, k, s)) = meta {
            self.edit_paper_meta(n, p, k, s);
        }
        if let Some((n, p)) = remove {
            self.confirm_remove_paper(n, p);
        }
        if let Some(c) = run {
            self.run(c);
        }
    }

    // ---------- Images ----------

    fn images_view(&mut self, ui: &mut egui::Ui) {
        use crate::settings::ImageSort;
        let folder = self.s.folders.path(crate::settings::FolderKind::Images);
        let mut action = None;
        crate::widgets::bar(ui, |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let tip = tf!("Add images… (copied to {folder}/)", folder = folder.to_string_lossy().replace('\\', "/"));
                if ui.add(icons::icon_button(Icon::Plus)).on_hover_text(tip).clicked() {
                    action = Some(ImageAction::Add);
                }
                let sort = crate::menu::dropdown(ui, icons::icon_button(Icon::ArrowUpDown), |ui| {
                        for (sort, label) in ImageSort::ALL {
                            if ui.add(crate::menu::item(t(label)).checked(self.s.image_sort == sort)).clicked() {
                                self.s.image_sort = sort;
                                ui.close();
                            }
                        }
                    })
                    .0;
                sort.on_hover_text(tf!("Order: {order}", order = self.s.image_sort.label()));
                ui.add(crate::widgets::field(&mut self.image_filter).hint_text(t("Search images")).desired_width(ui.available_width()));
            });
        });
        if self.images.is_empty() {
            ui.add_space(8.0);
            ui.weak(t("No image in the project yet. Add some, paste one with Ctrl+Alt+I, or drag a file onto the window."));
            ui.add_space(4.0);
            if ui.button(t("Add images…")).clicked() {
                self.run(Cmd::AddImages);
            }
            return;
        }

        // The count and the "not used" filter.
        let unused = self.images.iter().filter(|i| i.uses.is_empty()).count();
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let small = |s: &str| RichText::new(s).small();
            ui.label(small(&tn("{n} image", "{n} images", self.images.len())).weak());
            if unused > 0 {
                ui.label(small("·").weak());
                let label = tn("{n} not used", "{n} not used", unused);
                if ui.selectable_label(self.image_unused_only, small(&label)).on_hover_text(t("Show only the images the text does not use")).clicked() {
                    self.image_unused_only = !self.image_unused_only;
                }
            }
        });
        ui.add_space(2.0);

        let q = crate::textmatch::folded(&self.image_filter);
        let mut shown: Vec<&crate::backend::images::Image> = self
            .images
            .iter()
            .filter(|i| !self.image_unused_only || i.uses.is_empty())
            .filter(|i| q.is_empty() || crate::textmatch::folded(&i.rel_path).contains(&q))
            .collect();
        match self.s.image_sort {
            ImageSort::Name => shown.sort_by_cached_key(|i| crate::textmatch::folded(&i.name)),
            ImageSort::Size => shown.sort_by(|a, b| b.size.cmp(&a.size)),
            ImageSort::Uses => shown.sort_by(|a, b| a.uses.len().cmp(&b.uses.len()).then_with(|| a.name.cmp(&b.name))),
        }
        if shown.is_empty() {
            ui.weak(t("No image found."));
        }
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            for img in shown {
                let id = ui.id().with(("img", &img.path));
                let width = ui.available_width();
                let background = ui.painter().add(egui::Shape::Noop);
                // Dragged into the text, it becomes a figure there.
                let r = ui.dnd_drag_source(id, ImageDrag(img.rel_path.clone()), |ui| {
                    ui.set_width(width);
                    egui::Frame::new().inner_margin(egui::Margin { left: 6, right: 30, top: 5, bottom: 5 }).show(ui, |ui| {
                        ui.horizontal(|ui| {
                            // A slot of one size for every thumbnail, so the names line up.
                            let thumb = egui::Image::new(format!("file://{}", img.path.display())).fit_to_exact_size(egui::vec2(48.0, 36.0)).maintain_aspect_ratio(true).corner_radius(3);
                            ui.add_sized([48.0, 36.0], thumb);
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.add(egui::Label::new(RichText::new(&img.name).color(ui.visuals().strong_text_color())).truncate());
                                let uses = if img.uses.is_empty() { t("not used").to_string() } else { tn("{n} use", "{n} uses", img.uses.len()) };
                                ui.label(RichText::new(format!("{uses} · {}", human_size(img.size))).small().weak());
                                // What a figure lacks, quietly.
                                let mut lacks = Vec::new();
                                if img.missing_caption {
                                    lacks.push(t("no caption"));
                                }
                                if img.missing_label {
                                    lacks.push(t("no label"));
                                }
                                if !lacks.is_empty() {
                                    ui.label(RichText::new(lacks.join(" · ")).small().color(ui.visuals().warn_fg_color));
                                }
                            });
                        });
                    });
                });
                let rect = r.response.rect;
                let r = ui.interact(rect, id.with("click"), egui::Sense::click());
                let hovered = ui.rect_contains_pointer(rect);
                if hovered {
                    ui.painter().set(background, egui::Shape::rect_filled(rect, 4.0, ui.visuals().widgets.hovered.bg_fill));
                }
                // Not used by the text: a dot by the name.
                if img.uses.is_empty() {
                    let dot = egui::pos2(rect.right() - 16.0, rect.center().y);
                    if !hovered {
                        ui.painter().circle_filled(dot, 2.5, ui.visuals().warn_fg_color);
                    }
                }
                if hovered {
                    let trash = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), egui::vec2(22.0, 22.0));
                    let tr = ui.interact(trash, id.with("remove"), egui::Sense::click()).on_hover_text(t("Delete the image"));
                    if tr.hovered() {
                        ui.painter().rect_filled(trash, 4.0, ui.visuals().widgets.active.bg_fill);
                    }
                    icons::show_at(ui, Icon::Trash, trash.shrink(4.0), if tr.hovered() { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() });
                    if tr.clicked() {
                        action = Some(ImageAction::Remove(img.path.clone(), img.uses.len()));
                    }
                }
                let r = r.on_hover_text(format!("{}\n{}", img.rel_path, t("Click to insert the figure at the cursor, or drag it into the text.")));
                if r.clicked() && action.is_none() {
                    action = Some(ImageAction::Insert(img.rel_path.clone()));
                }
                crate::menu::context(&r, |ui| {
                    if ui.add(crate::menu::item(t("Insert figure"))).clicked() {
                        action = Some(ImageAction::Insert(img.rel_path.clone()));
                        ui.close();
                    }
                    if ui.add(crate::menu::item(t("Open image"))).clicked() {
                        action = Some(ImageAction::Open(img.path.clone()));
                        ui.close();
                    }
                    if ui.add(crate::menu::item(t("Reveal in the file manager"))).clicked() {
                        action = Some(ImageAction::Reveal(img.path.clone()));
                        ui.close();
                    }
                    if !img.uses.is_empty() {
                        crate::menu::separator(ui);
                        crate::menu::label(ui, t("Used in:"));
                        for u in &img.uses {
                            if ui.add(crate::menu::item(format!("{}:{}", project::relative(&u.file, self.root.as_deref()), u.line))).clicked() {
                                action = Some(ImageAction::GoTo(u.file.clone(), u.line));
                                ui.close();
                            }
                        }
                    }
                    crate::menu::separator(ui);
                    if ui.add(crate::menu::item(t("Delete…")).danger()).clicked() {
                        action = Some(ImageAction::Remove(img.path.clone(), img.uses.len()));
                        ui.close();
                    }
                });
            }
        });
        match action {
            Some(ImageAction::Add) => self.run(Cmd::AddImages),
            Some(ImageAction::Insert(rel)) => self.insert_image(&rel, None),
            Some(ImageAction::GoTo(f, l)) => self.go_to(&f, l, true),
            Some(ImageAction::Open(p)) => {
                let kind = project::kind_of(&project::file_name(&p), false);
                self.open_file(&p, kind);
            }
            Some(ImageAction::Reveal(p)) => self.reveal(&p),
            Some(ImageAction::Remove(p, uses)) => self.confirm_remove_image(p, uses),
            None => {}
        }
    }
}

fn icon(node: &Node) -> Icon {
    kind_icon(node.kind)
}

/// The icon for a kind of file: shared by the tree and the tabs.
pub(super) fn kind_icon(kind: Kind) -> Icon {
    match kind {
        Kind::Dir => Icon::Folder,
        Kind::Tex => Icon::FileCode,
        Kind::Image | Kind::Svg => Icon::FileImage,
        Kind::Pdf => Icon::FileText,
        Kind::Text => Icon::FilePen,
        Kind::Binary | Kind::Other => Icon::File,
    }
}

fn human_size(bytes: u64) -> String {
    if bytes >= 1 << 20 {
        format!("{:.1} MB", bytes as f64 / (1u64 << 20) as f64)
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).ceil())
    }
}

/// A collapsible section of the sidebar: its title, a count and, with
/// `add`, a "+" button (whose tooltip is `add`) at the right of the header.
/// Returns whether "+" was clicked and the header, for its body.
fn section<'u>(ui: &'u mut egui::Ui, id: &str, title: &str, count: usize, add: Option<&str>) -> (bool, egui::collapsing_header::HeaderResponse<'u, ()>) {
    // Apart from the section above.
    ui.add_space(6.0);
    let state = egui::collapsing_header::CollapsingState::load_with_default_open(ui.ctx(), ui.id().with(id), true);
    let mut clicked = false;
    let header = state.show_header(ui, |ui| {
        ui.strong(title);
        ui.weak(count.to_string());
        if let Some(tip) = add {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                clicked = ui.add(icons::icon_button(Icon::Plus).min_size(egui::vec2(22.0, 22.0))).on_hover_text(tip).clicked();
            });
        }
    });
    (clicked, header)
}

/// How far a view's content travels as it comes in, in points.
const VIEW_RISE: f32 = 8.0;

/// A new sidebar view fades in while sliding a few points from the side of
/// the activity bar it was picked from: below for a view further down,
/// above for one further up. Returns the opacity and the vertical offset.
fn view_switch(ui: &egui::Ui, index: usize) -> (f32, f32) {
    let id = egui::Id::new("sidebar-view-switch");
    let now = ui.input(|i| i.time);
    // (view shown, direction it came from, when).
    let (_, dir, at) = ui.data_mut(|d| {
        let state = d.get_temp_mut_or_insert_with(id, || (index, 0.0f32, f64::NEG_INFINITY));
        if state.0 != index {
            *state = (index, if index > state.0 { 1.0 } else { -1.0 }, now);
        }
        *state
    });
    let t = ((now - at) as f32 / crate::theme::MOTION).clamp(0.0, 1.0);
    if t < 1.0 {
        ui.ctx().request_repaint();
    }
    let eased = egui::emath::easing::quadratic_out(t);
    (eased, dir * VIEW_RISE * (1.0 - eased))
}
