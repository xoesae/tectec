//! Drawing the window: menu bar, status bar, error panel, activity bar,
//! sidebar, the tab strip with the LaTeX toolbar and the editor or viewer,
//! and the dockable preview.

use super::actions::{Cmd, MenuEntry};
use super::{App, StatusKind, TabView};
use crate::backend::latex;
use crate::backend::project::{self, Kind};
use crate::editor::{self, EditorEnv};
use crate::pdfview::PdfEvent;
use crate::settings::{Dock, SidebarView};
use crate::i18n::{n, t, tn};
use crate::icons::{self, Icon};
use crate::theme;
use crate::tf;
use egui::RichText;
use std::path::PathBuf;

/// Width of the zen mode's text column, in characters.
const ZEN_COLUMN_CHARS: f32 = 90.0;
/// How long a compilation runs before the preview shows a spinner.
const SPINNER_AFTER: f64 = 0.3;

/// The toolbar shown over a `.tex` tab.
enum Tool {
    Surround(&'static str, &'static str),
    Block(&'static str),
    Table,
    List,
    Math,
    Image,
    Find,
    PasteImage,
}

/// What a toolbar button shows.
enum Face {
    Bold,
    Italic,
    Underline,
    Icon(Icon),
    Text(&'static str),
}

const TOOLS: &[(Face, &str, Tool)] = &[
    (Face::Bold, n("Bold: \\textbf{} (Ctrl+B)"), Tool::Surround("\\textbf{", "}")),
    (Face::Italic, n("Emphasis (italic): \\emph{} (Ctrl+I)"), Tool::Surround("\\emph{", "}")),
    (Face::Underline, n("Underline: \\underline{}"), Tool::Surround("\\underline{", "}")),
    (Face::Icon(Icon::Heading), n("Section: \\section{}"), Tool::Block("\\section{$SEL$|}\n")),
    (Face::Icon(Icon::List), n("List… (bulleted or numbered)"), Tool::List),
    (Face::Icon(Icon::Table), n("Table… (choose rows/columns)"), Tool::Table),
    (Face::Icon(Icon::Image), n("Image… (from the project or the computer)"), Tool::Image),
    (Face::Text("$x$"), n("Inline math"), Tool::Surround("$", "$")),
    (Face::Icon(Icon::Sigma), n("Formula… (Ctrl+M)"), Tool::Math),
    (Face::Icon(Icon::ClipboardPaste), n("Paste image from the clipboard (Ctrl+Alt+I)"), Tool::PasteImage),
    (Face::Icon(Icon::Search), n("Find and replace (Ctrl+F)"), Tool::Find),
];

impl App {
    pub fn draw(&mut self, ui: &mut egui::Ui) {
        if self.zen_reading {
            egui::CentralPanel::default().frame(egui::Frame::central_panel(ui.style()).inner_margin(0)).show(ui, |ui| self.zen_reader(ui));
            return;
        }
        // Where each part lands, for the tour's spotlight.
        let mut spots = super::tour::Spots::default();
        if !self.zen {
            spots.menu = Some(egui::Panel::top("menu-bar").show(ui, |ui| self.menu_bar(ui)).response.rect);
        }
        egui::Panel::bottom("status-bar").show(ui, |ui| self.status_bar(ui));
        // The error panel and the sidebar slide in and out (theme::MOTION).
        if !self.zen {
            let mut open = self.errors_open && !self.errors.is_empty();
            let was = open;
            egui::Panel::bottom("error-panel")
                .resizable(true)
                .default_size(160.0)
                .drag_to_open(false)
                .show_collapsible(ui, &mut open, |ui| self.error_panel(ui));
            if open != was {
                self.errors_open = open;
            }
        }
        // Without a project, the start screen has the window to itself:
        // an empty sidebar and preview would only say "no project" again.
        if !self.zen && self.root.is_some() {
            let activity = egui::Panel::left("activity-bar").resizable(false).exact_size(40.0).show(ui, |ui| self.activity_bar(ui)).response.rect;
            let mut open = !self.s.sidebar_hidden;
            let sidebar = egui::Panel::left("sidebar")
                .default_size(260.0)
                .size_range(160.0..=600.0)
                .drag_to_open(false)
                .show_collapsible(ui, &mut open, |ui| self.sidebar(ui));
            spots.sidebar = sidebar.map(|s| s.response.rect.union(activity));
            self.s.sidebar_hidden = !open;
            let panel = match self.s.preview_dock {
                Dock::Right => egui::Panel::right("preview").default_size(520.0),
                Dock::Left => egui::Panel::left("preview-left").default_size(520.0),
                Dock::Top => egui::Panel::top("preview-top").resizable(true).default_size(360.0),
                Dock::Bottom => egui::Panel::bottom("preview-bottom").resizable(true).default_size(360.0),
            };
            let mut open = !self.s.preview_hidden;
            let was = open;
            spots.preview = panel.drag_to_open(false).show_collapsible(ui, &mut open, |ui| self.preview_panel(ui)).map(|p| p.response.rect);
            // Only on a drag-to-close: the panel's own close button sets the flag directly.
            if open != was {
                self.s.preview_hidden = !open;
            }
        }
        // The same top and bottom margin as the side panels', so the tabs sit
        // level with the sidebar's and the preview's headers.
        let frame = egui::Frame::central_panel(ui.style()).inner_margin(egui::Margin::symmetric(8, 2));
        let editor = egui::CentralPanel::default().frame(frame).show(ui, |ui| self.editor_pane(ui)).response.rect;
        spots.editor = Some(editor).filter(|_| !self.zen);
        self.spots = spots;
    }

