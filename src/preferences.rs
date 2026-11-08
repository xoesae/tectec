//! The preferences window, laid out after gpui-kit's Settings component
//! (https://gpui-kit.com/component/settings/): a sidebar with a search field
//! and the pages; beside it the page, with its title and a button
//! that puts the page back to the defaults, then its groups, each a muted
//! title over an outlined card of rows: title and description on the left,
//! the control on the right. Changes apply as they are made.
//!
//! The search matches the titles, descriptions and extra keywords of every
//! setting (and the titles of pages and groups), ignoring case and accents,
//! and shows only what matches.

use crate::i18n::{n, t, Language};
use crate::icons::Icon;
use crate::settings::{check_folder, Accent, Dock, EditorFont, FolderKind, Folders, Settings};
use crate::textmatch::folded;
use crate::tf;
use egui::{pos2, vec2, Align, Color32, CornerRadius, FontId, Key, Layout, Rect, Sense, Stroke, Ui, UiBuilder};

const SIDEBAR: f32 = 228.0;
const HEADER: f32 = 52.0;
const ROW: f32 = 30.0;
/// Room around the page's content and inside a group's card.
const PAD: f32 = 16.0;
const RADIUS: u8 = 8;

type Field = Box<dyn Fn(&mut Ui, &mut Settings)>;

/// Under a setting's title: an explanation, or what is wrong with its value.
struct Note {
    text: String,
    error: bool,
}

struct Item {
    title: &'static str,
    note: Box<dyn Fn(&Settings) -> Option<Note>>,
    keywords: Vec<&'static str>,
    disabled: fn(&Settings) -> bool,
    field: Field,
}

impl Item {
    fn new(title: &'static str, field: impl Fn(&mut Ui, &mut Settings) + 'static) -> Self {
        Self { title, note: Box::new(|_| None), keywords: vec![], disabled: |_| false, field: Box::new(field) }
    }

    /// A fixed description (marked with `n`, translated when shown).
    fn about(self, text: &'static str) -> Self {
        self.note(move |_| Some(Note { text: t(text).to_string(), error: false }))
    }

    fn note(mut self, note: impl Fn(&Settings) -> Option<Note> + 'static) -> Self {
        self.note = Box::new(note);
        self
    }

    /// More words the search finds it by, never shown.
    fn keywords(mut self, words: &[&'static str]) -> Self {
        self.keywords = words.to_vec();
        self
    }

    fn disabled_when(mut self, when: fn(&Settings) -> bool) -> Self {
        self.disabled = when;
        self
    }

    fn matches(&self, s: &Settings, query: &str) -> bool {
        let note = (self.note)(s).map(|n| n.text).unwrap_or_default();
        [t(self.title), &note].iter().any(|text| folded(text).contains(query))
            || self.keywords.iter().any(|k| folded(t(k)).contains(query) || folded(k).contains(query))
    }
}

struct Group {
    title: Option<&'static str>,
    items: Vec<Item>,
    /// Muted text under the card.
    footer: Option<&'static str>,
}

fn group(title: &'static str, items: Vec<Item>) -> Group {
    Group { title: Some(title), items, footer: None }
}

struct Page {
    title: &'static str,
    icon: Icon,
    groups: Vec<Group>,
    /// Puts this page's settings back to the defaults.
    reset: fn(&mut Settings, &Settings),
}

impl Page {
    /// Whether any of its settings is away from the default.
    fn changed(&self, s: &Settings) -> bool {
        let mut reset = s.clone();
        (self.reset)(&mut reset, &Settings::default());
        serde_json::to_value(&reset).ok() != serde_json::to_value(s).ok()
    }
}

fn combo<T: PartialEq + Copy>(ui: &mut Ui, id: &str, value: &mut T, choices: &[(T, &str)]) {
    let current = choices.iter().find(|(v, _)| v == value).map(|(_, l)| *l).unwrap_or("");
    egui::ComboBox::from_id_salt(id).width(190.0).selected_text(current).show_ui(ui, |ui| {
        for (v, label) in choices {
            ui.selectable_value(value, *v, *label);
        }
    });
}

/// The accent colours as round swatches, the chosen one ringed. Picking one
/// stops following the system. Laid out right to left, so the last first.
fn accent_swatches(ui: &mut Ui, accent: &mut Accent) {
    ui.spacing_mut().item_spacing.x = 2.0;
    let current = accent.color();
    for (a, name, [r, g, b]) in Accent::COLORS.into_iter().rev() {
        let color = Color32::from_rgb(r, g, b);
        let (rect, resp) = ui.allocate_exact_size(vec2(20.0, 22.0), Sense::click());
        let chosen = *accent == a || (*accent == Accent::System && current == color);
        let painter = ui.painter();
        painter.circle_filled(rect.center(), if resp.hovered() { 7.5 } else { 6.5 }, color);
        if chosen {
            painter.circle_stroke(rect.center(), 9.0, Stroke::new(1.5, ui.visuals().text_color()));
        }
        if resp.on_hover_text(t(name)).clicked() {
            *accent = a;
        }
    }
}

fn folder_item(kind: FolderKind, label: &'static str) -> Item {
    Item::new(label, move |ui, s| {
        let default = Folders::default().get(kind).to_string();
        let value = s.folders.get_mut(kind);
        if value.as_str() != default
            && ui.add(crate::icons::icon_button(Icon::Undo)).on_hover_text(tf!("Back to {folder}", folder = default.as_str())).clicked()
        {
            *value = default.clone();
        }
        ui.add(crate::widgets::field(value).hint_text(default.as_str()).desired_width(170.0));
    })
    .note(move |s| {
        // Until it is fixed, the default is used.
        check_folder(s.folders.get(kind)).err().map(|why| Note {
            text: tf!("{why} Using {folder}/ meanwhile.", why = t(why), folder = Folders::default().get(kind)),
            error: true,
        })
    })
}

fn pages() -> Vec<Page> {
    vec![
        Page {
            title: n("General"),
            icon: Icon::Settings,
            reset: |s, d| {
                s.language = d.language;
                s.dark = d.dark;
                s.accent = d.accent;
            },
            groups: vec![
                group(
                    n("Interface"),
                    vec![
                        Item::new(n("Language"), |ui, s| {
                            let mut choices = vec![(Language::System, t("Follow the system"))];
                            choices.extend(Language::CHOICES);
                            combo(ui, "pref-language", &mut s.language, &choices);
                        })
                        .about(n("Of the menus, dialogs and messages."))
                        .keywords(&["English", "Português", n("Translation")]),
                        Item::new(n("Theme"), |ui, s| combo(ui, "pref-theme", &mut s.dark, &[(false, t("Light")), (true, t("Dark"))]))
                            .keywords(&[n("Light"), n("Dark"), n("Appearance")]),
                    ],
                ),
                group(
                    n("Accent colour"),
                    vec![
                        Item::new(n("Colour"), |ui, s| accent_swatches(ui, &mut s.accent))
                            .about(n("Leading buttons, the cursor, selections and what is active.")),
                        Item::new(n("Follow the system"), |ui, s| {
                            let mut follow = s.accent == Accent::System;
                            if crate::widgets::switch(ui, &mut follow).changed() {
                                s.accent = if follow { Accent::System } else { Accent::Terracotta };
                            }
                        })
                        .note(|_| {
                            let text = match crate::settings::system_accent() {
                                Some(_) => t("Uses the colour chosen in the system settings (Appearance), and follows it when it changes."),
                                None => t("The system does not say its accent colour: the app's own is used."),
                            };
                            Some(Note { text: text.to_string(), error: false })
                        }),
                    ],
                ),
            ],
        },
        Page {
            title: n("Editor"),
            icon: Icon::Type,
            reset: |s, d| {
                s.editor_font = d.editor_font;
                s.font_size = d.font_size;
                s.comfortable_width = d.comfortable_width;
                s.typewriter = d.typewriter;
            },
            groups: vec![
                group(
                    n("Font"),
                    vec![
                        Item::new(n("Typeface"), |ui, s| {
                            combo(ui, "pref-font", &mut s.editor_font, &[(EditorFont::Monospace, t("Monospaced")), (EditorFont::Proportional, t("Proportional"))])
                        })
                        .about(n("Monospaced lines up markup and tables; proportional reads more easily in long prose.")),
                        Item::new(n("Size"), |ui, s| {
                            crate::widgets::stepper(ui, &mut s.font_size, 9.0..=32.0, 1.0, 0, " pt");
                        })
                        .keywords(&[n("Zoom")]),
                    ],
                ),
                group(
                    n("Layout"),
                    vec![
                        Item::new(n("Comfortable line width"), |ui, s| {
                            crate::widgets::switch(ui, &mut s.comfortable_width);
                        })
                        .about(n("Lines wrap at a comfortable length instead of at the window's edge.")),
                        Item::new(n("Typewriter mode"), |ui, s| {
                            crate::widgets::switch(ui, &mut s.typewriter);
                        })
                        .about(n("The line being written stays in the middle of the screen.")),
                    ],
                ),
            ],
        },
        Page {
            title: n("Writing"),
            icon: Icon::SpellCheck,
            reset: |s, d| {
                s.review = d.review;
                s.spell_check = d.spell_check;
            },
            groups: vec![group(
                n("Checks"),
                vec![
                    Item::new(n("Writing review"), |ui, s| {
                        crate::widgets::switch(ui, &mut s.review);
                    })
                    .about(n("Underlines small, certain mistakes in the prose (repeated words, spacing, quotes), each with its fix."))
                    .keywords(&[n("Suggestions"), "lint"]),
                    Item::new(n("Spell check"), |ui, s| {
                        crate::widgets::switch(ui, &mut s.spell_check);
                    })
                    .about(n("Underlines misspelled words, in the document's own language."))
                    .keywords(&[n("Dictionary"), n("Spelling")]),
                ],
            )],
        },
        Page {
            title: n("Compilation"),
            icon: Icon::Play,
            reset: |s, d| {
                s.system_tex = d.system_tex;
                s.auto_compile = d.auto_compile;
                s.compile_delay = d.compile_delay;
            },
            groups: vec![
                group(
                    "LaTeX",
                    vec![Item::new(n("Distribution"), |ui, s| {
                        combo(ui, "pref-tex", &mut s.system_tex, &[(false, t("Built-in")), (true, t("Installed on the system"))])
                    })
                    .note(|s| {
                        let text = if s.system_tex {
                            t("TeX Live or MiKTeX from the computer: pdfLaTeX, LuaLaTeX and biber.")
                        } else {
                            t("Comes with the app: nothing to install. Packages it lacks are downloaded once.")
                        };
                        Some(Note { text: text.to_string(), error: false })
                    })
                    .keywords(&["TeX Live", "MiKTeX", "pdfLaTeX", "LuaLaTeX", "XeTeX", n("Engine")])],
                ),
                group(
                    n("Automatic compilation"),
                    vec![
                        Item::new(n("Compile automatically"), |ui, s| {
                            crate::widgets::switch(ui, &mut s.auto_compile);
                        })
                        .about(n("Compiles by itself after a pause in typing.")),
                        Item::new(n("Wait after typing stops"), |ui, s| {
                            crate::widgets::stepper(ui, &mut s.compile_delay, 0.5..=5.0, 0.5, 1, " s");
                        })
                        .keywords(&[n("Delay")])
                        .disabled_when(|s| !s.auto_compile),
                    ],
                ),
            ],
        },
        Page {
            title: n("Preview"),
            icon: Icon::Monitor,
            reset: |s, d| {
                s.preview_dock = d.preview_dock;
                s.sync_follow = d.sync_follow;
                s.pdf_dark = d.pdf_dark;
                s.pdf_fit_page = d.pdf_fit_page;
            },
            groups: vec![
                group(
                    n("Placement"),
                    vec![Item::new(n("Position"), |ui, s| {
                        let choices: Vec<(Dock, &str)> = Dock::ALL.iter().map(|(d, l)| (*d, t(l))).collect();
                        combo(ui, "pref-dock", &mut s.preview_dock, &choices);
                    })
                    .about(n("Where the PDF sits next to the editor."))
                    .keywords(&[n("Right"), n("Left"), n("Top"), n("Bottom")])],
                ),
                group(
                    "PDF",
                    vec![
                        Item::new(n("Follow the cursor in the PDF"), |ui, s| {
                            crate::widgets::switch(ui, &mut s.sync_follow);
                        })
                        .about(n("Scrolls the preview to the cursor's position as it moves."))
                        .keywords(&["SyncTeX"]),
                        Item::new(n("Dark PDF"), |ui, s| {
                            crate::widgets::switch(ui, &mut s.pdf_dark);
                        })
                        .about(n("Shows the pages with inverted, warm-tinted colours.")),
                        Item::new(n("Fit whole pages"), |ui, s| {
                            crate::widgets::switch(ui, &mut s.pdf_fit_page);
                        })
                        .about(n("Instead of fitting the page width."))
                        .keywords(&[n("Zoom")]),
                    ],
                ),
            ],
        },
        Page {
            title: n("Project folders"),
            icon: Icon::Folder,
            reset: |s, d| s.folders = d.folders.clone(),
            groups: vec![Group {
                title: None,
                items: FolderKind::ALL.into_iter().map(|(kind, label)| folder_item(kind, label)).collect(),
                footer: Some(n("Where what you add to a project goes. The sidebar still lists every file of each kind, wherever it is.")),
            }],
        },
    ]
}

/// Which groups of each page, and which of their items, match the search.
type Visible = Vec<Vec<(usize, Vec<usize>)>>;

fn filter(pages: &[Page], s: &Settings, query: &str) -> Visible {
    let query = folded(query.trim());
    pages
        .iter()
        .map(|page| {
            let page_hit = query.is_empty() || folded(t(page.title)).contains(&query);
            page.groups
                .iter()
                .enumerate()
                .filter_map(|(gi, g)| {
                    let group_hit = page_hit || g.title.is_some_and(|title| folded(t(title)).contains(&query));
                    let items: Vec<usize> = (0..g.items.len()).filter(|&i| group_hit || g.items[i].matches(s, &query)).collect();
                    (!items.is_empty()).then_some((gi, items))
                })
                .collect()
        })
        .collect()
}

pub struct Preferences {
    page: usize,
    query: String,
    focus_search: bool,
}

impl Preferences {
    pub fn new() -> Self {
        Self { page: 0, query: String::new(), focus_search: true }
    }