    fn menu_bar(&mut self, ui: &mut egui::Ui) {
        let mut run: Option<Cmd> = None;
        egui::MenuBar::new().ui(ui, |ui| {
            for group in self.menu_groups() {
                crate::menu::bar(ui, group.header, |ui| {
                    let mut recent = group.recent;
                    let mut entries = group.entries.into_iter().peekable();
                    while let Some(e) = entries.next() {
                        if e.section {
                            // The recent projects close the first section.
                            if std::mem::take(&mut recent) {
                                self.recent_menu(ui, &mut run);
                            }
                            crate::menu::separator(ui);
                        }
                        match e.submenu {
                            Some(name) => {
                                let mut items = vec![e];
                                while let Some(next) = entries.next_if(|n| n.submenu == Some(name) && !n.section) {
                                    items.push(next);
                                }
                                crate::menu::submenu(ui, name, None, |ui| {
                                    for e in items {
                                        menu_item(ui, e, &mut run);
                                    }
                                });
                            }
                            None => menu_item(ui, e, &mut run),
                        }
                    }
                    if recent {
                        self.recent_menu(ui, &mut run);
                    }
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.scope(super::window::controls);
                ui.add_space(12.0);
                ui.weak(&self.save_indicator);
                // The preview's header shows the compilation; with it hidden,
                // a small spinner here does.
                if self.compiling && self.s.preview_hidden {
                    ui.add(egui::Spinner::new().size(12.0)).on_hover_text(t("Compiling…"));
                }
                super::window::drag_area(ui);
            });
        });
        if let Some(cmd) = run {
            self.run(cmd);
        }
    }

    fn recent_menu(&self, ui: &mut egui::Ui, run: &mut Option<Cmd>) {
        crate::menu::submenu(ui, t("Recent projects"), None, |ui| {
            if self.s.recent.is_empty() {
                crate::menu::label(ui, t("None yet"));
            }
            for r in &self.s.recent {
                if ui.add(crate::menu::item(&r.name)).on_hover_text(short_path(&r.path, 80)).clicked() {
                    *run = Some(Cmd::OpenRecent(r.path.clone()));
                }
            }
            if !self.s.recent.is_empty() {
                crate::menu::separator(ui);
                if ui.add(crate::menu::item(t("Clear the list"))).clicked() {
                    *run = Some(Cmd::ClearRecent);
                }
            }
        });
    }

    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let mut go_section = None;
        crate::widgets::bar_of(ui, 26.0, |ui| {
            // Where the cursor is in the document's sections, first: the
            // toolbar has no room for it.
            if self.active_tab().is_some_and(|t| t.editable() && t.is_tex()) {
                go_section = self.breadcrumb(ui);
                ui.add_space(12.0);
            }
            let color = match self.status.1 {
                StatusKind::Error => ui.visuals().error_fg_color,
                StatusKind::Success => theme::SUCCESS,
                StatusKind::Normal => ui.visuals().text_color(),
            };
            // Errors stay; anything else fades out after a few seconds.
            const STATUS_SHOW: f64 = 6.0;
            let age = ui.input(|i| i.time) - self.status_at;
            let fade_out = if self.status.1 == StatusKind::Error { 1.0 } else { (1.0 - (age - STATUS_SHOW)).clamp(0.0, 1.0) as f32 };
            // A new message fades in, briefly.
            let fade_in = (age as f32 / theme::MOTION).clamp(0.0, 1.0);
            let fade = fade_out.min(fade_in);
            if fade_in < 1.0 || (fade > 0.0 && fade < 1.0) {
                ui.ctx().request_repaint();
            } else if fade >= 1.0 && self.status.1 != StatusKind::Error {
                ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((STATUS_SHOW - age).max(0.05)));
            }
            let color = color.gamma_multiply(fade);
            let status: String = self.status.0.chars().take(220).collect();
            ui.label(RichText::new(status).color(color));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut next_suggestion = false;
                if let Some(tab) = self.active_tab().filter(|t| t.editable()) {
                    // What a writer follows: the words. The source's
                    // characters and lines are in the tooltip. (Right to
                    // left: the rightmost first.)
                    let s = tab.editor.stats;
                    let document = self.document_words();
                    let details = [tn("{n} character", "{n} characters", s.chars), tn("{n} line", "{n} lines", s.lines)];
                    if let Some(total) = document {
                        ui.weak(tf!("{words} in the document", words = crate::i18n::number(total)));
                        ui.weak("·");
                    }
                    ui.label(words_label(s.words)).on_hover_text(details.join("\n"));
                    if s.selected > 0 {
                        ui.weak(tn("{n} selected", "{n} selected", s.selected));
                    }
                    ui.add_space(10.0);
                    ui.weak(tf!("Ln {line}, Col {col}", line = s.line, col = s.col));
                    ui.add_space(6.0);
                    let n = tab.editor.issues().len();
                    if self.s.review && n > 0 {
                        let label = RichText::new(tn("{n} suggestion", "{n} suggestions", n)).color(ui.visuals().weak_text_color());
                        let tip = t("Writing review: F7 goes to the next one, Ctrl+. fixes it");
                        next_suggestion = ui.add(icons::button(Icon::SpellCheck, label).frame(false)).on_hover_text(tip).clicked();
                    }
                }
                if next_suggestion {
                    self.run(Cmd::NextSuggestion(true));
                }
                if !self.errors.is_empty() {
                    let fatal = self.errors.iter().filter(|e| !e.warning).count();
                    let (icon, label, color) = if fatal > 0 {
                        (Icon::CircleX, tn("{n} error", "{n} errors", fatal), ui.visuals().error_fg_color)
                    } else {
                        (Icon::TriangleAlert, tn("{n} warning", "{n} warnings", self.errors.len()), ui.visuals().warn_fg_color)
                    };
                    // A soft glow behind the count when it changes.
                    let glow_at = ui.painter().add(egui::Shape::Noop);
                    let r = ui.add(icons::button(icon, RichText::new(label).color(color)));
                    let glow = count_changed_glow(ui, (fatal, self.errors.len()));
                    if glow > 0.0 {
                        ui.painter().set(glow_at, egui::Shape::rect_filled(r.rect, 4.0, color.gamma_multiply(0.2 * glow)));
                    }
                    if r.on_hover_text(t("Show/hide the diagnostics")).clicked() {
                        self.errors_open = !self.errors_open;
                    }
                }
            });
        });
        if let Some((file, line)) = go_section {
            self.go_to(&file, line, false);
        }
    }

    fn error_panel(&mut self, ui: &mut egui::Ui) {
        let mut go = None;
        crate::widgets::bar(ui, |ui| {
            let fatal = self.errors.iter().filter(|e| !e.warning).count();
            let warnings = self.errors.len() - fatal;
            ui.add_space(4.0);
            if fatal > 0 {
                icons::label(ui, Icon::CircleX, tn("{n} error", "{n} errors", fatal), Some(ui.visuals().error_fg_color));
            }
            if warnings > 0 {
                icons::label(ui, Icon::TriangleAlert, tn("{n} warning", "{n} warnings", warnings), Some(ui.visuals().warn_fg_color));
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.add(icons::icon_button(Icon::X)).on_hover_text(t("Close")).clicked() {
                    self.errors_open = false;
                }
            });
        });
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            for (n, e) in self.errors.iter().enumerate() {
                let mark = if e.warning { Icon::TriangleAlert } else { Icon::CircleX };
                let color = if e.warning { ui.visuals().warn_fg_color } else { ui.visuals().error_fg_color };
                let place = match &e.file {
                    Some(f) if e.line > 0 => format!("{}:{}", project::relative(f, self.root.as_deref()), e.line),
                    Some(f) => project::relative(f, self.root.as_deref()),
                    None => String::new(),
                };
                let original = e.message.lines().next().unwrap_or_default().to_string();
                let summary = latex::summary(&e.message).unwrap_or_else(|| original.clone());
                // The hint, without the summary it may start with.
                let hint = self.diagnostic_hint(&e.message).map(|h| {
                    let rest = h.strip_prefix(summary.as_str()).map(|r| r.trim_start_matches([':', ' '])).unwrap_or(&h);
                    let mut chars = rest.chars();
                    chars.next().map(|c| c.to_uppercase().chain(chars).collect::<String>()).unwrap_or_default()
                });
                // The offending source line: live from its tab when the file
                // is open, else as it was on disk when compiled.
                let source_line = e.file.as_ref().filter(|_| e.line > 0).and_then(|f| {
                    let line = match self.tabs.iter().find(|t| &t.path == f) {
                        Some(t) => t.content.lines().nth(e.line - 1),
                        None => self.error_lines.get(&(f.clone(), e.line)).map(String::as_str),
                    };
                    line.map(|l| l.trim().chars().take(140).collect::<String>())
                });
                let frame = egui::Frame::NONE.inner_margin(egui::Margin::symmetric(8, 5));
                let r = frame.show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 2.0;
                    ui.horizontal(|ui| {
                        icons::show(ui, mark, color, icons::SIZE);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if !place.is_empty() {
                                ui.label(RichText::new(&place).small().weak());
                            }
                            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                                ui.add(egui::Label::new(&summary).truncate());
                            });
                        });
                    });
                    if let Some(h) = hint.filter(|h| !h.is_empty()) {
                        ui.horizontal(|ui| {
                            ui.add_space(icons::SIZE + ui.spacing().item_spacing.x);
                            ui.add(egui::Label::new(RichText::new(h).small().weak()).wrap());
                        });
                    }
                });
                let r = ui.interact(r.response.rect, ui.id().with(("err", n)), egui::Sense::click());
                let hover = ui.ctx().animate_bool_with_time(r.id, r.hovered(), theme::MOTION);
                if hover > 0.0 {
                    ui.painter().rect_filled(r.rect, 4.0, ui.visuals().widgets.hovered.bg_fill.gamma_multiply(0.6 * hover));
                }
                // The details on hover: LaTeX's own words (to search the web)
                // and the line of the source.
                let r = r.on_hover_ui(|ui| {
                    ui.label(RichText::new(&original).monospace().small());
                    if let Some(src) = source_line.as_ref().filter(|s| !s.is_empty()) {
                        ui.label(RichText::new(format!("{} │ {src}", e.line)).monospace().small().weak());
                    }
                });
                crate::menu::context(&r, |ui| {
                    if ui.add(crate::menu::item(t("Copy LaTeX's message"))).clicked() {
                        ui.ctx().copy_text(e.message.clone());
                        ui.close();
                    }
                });
                if e.file.is_some() && r.on_hover_cursor(egui::CursorIcon::PointingHand).clicked() {
                    go = Some((e.file.clone().unwrap(), e.line));
                }
                // A package the installation lacks: the command that installs it.
                if let Some((file, command)) = latex::missing_file(&e.message).and_then(|f| latex::install_command(&f).map(|c| (f, c))) {
                    ui.horizontal_wrapped(|ui| {
                        ui.add_space(30.0);
                        ui.label(RichText::new(tf!("To install {file}:", file)).small());
                        ui.label(RichText::new(&command).small().monospace().background_color(ui.visuals().code_bg_color));
                        if crate::widgets::copy_button(ui, ("copy-install", n)).clicked() {
                            ui.ctx().copy_text(command.clone());
                        }
                    });
                }
            }
        });
        if let Some((file, line)) = go {
            self.go_to(&file, line.max(1), true);
        }
    }

    fn activity_bar(&mut self, ui: &mut egui::Ui) {
        let mut selected_rect = None;
        ui.vertical_centered(|ui| {
            ui.add_space(4.0);
            for (view, icon, label) in SidebarView::ALL {
                let selected = self.s.sidebar_view == view && !self.s.sidebar_hidden;
                let button = egui::Button::selectable(selected, icon.image(egui::Color32::WHITE, 18.0)).image_tint_follows_text_color(true);
                let r = ui.add(button);
                if selected {
                    selected_rect = Some(r.rect);
                }
                if r.on_hover_text(t(label)).clicked() {
                    if selected {
                        self.s.sidebar_hidden = true;
                    } else {
                        self.s.sidebar_hidden = false;
                        self.s.sidebar_view = view;
                    }
                }
                ui.add_space(2.0);
            }
        });
        // A mark at the bar's edge glides to the view picked.
        let shown = ui.ctx().animate_bool_with_time(egui::Id::new("activity-mark"), selected_rect.is_some(), theme::MOTION);
        if let Some(rect) = selected_rect {
            let y = ui.ctx().animate_value_with_time(egui::Id::new("activity-mark-y"), rect.center().y, theme::MOTION);
            // In the gap between the window's edge and the buttons.
            let mark = egui::Rect::from_center_size(egui::pos2(rect.left() - 2.0, y), egui::vec2(2.0, rect.height() * 0.6));
            ui.painter().rect_filled(mark, 1.0, theme::accent().gamma_multiply(shown));
        }
    }

    fn preview_panel(&mut self, ui: &mut egui::Ui) {
        let fatal = self.errors.iter().filter(|e| !e.warning).count();
        let failed = matches!(self.last_compile, Some((false, _)));
        // Only a compilation that takes a while gets a spinner: a quick one
        // would just blink the header.
        let slow = self.compiling && self.now() - self.compile_started > SPINNER_AFTER;
        if self.compiling && !slow {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(SPINNER_AFTER));
        }
        // Where the preview stands relative to the text: each new state fades
        // in. Every state's animation runs each frame, so the first change
        // fades too.
        let state = if slow { 0 } else if failed { 1 } else if self.edited_since_compile { 2 } else if matches!(self.last_compile, Some((true, _))) { 3 } else { 4 };
        let mut shown = 1.0;
        for s in 0..5 {
            let k = ui.ctx().animate_bool_with_time(egui::Id::new(("preview-state", s)), s == state, theme::MOTION);
            if s == state {
                shown = k;
            }
        }
        self.preview.dark = self.s.pdf_dark;
        self.preview.fit_page = self.s.pdf_fit_page;
        self.preview.prepare(ui.ctx());
        let versions = self.s.folders.path(crate::settings::FolderKind::Versions).to_string_lossy().replace('\\', "/");
        // One row: the PDF's zoom and pages, where the compilation stands,
        // and the buttons. Hiding the preview is the editor's own button.
        crate::widgets::bar(ui, |ui| {
            if self.preview.has_pages() {
                self.preview.navigation(ui);
                ui.add_space(8.0);
            }
            // The words only where they fit beside the buttons.
            let room = ui.available_width() > 380.0;
            ui.scope(|ui| {
                ui.multiply_opacity(shown);
                if slow {
                    ui.add(egui::Spinner::new().size(12.0)).on_hover_text(t("Compiling…"));
                    if room {
                        ui.weak(t("Compiling…"));
                    }
                } else if failed {
                    let label = tn("{n} error", "{n} errors", fatal.max(1));
                    let b = icons::button(Icon::CircleX, RichText::new(label).color(ui.visuals().error_fg_color)).frame(false);
                    if ui.add(b).on_hover_text(t("Show/hide the diagnostics")).clicked() {
                        self.errors_open = !self.errors_open;
                    }
                } else if self.edited_since_compile {
                    let tip = if self.s.auto_compile { t("Compiles as soon as you stop typing") } else { t("Automatic compilation is off. Ctrl+S compiles") };
                    if room {
                        icons::label(ui, Icon::CircleDot, t("pending changes"), Some(ui.visuals().weak_text_color())).on_hover_text(tip);
                    } else {
                        icons::show(ui, Icon::CircleDot, ui.visuals().weak_text_color(), icons::SIZE).on_hover_text(tip);
                    }
                } else if let Some((true, at)) = &self.last_compile {
                    // The check grows into place, slightly.
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(icons::SIZE, icons::SIZE), egui::Sense::hover());
                    let side = icons::SIZE * (0.8 + 0.2 * shown);
                    Icon::CircleCheck.image(theme::SUCCESS, icons::SIZE).paint_at(ui, egui::Rect::from_center_size(rect.center(), egui::vec2(side, side)));
                    let updated = tf!("updated at {time}", time = at);
                    if room {
                        ui.weak(updated);
                    } else {
                        ui.interact(rect, ui.id().with("updated"), egui::Sense::hover()).on_hover_text(updated);
                    }
                }
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                crate::menu::dropdown(ui, icons::icon_button(Icon::EllipsisVertical), |ui| {
                    crate::menu::label(ui, t("Position"));
                    for (dock, label) in Dock::ALL {
                        if ui.add(crate::menu::item(t(label)).checked(self.s.preview_dock == dock)).clicked() {
                            self.s.preview_dock = dock;
                            ui.close();
                        }
                    }
                    crate::menu::separator(ui);
                    if ui.add(crate::menu::item(t("Compile automatically")).checked(self.s.auto_compile)).clicked() {
                        self.s.auto_compile = !self.s.auto_compile;
                    }
                    ui.add_enabled_ui(self.s.auto_compile, |ui| {
                        crate::menu::label(ui, t("Wait after typing stops"));
                        for (secs, label) in [(0.5, t("0.5 s")), (1.0, "1 s"), (2.0, "2 s"), (5.0, "5 s")] {
                            if ui.add(crate::menu::item(label).checked((self.s.compile_delay - secs).abs() < 0.01)).clicked() {
                                self.s.compile_delay = secs;
                            }
                        }
                    });
                    crate::menu::separator(ui);
                    if ui.add(crate::menu::item(t("Follow the cursor in the PDF")).checked(self.s.sync_follow)).clicked() {
                        self.s.sync_follow = !self.s.sync_follow;
                    }
                    if ui.add(crate::menu::item(t("Dark PDF")).checked(self.s.pdf_dark)).clicked() {
                        self.s.pdf_dark = !self.s.pdf_dark;
                    }
                })
                .0
                .on_hover_text(t("Preview options"));
                // Compiling controls only mean something with a project open.
                if self.root.is_some() {
                    let follow = ui
                        .add(toggle(ui, Icon::Crosshair, self.s.sync_follow))
                        .on_hover_text(t("Follow the cursor: the PDF scrolls to the passage you are editing"));
                    if follow.clicked() {
                        self.s.sync_follow = !self.s.sync_follow;
                        if self.s.sync_follow {
                            self.sync_to_preview();
                        }
                    }
                    let auto_icon = if self.s.auto_compile { Icon::Zap } else { Icon::ZapOff };
                    let auto = ui
                        .add(toggle(ui, auto_icon, self.s.auto_compile))
                        .on_hover_text(if self.s.auto_compile { t("Automatic compilation is on") } else { t("Automatic compilation is off") });
                    if auto.clicked() {
                        self.s.auto_compile = !self.s.auto_compile;
                    }
                    if ui.add(icons::icon_button(Icon::Play)).on_hover_text(t("Compile now (Ctrl+S)")).clicked() {
                        self.force_save_and_compile();
                    }
                }
                if self.preview.has_pages() {
                    ui.add_space(8.0);
                    self.preview.tools(ui, Some(&versions));
                }
            });
        });
        ui.separator();
        let area = ui.available_rect_before_wrap();
        self.preview_rect = Some(ui.max_rect());
        let event = self.preview.pages_view(ui);
        // A failed compilation leaves the last good PDF on screen: say so, in
        // a card floating over the bottom of the preview, so the pages do not
        // jump each time a compilation fails or succeeds while typing.
        // (Not while the diagnostics panel is open: it says it already.)
        // It rises a few pixels as it fades in.
        let card_shown = failed && self.preview.has_document() && !self.errors_open;
        let rise = ui.ctx().animate_bool_with_time(egui::Id::new("failed-card"), card_shown, theme::MOTION);
        if card_shown {
            let drop = 6.0 * (1.0 - rise);
            let card = egui::Rect::from_min_max(
                egui::pos2(area.left() + 10.0, area.bottom() - 40.0 + drop),
                egui::pos2(area.right() - 10.0, area.bottom() - 10.0 + drop),
            );
            ui.scope_builder(egui::UiBuilder::new().max_rect(card), |ui| {
                ui.multiply_opacity(rise);
                let fill = ui.visuals().window_fill;
                let stroke = egui::Stroke::new(1.0, ui.visuals().error_fg_color.gamma_multiply(0.6));
                egui::Frame::NONE
                    .fill(fill)
                    .stroke(stroke)
                    .shadow(ui.visuals().popup_shadow)
                    .inner_margin(egui::Margin::symmetric(10, 2))
                    .corner_radius(6)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            icons::show(ui, Icon::TriangleAlert, ui.visuals().error_fg_color, icons::SIZE);
                            let msg = tn("{n} error. This is the last PDF that compiled.", "{n} errors. This is the last PDF that compiled.", fatal.max(1));
                            ui.add(egui::Label::new(RichText::new(msg).small()).truncate());
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(t("Show errors")).clicked() {
                                    self.errors_open = true;
                                }
                            });
                        });
                    });
            });
        }
        // Nothing to show yet because the compilation failed: the way out.
        if failed && !self.preview.has_document() && !self.errors_open {
            let at = egui::Rect::from_center_size(area.center() + egui::vec2(0.0, 34.0), egui::vec2(200.0, 28.0));
            ui.scope_builder(egui::UiBuilder::new().max_rect(at), |ui| {
                ui.vertical_centered(|ui| {
                    if ui.add(icons::button(Icon::TriangleAlert, t("Show errors"))).on_hover_text("F8").clicked() {
                        self.run(Cmd::NextError(true));
                    }
                });
            });
        }
        self.s.pdf_fit_page = self.preview.fit_page;
        match event {
            Some(PdfEvent::SyncClick { page, x, y, words }) => self.sync_to_editor(page, x, y, words.as_deref(), false),
            Some(PdfEvent::Selected { page, x, y, text }) => self.sync_to_editor(page, x, y, Some(&text), true),
            Some(PdfEvent::ExportVersion) => {
                if let Some(p) = self.preview.path().map(|p| p.to_path_buf()) {
                    self.prompt_export_version(p);
                }
            }
            None => {}
        }
    }

    /// Zen mode's reading side: the PDF over the whole window, without its
    /// toolbar (the search bar shows while it is open). A click on the PDF
    /// goes back to the text, at the line clicked.
    fn zen_reader(&mut self, ui: &mut egui::Ui) {
        self.preview.dark = self.s.pdf_dark;
        self.preview.fit_page = self.s.pdf_fit_page;
        self.preview.prepare(ui.ctx());
        if self.preview.finding() {
            crate::widgets::bar(ui, |ui| {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| self.preview.tools(ui, None));
            });
        }
        self.preview_rect = Some(ui.max_rect());
        let event = self.preview.pages_view(ui);
        self.s.pdf_fit_page = self.preview.fit_page;
        match event {
            Some(PdfEvent::SyncClick { page, x, y, words }) => {
                self.set_zen_reading(false);
                self.sync_to_editor(page, x, y, words.as_deref(), false);
            }
            Some(PdfEvent::Selected { .. } | PdfEvent::ExportVersion) | None => {}
        }
    }

    fn tab_strip(&mut self, ui: &mut egui::Ui) {
        let mut activate = None;
        let mut close = None;
        let mut menu_action: Option<(usize, &'static str)> = None;
        // Tabs opened this frame widen the strip only next frame, so the
        // scroll to the active one is retried until it shows whole.
        let mut active_shown = true;
        let mut draw_tabs = |app: &mut App, ui: &mut egui::Ui| {
            // The active tab's border glides to the tab picked; drawn after
            // the tabs, relative to the first so scrolling does not drag it.
            let mut first_tab: Option<egui::Rect> = None;
            let mut active_tab: Option<egui::Rect> = None;
            for (i, tab) in app.tabs.iter().enumerate() {
                let active = Some(i) == app.active;
                // The unsaved mark fades in and out.
                let dirty = ui.ctx().animate_bool_with_time(egui::Id::new(("tab-dirty", &tab.path)), tab.dirty, theme::MOTION);
                let frame = egui::Frame::NONE
                    .fill(if active { ui.visuals().extreme_bg_color } else { ui.visuals().faint_bg_color })
                    .inner_margin(egui::Margin::symmetric(10, 2));
                let r = frame.show(ui, |ui| {
                    ui.horizontal(|ui| {
                        let color = if active { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
                        let mut atoms: Vec<egui::Atom<'_>> = Vec::new();
                        if tab.pinned {
                            atoms.push(Icon::Pin.tinted(color).into());
                        }
                        atoms.push(super::sidebar::kind_icon(tab.kind).tinted(color).into());
                        atoms.push(RichText::new(&tab.name).color(color).into());
                        if dirty > 0.0 {
                            atoms.push(RichText::new(" ●").color(color.gamma_multiply(dirty)).into());
                        }
                        let l = ui.add(egui::AtomLayout::new(egui::Atoms::from(atoms)).sense(egui::Sense::click()));
                        if !tab.pinned && ui.add(icons::icon_button(Icon::X).frame(false).min_size(egui::vec2(18.0, 18.0))).on_hover_text(t("Close (Ctrl+W)")).clicked() {
                            close = Some(i);
                        }
                        l
                    })
                    .inner
                });
                first_tab.get_or_insert(r.response.rect);
                if active {
                    active_tab = Some(r.response.rect);
                }
                let resp = r.inner.union(r.response).on_hover_text(project::relative(&tab.path, app.root.as_deref()));
                if resp.clicked() {
                    activate = Some(i);
                }
                if resp.middle_clicked() && !tab.pinned {
                    close = Some(i);
                }
                if active && app.scroll_active_tab {
                    resp.scroll_to_me(None);
                    let clip = ui.clip_rect();
                    active_shown = clip.contains_rect(resp.rect.shrink(1.0)) || resp.rect.width() >= clip.width();
                }
                crate::menu::context(&resp, |ui| {
                    let textable = tab.kind.editable() || tab.kind == Kind::Svg;
                    if ui.add(crate::menu::item(if tab.pinned { t("Unpin tab") } else { t("Pin tab") })).clicked() {
                        menu_action = Some((i, "pin"));
                        ui.close();
                    }
                    if textable && !tab.kind.editable() {
                        crate::menu::submenu(ui, t("Reopen with"), None, |ui| {
                            if ui.add(crate::menu::item(t("Text editor")).checked(tab.view == TabView::Editor)).clicked() {
                                menu_action = Some((i, "editor"));
                                ui.close();
                            }
                            if ui.add(crate::menu::item(t("Viewer")).checked(tab.view == TabView::Viewer)).clicked() {
                                menu_action = Some((i, "viewer"));
                                ui.close();
                            }
                        });
                    }
                    crate::menu::separator(ui);
                    for (label, action) in [
                        (t("Close"), "close"),
                        (t("Close others"), "close-others"),
                        (t("Close to the right"), "close-right"),
                        (t("Close all"), "close-all"),
                    ] {
                        let row = crate::menu::item(label);
                        let row = if action == "close" { row.shortcut("Ctrl+W") } else { row };
                        if ui.add(row).clicked() {
                            menu_action = Some((i, action));
                            ui.close();
                        }
                    }
                    crate::menu::separator(ui);
                    if ui.add_enabled(i > 0, crate::menu::item(t("Move left"))).clicked() {
                        menu_action = Some((i, "left"));
                        ui.close();
                    }
                    if ui.add_enabled(i + 1 < app.tabs.len(), crate::menu::item(t("Move right"))).clicked() {
                        menu_action = Some((i, "right"));
                        ui.close();
                    }
                    if project::is_document_source(&tab.path) && ui.add(crate::menu::item(t("Set as main file"))).clicked() {
                        menu_action = Some((i, "main"));
                        ui.close();
                    }
                    if tab.editable() && ui.add(crate::menu::item(t("Local history…"))).clicked() {
                        menu_action = Some((i, "history"));
                        ui.close();
                    }
                });
            }
            if let (Some(first), Some(rect)) = (first_tab, active_tab) {
                let ctx = ui.ctx().clone();
                let glide = |what: &str, to: f32| ctx.animate_value_with_time(egui::Id::new(("tab-border", what)), to, theme::MOTION);
                let min = first.min + egui::vec2(glide("x", rect.left() - first.left()), glide("y", rect.top() - first.top()));
                let size = egui::vec2(glide("w", rect.width()), glide("h", rect.height()));
                let stroke = egui::Stroke::new(1.0, ui.visuals().selection.bg_fill);
                ui.painter().rect_stroke(egui::Rect::from_min_size(min, size), 0.0, stroke, egui::StrokeKind::Inside);
            }
        };
        let mut listed = None;
        let row = |ui: &mut egui::Ui| {
            // Whether the tabs did not fit last frame: room is kept for the
            // list of them then, which keeps the answer from flickering.
            let overflow_id = egui::Id::new("tab-overflow");
            let had = ui.data(|d| d.get_temp::<bool>(overflow_id)).unwrap_or(false);
            let w = ui.available_width() - if had { 100.0 } else { 70.0 };
            let out = ui
                .scope(|ui| {
                    // A plain mouse wheel scrolls the tabs sideways.
                    ui.style_mut().always_scroll_the_only_direction = true;
                    // The bar gets a strip of its own under the tabs, there
                    // whenever some are out of sight, and does not swell
                    // over their names under the pointer.
                    let scroll = &mut ui.spacing_mut().scroll;
                    scroll.floating = true;
                    scroll.bar_width = 3.0;
                    scroll.floating_width = 3.0;
                    scroll.floating_allocated_width = 6.0;
                    scroll.bar_outer_margin = 0.0;
                    scroll.foreground_color = false;
                    scroll.dormant_background_opacity = 0.0;
                    scroll.active_background_opacity = 0.0;
                    scroll.interact_background_opacity = 0.0;
                    scroll.dormant_handle_opacity = 1.0;
                    scroll.active_handle_opacity = 1.0;
                    scroll.interact_handle_opacity = 1.0;
                    egui::ScrollArea::horizontal().id_salt("tab-scroll").max_width(w.max(50.0)).show(ui, |ui| {
                        ui.horizontal(|ui| draw_tabs(self, ui));
                    })
                })
                .inner;
            let overflow = out.content_size.x > out.inner_rect.width() + 1.0;
            super::navigate::claim_sideways(ui.ctx(), out.inner_rect, out.content_size.x);
            ui.data_mut(|d| d.insert_temp(overflow_id, overflow));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let label = if self.s.preview_hidden { t("Show preview (Ctrl+J)") } else { t("Hide preview (Ctrl+J)") };
                if ui.add(icons::icon_button(Icon::Columns)).on_hover_text(label).clicked() {
                    self.s.preview_hidden = !self.s.preview_hidden;
                }
                if overflow {
                    crate::menu::dropdown(ui, icons::icon_button(Icon::ChevronDown), |ui| {
                            ui.set_max_width(360.0);
                            egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                                for (i, tab) in self.tabs.iter().enumerate() {
                                    let icon = if tab.pinned { Icon::Pin } else { super::sidebar::kind_icon(tab.kind) };
                                    let name = if tab.dirty { format!("{} ●", tab.name) } else { tab.name.clone() };
                                    let r = ui
                                        .add(crate::menu::item(name).icon(icon).selected(Some(i) == self.active))
                                        .on_hover_text(project::relative(&tab.path, self.root.as_deref()));
                                    if r.clicked() {
                                        listed = Some(i);
                                        ui.close();
                                    }
                                }
                            });
                        })
                        .0
                        .on_hover_text(t("All open tabs"));
                }
            });
        };
        // Level with the sidebar's and the preview's headers.
        crate::widgets::bar(ui, row);
        self.scroll_active_tab &= !active_shown;
        if let Some(i) = activate.or(listed) {
            self.activate(i);
        }
        if let Some(i) = close {
            self.close_tab(i);
        }
        if let Some((i, action)) = menu_action {
            self.tab_menu_action(i, action);
        }
    }

    fn tab_menu_action(&mut self, i: usize, action: &str) {
        match action {
            "pin" => {
                self.tabs[i].pinned = !self.tabs[i].pinned;
                self.sort_pinned();
            }
            "editor" => self.set_tab_view(i, TabView::Editor),
            "viewer" => self.set_tab_view(i, TabView::Viewer),
            "close" => self.close_tab(i),
            "close-others" => {
                let keep = self.tabs[i].path.clone();
                let others: Vec<_> = self.tabs.iter().filter(|t| t.path != keep && !t.pinned).map(|t| t.path.clone()).collect();
                for p in others {
                    if let Some(j) = self.tab_index(&p) {
                        self.close_tab(j);
                    }
                }
            }
            "close-right" => {
                let right: Vec<_> = self.tabs[i + 1..].iter().filter(|t| !t.pinned).map(|t| t.path.clone()).collect();
                for p in right {
                    if let Some(j) = self.tab_index(&p) {
                        self.close_tab(j);
                    }
                }
            }
            "close-all" => self.run(Cmd::CloseAllTabs),
            "left" if i > 0 => {
                self.tabs.swap(i, i - 1);
                if self.active == Some(i) {
                    self.active = Some(i - 1);
                } else if self.active == Some(i - 1) {
                    self.active = Some(i);
                }
            }
            "right" if i + 1 < self.tabs.len() => {
                self.tabs.swap(i, i + 1);
                if self.active == Some(i) {
                    self.active = Some(i + 1);
                } else if self.active == Some(i + 1) {
                    self.active = Some(i);
                }
            }
            "main" => {
                let p = self.tabs[i].path.clone();
                self.set_main_file(p, false);
            }
            "history" => {
                self.activate(i);
                self.run(Cmd::LocalHistory);
            }
            _ => {}
        }
    }

    /// The start screen: open or create a project, recent projects, and
    /// whether a LaTeX toolchain was found.
    fn welcome(&mut self, ui: &mut egui::Ui) {
        let mut run = None;
        egui::ScrollArea::vertical().auto_shrink(false).show(ui, |ui| {
            ui.vertical_centered(|ui| {
                let first_run = self.s.recent.is_empty();
                ui.add_space((ui.available_height() * if first_run { 0.08 } else { 0.12 }).max(24.0));
                icons::show(ui, Icon::FileCode, theme::accent(), 44.0);
                ui.add_space(6.0);
                ui.label(RichText::new("TecTec").size(26.0).strong());
                if first_run {
                    ui.label(t("Write LaTeX and see the PDF take shape as you type."));
                    ui.add_space(2.0);
                    ui.weak(RichText::new(t("A project is a folder: your .tex files, the bibliography and the images, side by side.")).small());
                    ui.add_space(24.0);
                    if let Some(cmd) = first_steps(ui) {
                        run = Some(cmd);
                    }
                } else {
                    ui.weak(t("Write LaTeX with a live preview."));
                }
                if !first_run {
                    ui.add_space(18.0);
                    ui.horizontal(|ui| {
                        theme::framed(ui);
                        let w = 150.0 * 2.0 + ui.spacing().item_spacing.x;
                        ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
                        let open = icons::button(Icon::FolderOpen, RichText::new(t("Open project…")).strong())
                            .fill(theme::accent().gamma_multiply(0.35))
                            .min_size(egui::vec2(150.0, 32.0));
                        if ui.add(open).on_hover_text("Ctrl+O").clicked() {
                            run = Some(Cmd::OpenProject);
                        }
                        crate::menu::dropdown(ui, icons::button(Icon::Plus, t("New project")), |ui| {
                            for t in crate::backend::templates::Template::ALL {
                                if ui.add(crate::menu::item(t.label())).clicked() {
                                    run = Some(Cmd::NewProject(t));
                                    ui.close();
                                }
                            }
                        });
                    });
                    ui.add_space(22.0);
                    ui.label(RichText::new(t("Recent")).small().weak());
                    ui.add_space(4.0);
                    for r in self.s.recent.clone() {
                        // One row: the name, where it is (short) and, under
                        // the pointer, a button that drops it from the list.
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(460.0, 30.0), egui::Sense::click());
                        let resp = resp.on_hover_text(r.path.display().to_string());
                        let painter = ui.painter();
                        let hovered = ui.rect_contains_pointer(rect);
                        if hovered {
                            painter.rect_filled(rect, 5.0, ui.visuals().widgets.hovered.bg_fill);
                        }
                        let text = ui.visuals().text_color();
                        let weak = ui.visuals().weak_text_color();
                        icons::show_at(ui, Icon::Folder, egui::Rect::from_center_size(egui::pos2(rect.left() + 18.0, rect.center().y), egui::vec2(16.0, 16.0)), text);
                        painter.text(egui::pos2(rect.left() + 36.0, rect.center().y), egui::Align2::LEFT_CENTER, &r.name, egui::TextStyle::Body.resolve(ui.style()), text);
                        painter.text(egui::pos2(rect.right() - 36.0, rect.center().y), egui::Align2::RIGHT_CENTER, short_path(&r.path, 40), egui::TextStyle::Small.resolve(ui.style()), weak);
                        let mut removed = false;
                        if hovered {
                            let x = egui::Rect::from_center_size(egui::pos2(rect.right() - 16.0, rect.center().y), egui::vec2(24.0, 24.0));
                            let xr = ui.interact(x, ui.id().with(("forget", &r.path)), egui::Sense::click()).on_hover_text(t("Remove from the recent projects"));
                            if xr.hovered() {
                                ui.painter().rect_filled(x, 4.0, ui.visuals().widgets.active.bg_fill);
                            }
                            icons::show_at(ui, Icon::X, x, if xr.hovered() { text } else { weak });
                            if xr.clicked() {
                                removed = true;
                                run = Some(Cmd::ForgetRecent(r.path.clone()));
                            }
                        }
                        if resp.clicked() && !removed {
                            run = Some(Cmd::OpenRecent(r.path.clone()));
                        }
                        crate::menu::context(&resp, |ui| {
                            if ui.add(crate::menu::item(t("Open"))).clicked() {
                                run = Some(Cmd::OpenRecent(r.path.clone()));
                                ui.close();
                            }
                            if ui.add(crate::menu::item(t("Reveal in the file manager"))).clicked() {
                                run = Some(Cmd::RevealRecent(r.path.clone()));
                                ui.close();
                            }
                            crate::menu::separator(ui);
                            if ui.add(crate::menu::item(t("Remove from the recent projects"))).clicked() {
                                run = Some(Cmd::ForgetRecent(r.path.clone()));
                                ui.close();
                            }
                        });
                    }
                    ui.add_space(4.0);
                    if ui.add(egui::Button::new(RichText::new(t("Clear the list")).small().weak()).frame(false)).clicked() {
                        run = Some(Cmd::ClearRecent);
                    }
                }
                ui.add_space(22.0);
                match &self.engine {
                    Some(_) if !latex::system_tex() => {
                        let ready = t("LaTeX built in: nothing to install.");
                        ui.horizontal(|ui| centered_line(ui, Icon::CircleCheck, ready, theme::SUCCESS));
                    }
                    Some(e) if e.available => {
                        let found = tf!("LaTeX found: {engine}", engine = e.name);
                        ui.horizontal(|ui| centered_line(ui, Icon::CircleCheck, found, theme::SUCCESS));
                        if e.name != "latexmk" {
                            // A suggestion, not a success: its own line, its own colour.
                            let tip = t("Install latexmk for sturdier compilations (it runs BibTeX and the extra passes).");
                            let color = ui.visuals().warn_fg_color;
                            ui.horizontal(|ui| centered_line(ui, Icon::Info, tip, color));
                        }
                    }
                    Some(_) => {
                        let msg = t("No LaTeX compiler on the PATH. Install TeX Live (Linux), MacTeX (macOS) or MiKTeX (Windows).");
                        let color = ui.visuals().warn_fg_color;
                        ui.horizontal(|ui| centered_line(ui, Icon::TriangleAlert, msg, color));
                    }
                    None => {
                        ui.weak(RichText::new(t("Looking for LaTeX…")).small());
                    }
                }
                ui.add_space(8.0);
                ui.weak(RichText::new(t("Tip: Ctrl+Shift+P opens the palette with every command.")).small());
            });
        });
        if let Some(cmd) = run {
            self.run(cmd);
        }
    }

    /// A project is open but no file is: point at the main file and the
    /// ways to find the others.
    fn no_tab(&mut self, ui: &mut egui::Ui) {
        let mut open_main = false;
        let mut run = None;
        ui.vertical_centered(|ui| {
            ui.add_space(ui.available_height() * 0.3);
            icons::show(ui, Icon::FileText, ui.visuals().weak_text_color(), 36.0);
            ui.add_space(6.0);
            if let Some(main) = &self.main_file {
                if ui.button(tf!("Open {file}", file = project::file_name(main))).clicked() {
                    open_main = true;
                }
                ui.add_space(6.0);
            }
            for (label, accel, cmd) in [
                (t("Go to file"), "Ctrl+Shift+N", Cmd::GoToFile),
                (t("New file"), "Ctrl+N", Cmd::NewFile),
                (t("Command palette"), "Ctrl+Shift+P", Cmd::Palette),
            ] {
                if ui.add(egui::Button::new(RichText::new(format!("{label}   {accel}")).weak()).frame(false)).clicked() {
                    run = Some(cmd);
                }
            }
        });
        if open_main {
            if let Some(main) = self.main_file.clone() {
                self.open_file(&main, Kind::Tex);
            }
        }
        if let Some(cmd) = run {
            self.run(cmd);
        }
    }

    fn toolbar(&mut self, ui: &mut egui::Ui) {
        let mut chosen = None;
        crate::widgets::bar(ui, |ui| {
            for (i, (face, tip, _)) in TOOLS.iter().enumerate() {
                let button = match face {
                    // The first letter of the style's name in each language.
                    Face::Bold => egui::Button::new(RichText::new(t("B")).strong()),
                    Face::Italic => egui::Button::new(RichText::new(t("I")).italics()),
                    Face::Underline => egui::Button::new(RichText::new(t("U")).underline()),
                    Face::Icon(icon) => icons::icon_button(*icon),
                    Face::Text(text) => egui::Button::new(*text),
                };
                if ui.add(button).on_hover_text(t(tip)).clicked() {
                    chosen = Some(i);
                }
                if matches!(i, 2 | 6 | 8) {
                    ui.separator();
                }
            }
        });
        if let Some(i) = chosen {
            match &TOOLS[i].2 {
                Tool::Surround(a, b) => self.surround(a, b),
                Tool::Block(t) => self.insert_block(t),
                Tool::Table => self.run(Cmd::InsertTable),
                Tool::List => self.run(Cmd::InsertList),
                Tool::Math => self.run(Cmd::InsertMath),
                Tool::Image => self.run(Cmd::InsertImage),
                Tool::Find => self.run(Cmd::Find),
                Tool::PasteImage => self.run(Cmd::PasteImage),
            }
        }
    }

    /// For the main file of a document that cites but prints no
    /// bibliography: the lines that print it, fit for what the preamble loads
    /// and naming the project's .bib files.
    fn missing_bibliography(&self, path: &std::path::Path) -> Option<String> {
        let doc = self.doc.as_ref().filter(|d| d.cites && !d.bibliography)?;
        if self.main_file.as_deref() != Some(path) {
            return None;
        }
        if doc.biblatex {
            return Some("\\printbibliography\n".into());
        }
        let base = path.parent()?;
        let mut files = Vec::new();
        if let Some(tree) = &self.tree {
            tree.files(&mut files);
        }
        let mut bibs: Vec<String> = files
            .iter()
            .filter(|f| project::ext_lower(&f.path) == "bib")
            .filter_map(|f| f.path.strip_prefix(base).ok().map(|p| p.with_extension("").to_string_lossy().replace('\\', "/")))
            .collect();
        bibs.sort();
        let names = if bibs.is_empty() { "refs".to_string() } else { bibs.join(",") };
        // abntex2cite sets the style itself.
        let style = if doc.abnt_citations { String::new() } else { "\\bibliographystyle{plain}\n".into() };
        Some(format!("{style}\\bibliography{{{names}}}\n"))
    }

    /// What to do about a compilation message. A citation left undefined
    /// because the document prints no bibliography says so, whatever the key.
    fn diagnostic_hint(&self, message: &str) -> Option<String> {
        let no_bibliography = self.doc.as_ref().is_some_and(|d| d.cites && !d.bibliography);
        if no_bibliography && message.to_lowercase().contains("citation") {
            return Some(t("The document prints no bibliography, so no citation resolves: in the main file, Ctrl+. on \\end{document} inserts it.").into());
        }
        latex::hint(message).map(String::from)
    }

    /// Where the cursor is in the document (`Introduction › Method`); a
    /// click lists every heading to jump to. Returns the line chosen.
    fn breadcrumb(&self, ui: &mut egui::Ui) -> Option<(PathBuf, usize)> {
        let (rows, here) = self.outline_rows();
        if rows.is_empty() {
            return None;
        }
        let mut path: Vec<&str> = Vec::new();
        if let Some(i) = here {
            let mut level = usize::MAX;
            for r in rows[..=i].iter().rev() {
                if r.entry.level < level {
                    path.push(&r.entry.title);
                    level = r.entry.level;
                }
            }
            path.reverse();
        }
        let weak = ui.visuals().weak_text_color();
        let label = if path.is_empty() { t("Beginning of the document").to_string() } else { path.join("  ›  ") };
        let min_level = rows.iter().map(|r| r.entry.level).min().unwrap_or(0);
        let mut go = None;
        let button = egui::Button::new((Icon::ListTree.tinted(weak), RichText::new(label).color(weak))).frame(false).truncate();
        ui.scope(|ui| {
            // Up to half the status bar; its messages and counts need the rest.
            ui.set_max_width((ui.available_width() * 0.5).min(720.0));
            crate::menu::dropdown(ui, button, |ui| {
                ui.set_max_width(420.0);
                egui::ScrollArea::vertical().max_height(420.0).show(ui, |ui| {
                    for (i, r) in rows.iter().enumerate() {
                        let e = &r.entry;
                        let text = if e.level <= min_level { RichText::new(&e.title).strong() } else { RichText::new(&e.title) };
                        let row = crate::menu::item(text).indent((e.level - min_level) as f32 * 14.0).selected(here == Some(i));
                        if ui.add(row).clicked() {
                            go = Some((r.file.clone(), e.line));
                            ui.close();
                        }
                    }
                });
            });
        })
        .response
        .on_hover_text(t("Sections of this file"));
        go
    }

    fn editor_pane(&mut self, ui: &mut egui::Ui) {
        if !self.zen && self.root.is_some() {
            self.tab_strip(ui);
            if self.active_tab().map(|t| t.is_tex()).unwrap_or(false) {
                self.toolbar(ui);
            }
            ui.separator();
        }
        let Some(i) = self.active else {
            if self.root.is_none() {
                self.welcome(ui);
            } else {
                self.no_tab(ui);
            }
            return;
        };

        let view = self.tabs[i].view;
        let kind = self.tabs[i].kind;
        if view == TabView::Editor {
            let spell_on = self.s.spell_check;
            let diagnostics: Vec<editor::Diagnostic> = self
                .errors
                .iter()
                .filter(|e| e.line > 0 && e.file.as_deref() == Some(self.tabs[i].path.as_path()))
                .map(|e| editor::Diagnostic {
                    line: e.line,
                    warning: e.warning,
                    message: latex::summary(&e.message).unwrap_or_else(|| e.message.clone()),
                    hint: self.diagnostic_hint(&e.message),
                })
                .collect();
            let bibliography = self.missing_bibliography(&self.tabs[i].path);
            let t = &mut self.tabs[i];
            let lang = t.lang();
            let env = EditorEnv {
                completions: &self.completions,
                spell: if spell_on { Some(&mut self.spell) } else { None },
                math: Some(&mut self.math),
                dark: self.s.dark,
                typewriter: self.s.typewriter,
                review: self.s.review,
                // Zen has its own column.
                measure: self.s.comfortable_width && !self.zen,
                review_context: crate::editor::lint::Context { bibliography, t1_bitmap: latex::t1_bitmap() },
                lang,
                font_size: self.s.font_size,
                proportional: self.s.editor_font == crate::settings::EditorFont::Proportional,
                diagnostics: &diagnostics,
            };
            // Zen: a centred column of comfortable width instead of the
            // whole window.
            let resp = if self.zen {
                let font = egui::FontId::monospace(self.s.font_size);
                let char_w = ui.ctx().fonts_mut(|f| f.glyph_width(&font, '0'));
                let full = ui.available_rect_before_wrap();
                let col = (char_w * ZEN_COLUMN_CHARS + 70.0).min(full.width());
                let x = full.center().x;
                let rect = egui::Rect::from_x_y_ranges((x - col / 2.0)..=(x + col / 2.0), (full.top() + 24.0)..=full.bottom());
                let mut column = ui.new_child(egui::UiBuilder::new().max_rect(rect));
                editor::show(&mut column, &mut t.content, &mut t.editor, env)
            } else {
                editor::show(ui, &mut t.content, &mut t.editor, env)
            };
            if resp.changed {
                self.on_edit();
            }
            if let Some(line) = resp.cursor_line_changed {
                self.on_cursor_line(line);
            }
            if let Some((rel, at)) = resp.dropped_image {
                self.insert_image(&rel, Some(at));
            }
            if let Some(at) = resp.paste_image_at {
                self.paste_image(at);
            }
            let ctx = ui.ctx().clone();
            self.hover_card(&ctx, resp.hover);
            if let Some(target) = resp.goto {
                self.go_to_definition(target);
            }
            return;
        }
        match kind {
            Kind::Pdf => {
                let event = self.tabs[i].pdf.as_mut().and_then(|v| v.show(ui, None));
                if let Some(PdfEvent::SyncClick { .. }) = event {
                    // A paper or other PDF has no SyncTeX map of its own.
                }
            }
            Kind::Image | Kind::Svg => {
                let uri = format!("file://{}", self.tabs[i].path.display());
                egui::ScrollArea::both().auto_shrink(false).show(ui, |ui| {
                    ui.add(egui::Image::new(uri).max_width(ui.available_width()).fit_to_original_size(1.0).shrink_to_fit());
                });
            }
            _ => {
                ui.centered_and_justified(|ui| {
                    ui.weak(tf!("{file} is a binary file and cannot be opened as text.", file = self.tabs[i].name));
                });
            }
        }
    }
}