    /// Draws the window; returns whether it stays open.
    pub fn show(&mut self, ctx: &egui::Context, s: &mut Settings) -> bool {
        let pages = pages();
        let visible = filter(&pages, s, &self.query);
        // A search that leaves the page empty moves to the first that matches.
        if visible[self.page].is_empty() {
            if let Some(first) = visible.iter().position(|g| !g.is_empty()) {
                self.page = first;
            }
        }

        let screen = ctx.content_rect();
        let size = vec2(860.0f32.min(screen.width() - 48.0), 580.0f32.min(screen.height() - 48.0));
        let mut open = true;
        let frame = egui::Frame::window(&ctx.global_style()).inner_margin(0);
        egui::Window::new(t("Preferences"))
            .id(egui::Id::new("preferences"))
            .title_bar(false)
            .collapsible(false)
            .resizable(false)
            .frame(frame)
            .fixed_size(size)
            .anchor(egui::Align2::CENTER_CENTER, vec2(0.0, 0.0))
            .show(ctx, |ui| {
                let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
                let side = Rect::from_min_max(rect.min, pos2(rect.left() + SIDEBAR, rect.bottom()));
                let main = Rect::from_min_max(pos2(side.right(), rect.top()), rect.max);
                let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
                let r = RADIUS;
                ui.painter().rect_filled(side, CornerRadius { nw: r, sw: r, ne: 0, se: 0 }, ui.visuals().panel_fill);
                ui.painter().vline(side.right(), side.y_range(), Stroke::new(1.0, edge));

                let mut sidebar = ui.new_child(UiBuilder::new().max_rect(side.shrink(12.0)).layout(Layout::top_down(Align::Min)));
                self.sidebar(&mut sidebar, &pages, &visible);
                let mut page = ui.new_child(UiBuilder::new().max_rect(main).layout(Layout::top_down(Align::Min)));
                crate::theme::framed(&mut page);
                page.spacing_mut().button_padding = egui::vec2(10.0, 4.0);
                if !self.page(&mut page, &pages, &visible, s) {
                    open = false;
                }
            });

        // Escape clears the search first, then closes; an open list closes
        // itself.
        if !egui::Popup::is_any_open(ctx) && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, Key::Escape)) {
            if self.query.is_empty() {
                open = false;
            } else {
                self.query.clear();
            }
        }
        open
    }

    fn sidebar(&mut self, ui: &mut Ui, pages: &[Page], visible: &Visible) {
        // The search field, with its magnifier inside.
        let field = crate::widgets::field(&mut self.query)
            .hint_text(t("Search settings"))
            .margin(egui::Margin { left: 30, right: 8, top: 6, bottom: 6 })
            .desired_width(ui.available_width());
        let r = ui.add(field);
        if std::mem::take(&mut self.focus_search) {
            r.request_focus();
        }
        let weak = ui.visuals().weak_text_color();
        let glass = Rect::from_center_size(pos2(r.rect.left() + 16.0, r.rect.center().y), vec2(14.0, 14.0));
        Icon::Search.tinted(weak).paint_at(ui, glass);
        ui.add_space(10.0);

        egui::ScrollArea::vertical().id_salt("prefs-sidebar").auto_shrink([false, false]).show(ui, |ui| {
            ui.spacing_mut().item_spacing.y = 2.0;
            for (pi, page) in pages.iter().enumerate() {
                if !visible[pi].is_empty() && nav_row(ui, page.icon, t(page.title), pi == self.page).clicked() {
                    self.page = pi;
                }
            }
        });
    }

    /// The page's header and its groups. Returns false when closed.
    fn page(&mut self, ui: &mut Ui, pages: &[Page], visible: &Visible, s: &mut Settings) -> bool {
        let mut open = true;
        let full = ui.max_rect();
        let header = Rect::from_min_size(full.min, vec2(full.width(), HEADER));
        let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
        ui.painter().hline(header.x_range(), header.bottom(), Stroke::new(1.0, edge));
        let page = &pages[self.page];
        let any = visible.iter().any(|g| !g.is_empty());

        let mut bar = ui.new_child(UiBuilder::new().max_rect(header.shrink2(vec2(PAD + 4.0, 0.0))).layout(Layout::left_to_right(Align::Center)));
        if any {
            bar.label(egui::RichText::new(t(page.title)).font(fastframe_fonts::Weight::SemiBold.font_id(16.0)).color(bar.visuals().strong_text_color()));
        }
        bar.with_layout(Layout::right_to_left(Align::Center), |bar| {
            bar.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
            bar.visuals_mut().widgets.inactive.bg_stroke = Stroke::NONE;
            if bar.add(crate::icons::icon_button(Icon::X)).on_hover_text(t("Close")).clicked() {
                open = false;
            }
            if any && page.changed(s) {
                if bar.add(crate::icons::icon_button(Icon::Undo)).on_hover_text(t("Reset this page to the defaults")).clicked() {
                    (page.reset)(s, &Settings::default());
                }
            }
        });

        let body = Rect::from_min_max(pos2(full.left(), header.bottom() + 1.0), full.max);
        let mut ui = ui.new_child(UiBuilder::new().max_rect(body).layout(Layout::top_down(Align::Min)));
        if !any {
            no_results(&mut ui, body);
            return open;
        }

        egui::ScrollArea::vertical().id_salt(("prefs-page", self.page)).auto_shrink([false, false]).show(&mut ui, |ui| {
            egui::Frame::new().inner_margin(egui::Margin { left: PAD as i8 + 4, right: PAD as i8 + 4, top: 4, bottom: PAD as i8 * 2 }).show(ui, |ui| {
                ui.set_width(ui.available_width());
                for (gi, items) in &visible[self.page] {
                    ui.add_space(PAD);
                    group_box(ui, &page.groups[*gi], items, s);
                }
            });
        });
        open
    }
}