/// "1,234 words", or "1,234 words · 45,678 in the document" for a file of a
/// document split in several.
fn words_label(words: usize) -> String {
    tn("{n} word", "{n} words", words).replace(&words.to_string(), &crate::i18n::number(words))
}

/// A path for a list: `~` for the home folder, and cut in the middle when
/// longer than `max` characters, keeping its start and its end.
fn short_path(path: &std::path::Path, max: usize) -> String {
    let mut text = path.display().to_string();
    if let Some(home) = std::env::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            text = format!("~/{}", rest.display());
        }
    }
    let chars: Vec<char> = text.chars().collect();
    if chars.len() <= max {
        return text;
    }
    let keep = max.saturating_sub(1);
    let head: String = chars[..keep / 2].iter().collect();
    let tail: String = chars[chars.len() - (keep - keep / 2)..].iter().collect();
    format!("{head}…{tail}")
}

/// An on/off icon in a toolbar: the accent when on, faint when off, with no
/// filled block either way.
fn toggle<'a>(ui: &egui::Ui, icon: Icon, on: bool) -> egui::Button<'a> {
    let color = if on { theme::accent() } else { ui.visuals().weak_text_color() };
    let side = crate::widgets::ICON_BUTTON;
    egui::Button::new(icon.tinted(color)).min_size(egui::vec2(side, side))
}