/// A sidebar entry: the page's icon and title.
fn nav_row(ui: &mut Ui, icon: Icon, title: &str, active: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(ui.available_width(), ROW), Sense::click());
    let w = &ui.visuals().widgets;
    let hover = ui.ctx().animate_bool_with_time(resp.id.with("hover"), resp.hovered(), crate::theme::MOTION);
    if active {
        ui.painter().rect_filled(rect, 6.0, w.inactive.bg_fill);
    } else if hover > 0.0 {
        ui.painter().rect_filled(rect, 6.0, w.inactive.bg_fill.gamma_multiply(0.35 * hover));
    }
    let color = if active { ui.visuals().strong_text_color() } else { ui.visuals().text_color() };
    let tint = if active { crate::theme::accent() } else { ui.visuals().weak_text_color() };
    let left = rect.left() + 10.0;
    icon.tinted(tint).paint_at(ui, Rect::from_center_size(pos2(left + 7.0, rect.center().y), vec2(14.0, 14.0)));
    let x = left + 24.0;
    let font = if active { fastframe_fonts::Weight::SemiBold.font_id(13.5) } else { egui::TextStyle::Body.resolve(ui.style()) };
    let galley = ui.fonts_mut(|f| f.layout(title.to_string(), font, color, rect.right() - x - 6.0));
    ui.painter().with_clip_rect(rect).galley(pos2(x, rect.center().y - galley.size().y / 2.0), galley, color);
    resp
}

/// A group: its muted title, the outlined card of its rows, and the footer.
fn group_box(ui: &mut Ui, group: &Group, items: &[usize], s: &mut Settings) {
    let weak = ui.visuals().weak_text_color();
    if let Some(title) = group.title {
        ui.label(egui::RichText::new(t(title)).color(weak));
        ui.add_space(6.0);
    }
    let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
    egui::Frame::new().stroke(Stroke::new(1.0, edge)).corner_radius(RADIUS).inner_margin(PAD as i8).show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.spacing_mut().item_spacing.y = PAD;
        for &i in items {
            item_row(ui, &group.items[i], s);
        }
    });
    if let Some(footer) = group.footer {
        ui.add_space(8.0);
        ui.label(egui::RichText::new(t(footer)).size(12.5).color(weak));
    }
}

/// A setting: its title and note on the left, its control on the right,
/// both centred on the row. The control's size comes from the frame
/// before, so the text knows how much room it has.
fn item_row(ui: &mut Ui, item: &Item, s: &mut Settings) {
    let width = ui.available_width();
    let disabled = (item.disabled)(s);
    let size_id = ui.id().with(("pref-field", item.title));
    let field = ui.data(|d| d.get_temp::<egui::Vec2>(size_id)).unwrap_or(vec2(190.0, 24.0));
    let gap = 24.0;
    let text_width = (width - field.x - gap).max(width * 0.4);

    let fade = if disabled { 0.5 } else { 1.0 };
    let text_color = ui.visuals().text_color().gamma_multiply(fade);
    let title = ui.fonts_mut(|f| f.layout(t(item.title).to_string(), egui::TextStyle::Body.resolve(ui.style()), text_color, text_width));
    let note = (item.note)(s).map(|note| {
        let color = if note.error { ui.visuals().error_fg_color } else { ui.visuals().weak_text_color() }.gamma_multiply(fade);
        ui.fonts_mut(|f| f.layout(note.text, FontId::proportional(12.5), color, text_width))
    });
    let text_height = title.size().y + note.as_ref().map_or(0.0, |g| g.size().y + 3.0);
    let height = text_height.max(field.y).max(ui.spacing().interact_size.y);

    let (rect, _) = ui.allocate_exact_size(vec2(width, height), Sense::hover());
    let mut y = rect.center().y - text_height / 2.0;
    let title_height = title.size().y;
    ui.painter().galley(pos2(rect.left(), y), title, text_color);
    y += title_height + 3.0;
    if let Some(note) = note {
        ui.painter().galley(pos2(rect.left(), y), note, text_color);
    }

    let column = Rect::from_min_max(pos2(rect.left() + text_width + gap, rect.top()), rect.max);
    let mut control = ui.new_child(UiBuilder::new().max_rect(column).layout(Layout::right_to_left(Align::Center)).id_salt(item.title));
    if disabled {
        control.disable();
    }
    (item.field)(&mut control, s);
    let used = control.min_rect();
    let measured = vec2((rect.right() - used.left()).max(0.0), used.height());
    if (measured - field).length() > 0.5 {
        ui.data_mut(|d| d.insert_temp(size_id, measured));
        ui.ctx().request_repaint();
    }
    if used.bottom() > rect.bottom() {
        ui.allocate_space(vec2(width, used.bottom() - rect.bottom()));
    }
}