/// A line of the start screen: an icon and a small text, centred.
fn centered_line(ui: &mut egui::Ui, icon: Icon, text: impl Into<String>, color: egui::Color32) {
    let text = RichText::new(text.into()).small().color(color);
    let galley = egui::WidgetText::from(text.clone()).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, egui::TextStyle::Small);
    let w = galley.size().x + icons::SIZE + ui.spacing().item_spacing.x;
    ui.add_space(((ui.available_width() - w) / 2.0).max(0.0));
    icons::show(ui, icon, color, icons::SIZE);
    ui.label(text);
}

/// Width of the start screen's column of choices.
const START_COLUMN: f32 = 460.0;

/// The start screen before any project: one card per template, then the
/// way to open a folder that already exists. Returns what was chosen.
fn first_steps(ui: &mut egui::Ui) -> Option<Cmd> {
    use crate::backend::templates::Template;
    let mut run = None;
    let gap = 8.0;
    let card = egui::vec2((START_COLUMN - gap) / 2.0, 66.0);
    let column = |ui: &mut egui::Ui, add: &mut dyn FnMut(&mut egui::Ui)| {
        ui.allocate_ui_with_layout(egui::vec2(START_COLUMN, 0.0), egui::Layout::top_down(egui::Align::Min), |ui| {
            ui.set_min_width(START_COLUMN);
            add(ui)
        });
    };
    column(ui, &mut |ui| {
        ui.label(RichText::new(t("Start a new project")).small().weak());
        ui.add_space(4.0);
    });
    let cards = [
        (Template::Abnt, Icon::Library, t("ABNT academic work"), t("TCC, dissertation or thesis by the Brazilian standards")),
        (Template::Article, Icon::FileText, t("Article"), t("A paper or a short text with sections")),
        (Template::Report, Icon::BookOpenText, t("Report or thesis"), t("A long text divided into chapters")),
        (Template::Beamer, Icon::Monitor, t("Presentation"), t("Slides for a talk, with beamer")),
    ];
    for row in cards.chunks(2) {
        ui.allocate_ui_with_layout(egui::vec2(START_COLUMN, card.y), egui::Layout::left_to_right(egui::Align::Min), |ui| {
            ui.spacing_mut().item_spacing.x = gap;
            for &(template, icon, title, blurb) in row {
                if template_card(ui, card, icon, title, blurb).clicked() {
                    run = Some(Cmd::NewProject(template));
                }
            }
        });
        ui.add_space(gap);
    }
    ui.add_space(14.0);
    column(ui, &mut |ui| {
        ui.label(RichText::new(t("Or continue a project you already have")).small().weak());
        ui.add_space(4.0);
        theme::framed(ui);
        let open = icons::button(Icon::FolderOpen, t("Open folder…")).right_text(RichText::new("Ctrl+O").weak()).min_size(egui::vec2(START_COLUMN, 34.0));
        if ui.add(open).clicked() {
            run = Some(Cmd::OpenProject);
        }
        ui.add_space(4.0);
        ui.weak(RichText::new(t("You can also drop the folder, or one of its .tex files, on this window.")).small());
    });
    run
}

/// A clickable card for a template: an icon, a name and what it is for.
fn template_card(ui: &mut egui::Ui, size: egui::Vec2, icon: Icon, title: &str, blurb: &str) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(size, egui::Sense::click());
    let resp = resp.on_hover_cursor(egui::CursorIcon::PointingHand);
    let visuals = ui.visuals();
    let hovered = resp.hovered() || resp.has_focus();
    let fill = if hovered { visuals.widgets.hovered.weak_bg_fill } else { visuals.faint_bg_color };
    let edge = if hovered { theme::accent().gamma_multiply(0.8) } else { visuals.widgets.noninteractive.bg_stroke.color };
    let painter = ui.painter();
    painter.rect(rect, 8.0, fill, egui::Stroke::new(1.0, edge), egui::StrokeKind::Inside);
    let pad = 14.0;
    let icon_rect = egui::Rect::from_min_size(rect.min + egui::vec2(pad, pad), egui::vec2(18.0, 18.0));
    icon.image(theme::accent(), 18.0).paint_at(ui, icon_rect);
    let text_left = icon_rect.right() + 10.0;
    let wrap = rect.right() - pad - text_left;
    let text = visuals.text_color();
    let weak = visuals.weak_text_color();
    let name = painter.layout_no_wrap(title.to_owned(), egui::TextStyle::Body.resolve(ui.style()), text);
    let title_pos = egui::pos2(text_left, icon_rect.center().y - name.size().y / 2.0);
    let title_h = name.size().y;
    painter.galley(title_pos, name, text);
    let blurb = painter.layout(blurb.to_owned(), egui::TextStyle::Small.resolve(ui.style()), weak, wrap);
    painter.galley(egui::pos2(text_left, title_pos.y + title_h + 3.0), blurb, weak);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, title));
    resp
}