/// When nothing matches the search.
fn no_results(ui: &mut Ui, body: Rect) {
    let weak = ui.visuals().weak_text_color();
    let center = body.center() - vec2(0.0, 30.0);
    Icon::Search.image(weak, 28.0).paint_at(ui, Rect::from_center_size(center - vec2(0.0, 36.0), vec2(28.0, 28.0)));
    let painter = ui.painter();
    painter.text(center, egui::Align2::CENTER_CENTER, t("No settings found"), fastframe_fonts::Weight::SemiBold.font_id(15.0), ui.visuals().strong_text_color());
    painter.text(center + vec2(0.0, 24.0), egui::Align2::CENTER_CENTER, t("Try other words."), egui::TextStyle::Body.resolve(ui.style()), weak);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_finds_by_title_description_keyword_and_page() {
        let s = Settings::default();
        let p = pages();
        let pages_hit = |q: &str| -> Vec<&str> { filter(&p, &s, q).iter().enumerate().filter(|(_, g)| !g.is_empty()).map(|(i, _)| p[i].title).collect() };
        assert_eq!(pages_hit("typewriter"), ["Editor"]);
        assert_eq!(pages_hit("MIKTEX"), ["Compilation"], "a keyword, any case");
        assert_eq!(pages_hit("misspelled"), ["Writing"], "a description");
        assert!(pages_hit("").len() == p.len());
        assert!(pages_hit("zzzz").is_empty());
        // A page's title shows all of it.
        let editor = &filter(&p, &s, "editor")[1];
        assert_eq!(editor.iter().map(|(_, items)| items.len()).sum::<usize>(), 4);
    }

    #[test]
    fn reset_puts_a_page_back_and_only_it() {
        let p = pages();
        let mut s = Settings::default();
        assert!(p.iter().all(|page| !page.changed(&s)));
        s.font_size = 20.0;
        s.pdf_dark = true;
        assert!(p[1].changed(&s) && p[4].changed(&s) && !p[0].changed(&s));
        (p[1].reset)(&mut s, &Settings::default());
        assert_eq!(s.font_size, Settings::default().font_size);
        assert!(s.pdf_dark, "other pages keep theirs");
    }
}