#[cfg(test)]
mod tests {
    #[test]
    fn short_paths() {
        let home = std::env::home_dir().unwrap();
        assert_eq!(super::short_path(&home.join("tese"), 48), "~/tese");
        let long = std::path::Path::new("/tmp/projetos/um-nome-de-pasta-bem-comprido/capitulos/tese");
        let short = super::short_path(long, 30);
        assert_eq!(short.chars().count(), 30);
        assert_eq!(short, "/tmp/projetos/…/capitulos/tese");
    }
}

fn menu_item(ui: &mut egui::Ui, e: MenuEntry, run: &mut Option<Cmd>) {
    let mut row = crate::menu::item(&e.label).checked(e.checked == Some(true));
    if let Some(a) = e.accel {
        row = row.shortcut(a);
    }
    let clicked = ui.add_enabled(e.enabled, row).clicked();
    if clicked {
        *run = Some(e.cmd);
        ui.close();
    }
}

/// How long the glow behind a changed error count lasts, in seconds.
const COUNT_GLOW: f64 = 0.3;

/// From 1 down to 0 in the moment after `count` changes: the error count in
/// the status bar glows briefly when a compilation brings a new one.
fn count_changed_glow(ui: &egui::Ui, count: (usize, usize)) -> f32 {
    let id = egui::Id::new("error-count-glow");
    let now = ui.input(|i| i.time);
    let at = ui.data_mut(|d| {
        let (last, at) = d.get_temp_mut_or_insert_with(id, || (count, now));
        if *last != count {
            *last = count;
            *at = now;
        }
        *at
    });
    let glow = (1.0 - (now - at) / COUNT_GLOW).clamp(0.0, 1.0) as f32;
    if glow > 0.0 {
        ui.ctx().request_repaint();
    }
    glow
}
