//! Everything the menus, the command palette and the keyboard shortcuts can
//! do (one `Cmd` enum so the three stay in step), plus the file operations
//! and insertions they lead to.

use super::{App, ConfirmAction, Dialog, PromptAction, StatusKind, TabView};
use crate::backend::project::{self, Kind};
use crate::backend::references;
use crate::backend::templates::Template;
use crate::dialogs::{AddReference, CommandPalette, Confirm, NewReference, PaletteItem, PaperMeta, Prompt, ReferenceSource, SnippetManager};
use crate::inserts::{label_slug, ImageBuilder, ListBuilder, MathBuilder, MathKind, TableBuilder};
use crate::editor;
use crate::i18n::{t, tn, Language};
use crate::icons::Icon;
use crate::settings::{Dock, FolderKind, SidebarView};
use crate::tf;
use egui::{Key, KeyboardShortcut, Modifiers};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, PartialEq)]
pub enum Cmd {
    /// F8 / Shift+F8: the next (or previous) error of the last compilation.
    NextError(bool),
    OpenProject,
    NewProject(Template),
    OpenProjectPath,
    OpenRecent(PathBuf),
    /// Drops a project from the recent ones (the folder stays).
    ForgetRecent(PathBuf),
    ClearRecent,
    RevealRecent(PathBuf),
    NewFile,
    NewFolder,
    GoToFile,
    Palette,
    SaveCompile,
    Find,
    ProjectSearch,
    SetMain,
    InsertTable,
    InsertList,
    InsertMath,
    InsertImage,
    PasteImage,
    ToggleSidebar,
    /// Ctrl+B / Ctrl+I on the selection.
    Bold,
    Emphasis,
    TogglePreview,
    Dock(Dock),
    Zen,
    /// Zen mode: between the text and the PDF, each alone on the screen.
    ZenRead,
    ToggleTheme,
    Typewriter,
    SpellCheck,
    Review,
    /// F7 / Shift+F7: the next (or previous) suggestion of the reviewer.
    NextSuggestion(bool),
    ComfortableWidth,
    AutoCompile,
    LocalHistory,
    SyncFollow,
    SyncNow,
    PdfDark,
    Snippets,
    FontBigger,
    FontSmaller,
    CloseTab,
    CloseAllTabs,
    NextTab,
    PrevTab,
    /// Alt+← / Alt+→ or a two-finger swipe: the place visited before (after).
    NavBack,
    NavForward,
    Shortcuts,
    Tour,
    GotoView(SidebarView),
    /// A place in a file: the palette's sections.
    GoToLine(PathBuf, usize),
    /// A project file picked in the palette, opened in a tab.
    OpenFile(PathBuf),
    /// Opens the dialog that adds a reference to the project's .bib.
    NewReference(ReferenceSource),
    /// Opens the palette on the references, to cite one.
    CitePicker,
    /// Cites the key at the cursor.
    Cite(String),
    NewNote,
    AddPapers,
    AddImages,
    Language(Language),
    Preferences,
}

impl Cmd {
    /// Its icon in the command palette.
    pub fn icon(&self) -> Icon {
        match self {
            Cmd::NextError(_) => Icon::TriangleAlert,
            Cmd::OpenProject | Cmd::OpenProjectPath | Cmd::OpenRecent(_) | Cmd::RevealRecent(_) => Icon::FolderOpen,
            Cmd::ForgetRecent(_) | Cmd::ClearRecent => Icon::X,
            Cmd::NewProject(_) | Cmd::NewFolder => Icon::FolderPlus,
            Cmd::NewFile => Icon::FilePlus,
            Cmd::GoToFile => Icon::Files,
            Cmd::Palette => Icon::Search,
            Cmd::SaveCompile => Icon::Save,
            Cmd::Find | Cmd::ProjectSearch => Icon::Search,
            Cmd::SetMain => Icon::Star,
            Cmd::InsertTable => Icon::Table,
            Cmd::InsertList => Icon::List,
            Cmd::InsertMath => Icon::Sigma,
            Cmd::InsertImage => Icon::Image,
            Cmd::PasteImage => Icon::ClipboardPaste,
            Cmd::AddImages => Icon::Images,
            Cmd::ToggleSidebar => Icon::PanelLeft,
            Cmd::Bold => Icon::Bold,
            Cmd::Emphasis => Icon::Italic,
            Cmd::TogglePreview => Icon::Eye,
            Cmd::Dock(_) => Icon::Columns,
            Cmd::Zen => Icon::Maximize,
            Cmd::ZenRead => Icon::BookOpenText,
            Cmd::ToggleTheme | Cmd::PdfDark => Icon::Moon,
            Cmd::Typewriter => Icon::Type,
            Cmd::SpellCheck | Cmd::NextSuggestion(_) => Icon::SpellCheck,
            Cmd::Review => Icon::Pencil,
            Cmd::ComfortableWidth => Icon::FitWidth,
            Cmd::AutoCompile => Icon::Zap,
            Cmd::LocalHistory => Icon::History,
            Cmd::SyncFollow | Cmd::SyncNow => Icon::Crosshair,
            Cmd::Snippets => Icon::Braces,
            Cmd::FontBigger => Icon::ZoomIn,
            Cmd::FontSmaller => Icon::ZoomOut,
            Cmd::CloseTab | Cmd::CloseAllTabs => Icon::X,
            Cmd::NextTab => Icon::ChevronRight,
            Cmd::PrevTab => Icon::ChevronLeft,
            Cmd::NavBack => Icon::ArrowLeft,
            Cmd::NavForward => Icon::ArrowRight,
            Cmd::Shortcuts => Icon::Keyboard,
            Cmd::Tour => Icon::Info,
            Cmd::GotoView(v) => SidebarView::ALL.iter().find(|(w, _, _)| w == v).map(|(_, i, _)| *i).unwrap_or(Icon::Files),
            Cmd::GoToLine(..) => Icon::Heading,
            Cmd::OpenFile(_) => Icon::File,
            Cmd::NewReference(_) => Icon::Library,
            Cmd::CitePicker | Cmd::Cite(_) => Icon::Quote,
            Cmd::NewNote => Icon::NotebookPen,
            Cmd::AddPapers => Icon::FileText,
            Cmd::Language(_) => Icon::Languages,
            Cmd::Preferences => Icon::Settings,
        }
    }

    /// The key it is remembered under among the palette's recent commands;
    /// `None` for places that come and go (sections, references).
    pub fn recent_id(&self) -> Option<String> {
        match self {
            Cmd::GoToLine(..) | Cmd::OpenFile(_) | Cmd::Cite(_) | Cmd::Palette | Cmd::ForgetRecent(_) | Cmd::RevealRecent(_) => None,
            other => Some(format!("{other:?}")),
        }
    }
}

pub struct MenuEntry {
    pub label: String,
    pub accel: Option<&'static str>,
    pub cmd: Cmd,
    pub enabled: bool,
    pub checked: Option<bool>,
    /// Starts a new section: a separator above it.
    pub section: bool,
    /// Shown inside this submenu; consecutive entries share it.
    pub submenu: Option<&'static str>,
}

impl MenuEntry {
    fn sep(mut self) -> Self {
        self.section = true;
        self
    }

    fn sub(mut self, name: &'static str) -> Self {
        self.submenu = Some(name);
        self
    }
}

pub struct MenuGroup {
    pub header: &'static str,
    pub entries: Vec<MenuEntry>,
    /// Shows the recent projects above its entries.
    pub recent: bool,
}

fn entry(label: impl Into<String>, accel: Option<&'static str>, cmd: Cmd, enabled: bool) -> MenuEntry {
    MenuEntry { label: label.into(), accel, cmd, enabled, checked: None, section: false, submenu: None }
}

fn check(label: impl Into<String>, cmd: Cmd, checked: bool) -> MenuEntry {
    MenuEntry { label: label.into(), accel: None, cmd, enabled: true, checked: Some(checked), section: false, submenu: None }
}

impl App {
    /// The app menu, grouped the way the titlebar shows it. The command
    /// palette and the shortcuts dialog are built from this same list.
    pub fn menu_groups(&self) -> Vec<MenuGroup> {
        let has_project = self.root.is_some();
        let tab = self.active_tab();
        let editable = tab.map(|t| t.editable()).unwrap_or(false);
        let is_tex = tab.map(|t| t.is_tex()).unwrap_or(false);
        let tex_file = tab.map(|t| project::is_document_source(&t.path)).unwrap_or(false);
        let has_errors = self.errors.iter().any(|e| e.file.is_some());
        let language_menu = t("Interface language");
        let mut language = vec![check(t("Follow the system"), Cmd::Language(Language::System), self.s.language == Language::System).sub(language_menu).sep()];
        for (l, name) in Language::CHOICES {
            language.push(check(name, Cmd::Language(l), self.s.language == l).sub(language_menu));
        }
        let new_project = t("New project");
        let position = t("Preview position");
        vec![
            MenuGroup {
                header: t("File"),
                recent: true,
                entries: vec![
                    entry(t("ABNT academic work (TCC, thesis)…"), None, Cmd::NewProject(Template::Abnt), true).sub(new_project),
                    entry(t("Article…"), None, Cmd::NewProject(Template::Article), true).sub(new_project),
                    entry(t("Report or thesis…"), None, Cmd::NewProject(Template::Report), true).sub(new_project),
                    entry(t("Presentation (beamer)…"), None, Cmd::NewProject(Template::Beamer), true).sub(new_project),
                    entry(t("Open project…"), Some("Ctrl+O"), Cmd::OpenProject, true),
                    entry(t("Open project by path…"), None, Cmd::OpenProjectPath, true),
                    entry(t("New file…"), Some("Ctrl+N"), Cmd::NewFile, has_project).sep(),
                    entry(t("New folder…"), None, Cmd::NewFolder, has_project),
                    entry(t("Go to file…"), Some("Ctrl+Shift+N"), Cmd::GoToFile, has_project),
                    entry(t("Local history of the file…"), None, Cmd::LocalHistory, has_project && editable),
                    entry(t("Save and compile"), Some("Ctrl+S"), Cmd::SaveCompile, has_project).sep(),
                    entry(t("Close tab"), Some("Ctrl+W"), Cmd::CloseTab, tab.is_some()).sep(),
                    entry(t("Close all tabs"), None, Cmd::CloseAllTabs, !self.tabs.is_empty()),
                    entry(t("Preferences…"), Some("Ctrl+,"), Cmd::Preferences, true).sep(),
                ],
            },
            MenuGroup {
                header: t("Edit"),
                recent: false,
                entries: vec![
                    entry(t("Bold"), Some("Ctrl+B"), Cmd::Bold, editable),
                    entry(t("Emphasis (italic)"), Some("Ctrl+I"), Cmd::Emphasis, editable),
                    entry(t("Find and replace…"), Some("Ctrl+F"), Cmd::Find, editable).sep(),
                    entry(t("Find and replace in the project…"), Some("Ctrl+Shift+F"), Cmd::ProjectSearch, has_project),
                    check(t("Writing review"), Cmd::Review, self.s.review).sep(),
                    entry(t("Next suggestion"), Some("F7"), Cmd::NextSuggestion(true), self.s.review && editable),
                    entry(t("Previous suggestion"), Some("Shift+F7"), Cmd::NextSuggestion(false), self.s.review && editable),
                    check(t("Spell check"), Cmd::SpellCheck, self.s.spell_check),
                    entry(t("Manage snippets…"), None, Cmd::Snippets, true).sep(),
                    entry(t("Command palette…"), Some("Ctrl+Shift+P"), Cmd::Palette, true),
                ],
            },
            MenuGroup {
                header: t("Document"),
                recent: false,
                entries: vec![
                    entry(t("Set as main file"), None, Cmd::SetMain, tex_file),
                    entry(t("Next error"), Some("F8"), Cmd::NextError(true), has_errors),
                    entry(t("Previous error"), Some("Shift+F8"), Cmd::NextError(false), has_errors),
                    entry(t("Insert list…"), None, Cmd::InsertList, is_tex).sep(),
                    entry(t("Insert table…"), None, Cmd::InsertTable, is_tex),
                    entry(t("Insert formula…"), Some("Ctrl+M"), Cmd::InsertMath, is_tex && editable),
                    entry(t("Insert image…"), None, Cmd::InsertImage, is_tex && has_project),
                    entry(t("Paste image from the clipboard"), Some("Ctrl+Alt+I"), Cmd::PasteImage, has_project && editable),
                    entry(t("Add images to the project…"), None, Cmd::AddImages, has_project),
                    entry(t("Insert citation…"), Some("Ctrl+Alt+C"), Cmd::CitePicker, is_tex && has_project).sep(),
                    entry(t("New reference…"), None, Cmd::NewReference(ReferenceSource::Identifier), has_project),
                    entry(t("New note…"), None, Cmd::NewNote, has_project).sep(),
                    entry(t("Attach papers (PDF)…"), None, Cmd::AddPapers, has_project),
                ],
            },
            MenuGroup {
                header: t("View"),
                recent: false,
                entries: [
                    vec![
                        entry(if self.s.sidebar_hidden { t("Show sidebar") } else { t("Hide sidebar") }, Some("Ctrl+Shift+E"), Cmd::ToggleSidebar, true),
                        entry(if self.s.preview_hidden { t("Show preview") } else { t("Hide preview") }, Some("Ctrl+J"), Cmd::TogglePreview, true),
                        check(t("On the right"), Cmd::Dock(Dock::Right), self.s.preview_dock == Dock::Right).sub(position),
                        check(t("On the left"), Cmd::Dock(Dock::Left), self.s.preview_dock == Dock::Left).sub(position),
                        check(t("At the top"), Cmd::Dock(Dock::Top), self.s.preview_dock == Dock::Top).sub(position),
                        check(t("At the bottom"), Cmd::Dock(Dock::Bottom), self.s.preview_dock == Dock::Bottom).sub(position),
                        entry(t("Zen mode"), Some("Ctrl+Shift+B"), Cmd::Zen, true).sep(),
                        entry(if self.zen_reading { t("Back to the text") } else { t("Read the PDF in zen mode") }, Some("Ctrl+Shift+J"), Cmd::ZenRead, true),
                        check(t("Typewriter mode"), Cmd::Typewriter, self.s.typewriter),
                        check(t("Comfortable line width"), Cmd::ComfortableWidth, self.s.comfortable_width),
                        entry(t("Bigger editor font"), Some("Ctrl+="), Cmd::FontBigger, true),
                        entry(t("Smaller editor font"), Some("Ctrl+-"), Cmd::FontSmaller, true),
                        entry(if self.s.dark { t("Light theme") } else { t("Dark theme") }, None, Cmd::ToggleTheme, true),
                        entry(t("Next tab"), Some("Ctrl+Tab"), Cmd::NextTab, self.tabs.len() > 1).sep(),
                        entry(t("Previous tab"), Some("Ctrl+Shift+Tab"), Cmd::PrevTab, self.tabs.len() > 1),
                        entry(t("Go back"), Some("Alt+←"), Cmd::NavBack, self.nav.can_back()),
                        entry(t("Go forward"), Some("Alt+→"), Cmd::NavForward, self.nav.can_forward()),
                        check(t("Compile automatically"), Cmd::AutoCompile, self.s.auto_compile).sep(),
                        check(t("Follow the cursor in the PDF"), Cmd::SyncFollow, self.s.sync_follow),
                        entry(t("Show the cursor's position in the PDF"), Some("Ctrl+Alt+J"), Cmd::SyncNow, editable),
                        check(t("Dark PDF"), Cmd::PdfDark, self.s.pdf_dark),
                    ],
                    language,
                ]
                .into_iter()
                .flatten()
                .collect(),
            },
            MenuGroup { header: t("Help"), recent: false, entries: vec![entry(t("Keyboard shortcuts…"), None, Cmd::Shortcuts, true), entry(t("Show the tour"), None, Cmd::Tour, has_project)] },
        ]
    }

    pub fn palette_items(&self) -> Vec<PaletteItem<Cmd>> {
        let citing = self.palette.as_ref().is_some_and(|p| p.citations);
        if citing {
            return self.citation_items(false);
        }
        let item = |label: String, group: &str, cmd: Cmd| PaletteItem {
            icon: cmd.icon(),
            id: cmd.recent_id(),
            label,
            group: group.into(),
            accel: None,
            checked: None,
            cmd,
            searched_only: false,
            sectioned: false,
            boost: 0,
        };
        // The project's files first, so their heading comes right after the
        // recent commands: the ones used lately shown, every one searched.
        let mut items = self.file_items();
        items.extend(self
            .menu_groups()
            .into_iter()
            // A menu's own entries before its submenus' ("New project" after "File").
            .flat_map(|g| {
                let (own, sub): (Vec<_>, Vec<_>) = g.entries.into_iter().partition(|e| e.submenu.is_none());
                own.into_iter().chain(sub).map(move |e| (g.header, e))
            })
            .filter(|(_, e)| e.enabled && e.cmd != Cmd::Palette)
            .map(|(h, e)| PaletteItem { accel: e.accel.map(String::from), checked: e.checked, ..item(e.label, e.submenu.unwrap_or(h), e.cmd) }),
        );
        for (view, _, label) in SidebarView::ALL {
            items.push(item(t(label).into(), t("Go to"), Cmd::GotoView(view)));
        }
        for r in &self.s.recent {
            items.push(item(r.name.clone(), t("Recent projects"), Cmd::OpenRecent(r.path.clone())));
        }
        for r in &self.s.recent {
            let label = tf!("Remove {name} from the recent projects", name = r.name.as_str());
            items.push(PaletteItem { searched_only: true, ..item(label, t("Recent projects"), Cmd::ForgetRecent(r.path.clone())) });
        }
        if !self.s.recent.is_empty() {
            items.push(item(t("Clear the recent projects").into(), t("Recent projects"), Cmd::ClearRecent));
        }
        // The open file's sections: typing part of a title jumps there.
        if let Some(path) = self.active_tab().filter(|t| t.is_tex()).map(|t| t.path.clone()) {
            for e in &self.outline {
                items.push(PaletteItem { searched_only: true, id: None, ..item(e.title.clone(), t("Section"), Cmd::GoToLine(path.clone(), e.line)) });
            }
            items.extend(self.citation_items(true));
        }
        items
    }

    /// One palette row per reference: title, first author and year, the key
    /// on the right (searched too). Among the commands, found by searching.
    fn citation_items(&self, searched_only: bool) -> Vec<PaletteItem<Cmd>> {
        let Some(refs) = &self.refs else { return vec![] };
        refs.iter()
            .map(|e| {
                let byline = [crate::backend::references::short_authors(&e.author), e.year.clone()].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", ");
                let title = if e.title.is_empty() { e.key.as_str() } else { e.title.as_str() };
                let label = if byline.is_empty() { title.to_string() } else { format!("{title} — {byline}") };
                PaletteItem {
                    label,
                    // Beside it, and searched too.
                    group: format!("{} · {}", t("Cite"), e.key),
                    icon: Icon::Quote,
                    accel: None,
                    checked: None,
                    cmd: Cmd::Cite(e.key.clone()),
                    id: None,
                    searched_only,
                    sectioned: false,
                    boost: 0,
                }
            })
            .collect()
    }

    /// One palette row per project file, by its path in the project and with
    /// its kind's icon, in a section of their own. The ones opened lately are
    /// listed with nothing typed and rank higher when searching.
    fn file_items(&self) -> Vec<PaletteItem<Cmd>> {
        let Some(tree) = &self.tree else { return vec![] };
        let mut files = Vec::new();
        tree.files(&mut files);
        let shown = 5;
        let mut items: Vec<(usize, PaletteItem<Cmd>)> = files
            .into_iter()
            .map(|n| {
                let recent = self.recent_files.iter().position(|p| *p == n.path);
                let item = PaletteItem {
                    label: project::relative(&n.path, self.root.as_deref()),
                    group: t("Files").into(),
                    icon: super::sidebar::kind_icon(n.kind),
                    accel: None,
                    checked: None,
                    cmd: Cmd::OpenFile(n.path),
                    id: None,
                    searched_only: !recent.is_some_and(|r| r < shown),
                    sectioned: true,
                    boost: recent.map(|r| (20 - r as i32).max(1) * 12).unwrap_or(0),
                };
                (recent.unwrap_or(usize::MAX), item)
            })
            .collect();
        // Most recent first; the rest keep the tree's order.
        items.sort_by_key(|(r, _)| *r);
        items.into_iter().map(|(_, i)| i).collect()
    }

    pub fn shortcut_groups(&self) -> Vec<(&'static str, Vec<(String, String)>)> {
        let mut groups = vec![(
            t("Navigation"),
            vec![
                (t("Leave zen mode, close a dialog or menu").to_string(), "Esc".to_string()),
                (t("Open the autocomplete").to_string(), t("Ctrl+Space").to_string()),
                (t("Next / previous compilation error").to_string(), "F8 / Shift+F8".to_string()),
                (t("Sync: clicking the PDF goes to the line").to_string(), t("click").to_string()),
                (t("Previous/next page in the PDF").to_string(), "PgUp / PgDn".to_string()),
            ],
        )];
        groups.push((
            t("Editor"),
            [
                (t("Go to the definition (\\ref, \\cite, \\input, project macro)"), t("F12 / Ctrl+click")),
                (t("Comment/uncomment lines"), "Ctrl+/"),
                (t("Move lines up/down"), "Alt+↑ / Alt+↓"),
                (t("Duplicate lines"), "Ctrl+Shift+D"),
                (t("Indent / unindent"), "Tab / Shift+Tab"),
                (t("Close \\begin{…} with \\end, continue \\item"), "Enter"),
                (t("Close pairs and wrap the selection"), "{ [ ( $"),
                (t("Fix the suggestion at the cursor"), "Ctrl+."),
            ]
            .into_iter()
            .map(|(a, b)| (a.to_string(), b.to_string()))
            .collect(),
        ));
        for g in self.menu_groups() {
            let items: Vec<(String, String)> =
                g.entries.iter().filter_map(|e| e.accel.map(|a| (e.label.clone(), a.to_string()))).collect();
            if !items.is_empty() {
                groups.push((g.header, items));
            }
        }
        groups
    }

    pub fn handle_shortcuts(&mut self, ctx: &egui::Context) {
        let ctrl = Modifiers::COMMAND;
        let ctrl_shift = Modifiers::COMMAND | Modifiers::SHIFT;
        let shortcuts: [(Modifiers, Key, Cmd); 30] = [
            (ctrl, Key::Comma, Cmd::Preferences),
            (Modifiers::NONE, Key::F8, Cmd::NextError(true)),
            (Modifiers::SHIFT, Key::F8, Cmd::NextError(false)),
            (Modifiers::NONE, Key::F7, Cmd::NextSuggestion(true)),
            (Modifiers::SHIFT, Key::F7, Cmd::NextSuggestion(false)),
            (Modifiers::COMMAND | Modifiers::ALT, Key::J, Cmd::SyncNow),
            (ctrl_shift, Key::P, Cmd::Palette),
            (Modifiers::COMMAND | Modifiers::ALT, Key::C, Cmd::CitePicker),
            (ctrl_shift, Key::F, Cmd::ProjectSearch),
            (ctrl_shift, Key::B, Cmd::Zen),
            (ctrl_shift, Key::J, Cmd::ZenRead),
            (ctrl_shift, Key::N, Cmd::GoToFile),
            (ctrl_shift, Key::Tab, Cmd::PrevTab),
            (Modifiers::COMMAND | Modifiers::ALT, Key::I, Cmd::PasteImage),
            (ctrl, Key::O, Cmd::OpenProject),
            (ctrl, Key::N, Cmd::NewFile),
            (ctrl_shift, Key::E, Cmd::ToggleSidebar),
            (ctrl, Key::B, Cmd::Bold),
            (ctrl, Key::M, Cmd::InsertMath),
            (ctrl, Key::I, Cmd::Emphasis),
            (ctrl, Key::J, Cmd::TogglePreview),
            (ctrl, Key::W, Cmd::CloseTab),
            (ctrl, Key::S, Cmd::SaveCompile),
            (ctrl, Key::F, Cmd::Find),
            (ctrl, Key::Tab, Cmd::NextTab),
            (ctrl, Key::Equals, Cmd::FontBigger),
            (ctrl, Key::Plus, Cmd::FontBigger),
            (ctrl, Key::Minus, Cmd::FontSmaller),
            (Modifiers::ALT, Key::ArrowLeft, Cmd::NavBack),
            (Modifiers::ALT, Key::ArrowRight, Cmd::NavForward),
        ];
        for (mods, key, cmd) in shortcuts {
            if ctx.input_mut(|i| i.consume_shortcut(&KeyboardShortcut::new(mods, key))) {
                self.run(cmd);
            }
        }
        // The mouse's side buttons, as in a browser.
        let (back, forward) = ctx.input(|i| (i.pointer.button_pressed(egui::PointerButton::Extra1), i.pointer.button_pressed(egui::PointerButton::Extra2)));
        if back || forward {
            self.run(if back { Cmd::NavBack } else { Cmd::NavForward });
        }
        // Esc closes the PDF's search bar first.
        let pdf_find = self.zen_reading && self.preview.finding();
        if self.zen && !pdf_find && ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.set_zen(false);
        }
    }

    pub fn run(&mut self, cmd: Cmd) {
        let root = self.root.clone();
        match cmd {
            Cmd::OpenProject => self.open_project_dialog(),
            Cmd::NewProject(template) => self.new_project_dialog(template),
            Cmd::OpenProjectPath => self.dialogs.push_back(Dialog::Prompt(
                Prompt::new(t("Open project by path"), "", t("Open"), PromptAction::OpenPath)
                    .message(t("Absolute path of the LaTeX project folder.")),
            )),
            Cmd::OpenRecent(path) => match self.open_project_path(&path) {
                Ok(()) => {
                    self.set_status(t("Project opened."), StatusKind::Normal);
                    self.finish_opening_project();
                }
                Err(_) => {
                    self.s.remove_recent(&path);
                    self.set_status(t("That folder no longer exists, so it was removed from the recent projects."), StatusKind::Error);
                }
            },
            Cmd::ForgetRecent(path) => {
                let name = self.s.recent.iter().find(|r| r.path == path).map(|r| r.name.clone()).unwrap_or_default();
                self.s.remove_recent(&path);
                self.set_status(&tf!("{name} removed from the recent projects. The folder is untouched.", name), StatusKind::Normal);
            }
            Cmd::ClearRecent => {
                self.s.recent.clear();
                self.set_status(t("Recent projects cleared. No folder was touched."), StatusKind::Normal);
            }
            Cmd::RevealRecent(path) => self.reveal(&path),
            Cmd::NewFile => {
                if let Some(r) = root {
                    self.prompt_new_file(r);
                }
            }
            Cmd::NewFolder => {
                if let Some(r) = root {
                    self.prompt_new_folder(r);
                }
            }
            Cmd::GoToFile => {
                if root.is_some() {
                    self.s.sidebar_hidden = false;
                    self.s.sidebar_view = SidebarView::Tree;
                    self.tree_filter_focus = true;
                }
            }
            Cmd::Palette => self.palette = Some(CommandPalette::new()),
            Cmd::SaveCompile => self.force_save_and_compile(),
            Cmd::Find => {
                let ctx = self.ctx.clone();
                // Over the preview, or in a PDF's tab: search the PDF.
                let pointer = ctx.pointer_hover_pos();
                let over_preview = self.zen_reading || (!self.zen && !self.s.preview_hidden && self.preview_rect.zip(pointer).is_some_and(|(r, p)| r.contains(p)));
                if over_preview {
                    self.preview.open_find();
                    return;
                }
                if let Some(pdf) = self.active_tab_mut().and_then(|t| t.pdf.as_mut()) {
                    pdf.open_find();
                    return;
                }
                if let Some(t) = self.active_tab_mut().filter(|t| t.editable()) {
                    // A one-line selection becomes the query.
                    if let Some((a, b)) = t.editor.selection(&ctx).filter(|(a, b)| b > a) {
                        let picked: String = t.content.chars().skip(a).take(b - a).collect();
                        if !picked.contains('\n') {
                            t.editor.find.query = picked;
                        }
                    }
                    t.editor.find.open = true;
                    t.editor.find_focus();
                }
            }
            Cmd::ProjectSearch => {
                if root.is_some() {
                    self.s.sidebar_hidden = false;
                    self.s.sidebar_view = SidebarView::Search;
                    self.project_search.focus();
                }
            }
            Cmd::SetMain => {
                if let Some(p) = self.active_tab().map(|t| t.path.clone()) {
                    self.set_main_file(p, false);
                }
            }
            Cmd::InsertTable => self.table_builder = Some(TableBuilder::default()),
            Cmd::InsertList => self.list_builder = Some(ListBuilder::default()),
            Cmd::InsertMath => self.open_math_builder(),
            Cmd::InsertImage => {
                if self.root.is_some() && self.active_tex_index(t("Open a .tex file to insert the figure.")).is_some() {
                    let folder = self.s.folders.path(FolderKind::Images);
                    self.image_builder = Some(ImageBuilder::new(self.images.clone(), self.cursor_section(), &folder));
                }
            }
            Cmd::PasteImage => {
                let at = self.active_tab().map(|t| t.editor.selection(&self.ctx).map(|s| s.0).unwrap_or(0)).unwrap_or(0);
                self.paste_image(at);
            }
            Cmd::ToggleSidebar => self.s.sidebar_hidden = !self.s.sidebar_hidden,
            Cmd::Bold | Cmd::Emphasis => {
                let markdown = self.active_tab().is_some_and(|t| t.lang() == crate::editor::highlight::Lang::Markdown);
                match (cmd, markdown) {
                    (Cmd::Bold, false) => self.surround("\\textbf{", "}"),
                    (_, false) => self.surround("\\emph{", "}"),
                    (Cmd::Bold, true) => self.surround("**", "**"),
                    (_, true) => self.surround("*", "*"),
                }
            }
            Cmd::TogglePreview => self.s.preview_hidden = !self.s.preview_hidden,
            Cmd::Dock(d) => {
                self.s.preview_dock = d;
                self.s.preview_hidden = false;
            }
            Cmd::Zen => self.set_zen(!self.zen),
            Cmd::ZenRead => self.set_zen_reading(!self.zen_reading),
            Cmd::ToggleTheme => {
                self.s.dark = !self.s.dark;
                self.apply_theme();
            }
            Cmd::Typewriter => self.s.typewriter = !self.s.typewriter,
            Cmd::Review => self.s.review = !self.s.review,
            Cmd::ComfortableWidth => self.s.comfortable_width = !self.s.comfortable_width,
            Cmd::NextSuggestion(forward) => {
                let ctx = self.ctx.clone();
                if let Some(t) = self.active_tab_mut().filter(|t| t.editable()) {
                    let found = t.editor.go_to_issue(&ctx, &t.content, forward);
                    if !found {
                        self.set_status(crate::i18n::t("No suggestions in this file."), StatusKind::Normal);
                    }
                }
            }
            Cmd::AutoCompile => self.s.auto_compile = !self.s.auto_compile,
            Cmd::LocalHistory => {
                if let (Some(root), Some(t)) = (self.root.clone(), self.active_tab().filter(|t| t.editable())) {
                    self.history_dialog = Some(crate::dialogs::HistoryDialog::new(&root, &t.path, &t.content));
                }
            }
            Cmd::SyncFollow => self.s.sync_follow = !self.s.sync_follow,
            Cmd::SyncNow => {
                self.s.preview_hidden = false;
                self.sync_to_preview();
            }
            Cmd::PdfDark => self.s.pdf_dark = !self.s.pdf_dark,
            Cmd::SpellCheck => {
                self.s.spell_check = !self.s.spell_check;
                if self.s.spell_check {
                    self.spell.ensure_loading(&self.ctx);
                }
                for t in &mut self.tabs {
                    t.editor.invalidate();
                }
            }
            Cmd::Snippets => self.snippet_manager = Some(SnippetManager::default()),
            Cmd::FontBigger => self.s.font_size = (self.s.font_size + 1.0).min(32.0),
            Cmd::FontSmaller => self.s.font_size = (self.s.font_size - 1.0).max(9.0),
            Cmd::CloseTab => {
                if let Some(i) = self.active {
                    self.close_tab(i);
                }
            }
            Cmd::CloseAllTabs => {
                self.save_dirty_tabs();
                while !self.tabs.is_empty() {
                    self.remove_tab(0);
                }
            }
            Cmd::NextTab | Cmd::PrevTab => {
                if let Some(i) = self.active {
                    let n = self.tabs.len();
                    let next = if cmd == Cmd::NextTab { (i + 1) % n } else { (i + n - 1) % n };
                    self.activate(next);
                }
            }
            Cmd::NavBack => self.nav_step(true),
            Cmd::NavForward => self.nav_step(false),
            Cmd::Shortcuts => self.shortcuts_open = true,
            Cmd::Tour => self.tour = Some(super::tour::Tour::default()),
            Cmd::NextError(forward) => self.go_to_error(forward),
            Cmd::GoToLine(file, line) => self.go_to(&file, line, true),
            // In zen mode too: the file changes, the mode stays.
            Cmd::OpenFile(path) => {
                let kind = self.tree.as_ref().and_then(|tr| tr.find(&path)).map(|n| n.kind);
                let kind = kind.unwrap_or_else(|| project::kind_of(&project::file_name(&path), false));
                self.open_file(&path, kind);
            }
            Cmd::GotoView(v) => {
                self.s.sidebar_hidden = false;
                self.s.sidebar_view = v;
            }
            Cmd::NewReference(source) => self.add_ref = Some(AddReference::new(source)),
            Cmd::CitePicker => {
                if self.active_tex_index(t("Open a .tex file to insert the citation.")).is_some() {
                    if self.refs.as_ref().is_some_and(|r| r.is_empty()) {
                        self.add_ref = Some(AddReference::new(ReferenceSource::Identifier));
                        self.set_status(t("The project has no reference yet: add one to cite it."), StatusKind::Normal);
                    } else {
                        self.palette = Some(CommandPalette::citations());
                    }
                }
            }
            Cmd::Cite(key) => self.insert_citation(&key),
            Cmd::NewNote => self.dialogs.push_back(Dialog::Prompt(Prompt::new(t("New note"), "", t("Create"), PromptAction::CreateNote))),
            Cmd::AddPapers => self.pick_papers(),
            Cmd::AddImages => self.pick_images(),
            Cmd::Language(language) => self.set_language(language),
            Cmd::Preferences => {
                self.preferences.get_or_insert_with(crate::preferences::Preferences::new);
            }
        }
    }

    /// Runs what was picked in the palette, which remembers it among the
    /// recent ones. A setting turned on or off says so: the palette is gone,
    /// and the change may not show.
    pub fn run_from_palette(&mut self, cmd: Cmd, items: &[PaletteItem<Cmd>]) {
        if let Some(id) = cmd.recent_id() {
            self.s.palette_recent.retain(|r| *r != id);
            self.s.palette_recent.insert(0, id);
            self.s.palette_recent.truncate(crate::dialogs::PALETTE_RECENT * 4);
        }
        let toggled = items
            .iter()
            .find(|i| i.cmd == cmd && !matches!(cmd, Cmd::Dock(_) | Cmd::Language(_)))
            .and_then(|i| i.checked.map(|on| (i.label.clone(), !on)));
        self.run(cmd);
        if let Some((name, on)) = toggled {
            let message = if on { tf!("{name}: on", name) } else { tf!("{name}: off", name) };
            self.set_status(&message, StatusKind::Success);
        }
    }

    // ---------- Native dialogs ----------

    /// The folder picker runs off the UI thread (it blocks until the user
    /// answers); its answer comes back as a message.
    pub fn open_project_dialog(&mut self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.save_dirty_tabs();
        std::thread::spawn(move || {
            let dir = rfd::FileDialog::new().set_title(t("Choose the LaTeX project folder")).pick_folder();
            let _ = tx.send(super::Msg::FolderPicked(dir));
            ctx.request_repaint();
        });
    }

    /// Picks (or creates) a folder for a new project from a template.
    pub fn new_project_dialog(&mut self, template: Template) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.save_dirty_tabs();
        std::thread::spawn(move || {
            let dir = rfd::FileDialog::new()
                .set_title(tf!("Folder for the new project ({template})", template = template.label()))
                .pick_folder();
            let _ = tx.send(super::Msg::NewProjectPicked(dir, template));
            ctx.request_repaint();
        });
    }

    fn pick_papers(&mut self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let files = rfd::FileDialog::new()
                .set_title(t("Choose one or more PDFs to attach as papers"))
                .add_filter("PDF", &["pdf", "PDF"])
                .pick_files()
                .unwrap_or_default();
            let _ = tx.send(super::Msg::PapersPicked(files));
            ctx.request_repaint();
        });
    }

    fn pick_images(&mut self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let files = rfd::FileDialog::new()
                .set_title(t("Choose one or more images to add to the project"))
                .add_filter(t("Images"), &["png", "jpg", "jpeg", "gif", "webp", "bmp", "svg", "pdf", "eps"])
                .pick_files()
                .unwrap_or_default();
            let _ = tx.send(super::Msg::ImagesPicked(files));
            ctx.request_repaint();
        });
    }

    /// Copies images into the images folder (those already in the project
    /// stay where they are), without inserting them anywhere.
    pub fn add_images(&mut self, paths: Vec<PathBuf>) {
        let Some(root) = self.root.clone() else { return };
        if paths.is_empty() {
            return;
        }
        let folder = self.s.folders.path(FolderKind::Images);
        let mut added = Vec::new();
        for path in &paths {
            match crate::backend::images::import_file(&root, self.main_file.as_deref(), &folder, path) {
                Ok(img) => {
                    if let Some(w) = &self.watcher {
                        w.suppress(&img.path);
                    }
                    added.push(img);
                }
                Err(e) => self.set_status(tf!("Could not add the image: {e}", e), StatusKind::Error),
            }
        }
        if added.is_empty() {
            return;
        }
        self.refresh_tree();
        self.refresh_project_data();
        let msg = if added.len() == 1 { tf!("Image added to the project: {path}.", path = added[0].rel_path) } else { tn("{n} image added.", "{n} images added.", added.len()) };
        self.set_status(msg, StatusKind::Success);
    }

    pub fn add_papers(&mut self, paths: Vec<PathBuf>) {
        let Some(root) = self.root.clone() else { return };
        if paths.is_empty() {
            return;
        }
        match crate::backend::papers::add(&root, &self.s.folders.path(FolderKind::Papers), &paths) {
            Ok(added) => {
                if let Some(w) = &self.watcher {
                    for p in &added {
                        w.suppress(&p.path);
                    }
                }
                self.refresh_tree();
                self.refresh_project_data();
                let msg = if added.len() == 1 { tf!("Paper attached: {name}.", name = added[0].name) } else { tn("{n} paper attached.", "{n} papers attached.", added.len()) };
                self.set_status(msg, StatusKind::Success);
            }
            Err(e) => self.set_status(tf!("Could not attach the PDF: {e}", e), StatusKind::Error),
        }
    }

    // ---------- Zen ----------

    pub fn set_zen(&mut self, on: bool) {
        if on == self.zen {
            return;
        }
        self.zen = on;
        self.zen_reading = false;
        if on {
            self.status_before_zen = self.status.clone();
            self.fullscreen_before_zen = self.ctx.input(|i| i.viewport().fullscreen.unwrap_or(false));
            self.set_status(t("Zen mode - Esc or Ctrl+Shift+B to leave."), StatusKind::Normal);
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(true));
            if let Some(t) = self.active_tab_mut() {
                t.editor.request_focus = true;
            }
        } else {
            self.status = self.status_before_zen.clone();
            self.ctx.send_viewport_cmd(egui::ViewportCommand::Fullscreen(self.fullscreen_before_zen));
            self.sync_at = Some(self.now());
        }
    }

    /// Zen mode's two sides: the PDF alone, or back to the text where the
    /// cursor was. Reading starts zen mode if it is off, at the passage of
    /// the cursor.
    pub fn set_zen_reading(&mut self, on: bool) {
        if on == self.zen_reading {
            return;
        }
        if on {
            self.set_zen(true);
            self.zen_reading = true;
            self.sync_to_preview();
        } else {
            self.zen_reading = false;
            if let Some(t) = self.active_tab_mut() {
                t.editor.request_focus = true;
            }
        }
    }

    pub fn update_title(&mut self, ctx: &egui::Context) {
        let project = self.root.as_ref().map(|r| project::file_name(r)).unwrap_or_default();
        let mut title = match self.active_tab() {
            Some(t) => {
                let dirty = if t.dirty { "● " } else { "" };
                format!("{dirty}{} · {project}", project::relative(&t.path, self.root.as_deref()))
            }
            None if !project.is_empty() => project,
            None => String::new(),
        };
        title = if title.is_empty() { "TecTec".into() } else { format!("{title} · TecTec") };
        if title != self.window_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.window_title = title;
        }
    }

    /// Jumps to the next (or previous) problem of the last compilation,
    /// counting from the cursor, across files; errors before warnings.
    fn go_to_error(&mut self, forward: bool) {
        let fatal = self.errors.iter().any(|e| !e.warning && e.file.is_some());
        let list: Vec<(PathBuf, usize)> = self
            .errors
            .iter()
            .filter(|e| !fatal || !e.warning)
            .filter_map(|e| Some((e.file.clone()?, e.line.max(1))))
            .collect();
        if list.is_empty() {
            self.set_status(t("No error with a line in the last compilation."), StatusKind::Normal);
            return;
        }
        let here = self.active_tab().map(|t| t.path.clone());
        let line = self.cursor_line;
        let same = |p: &PathBuf| here.as_ref() == Some(p);
        let pick = if forward {
            list.iter()
                .position(|(p, l)| same(p) && *l > line)
                .or_else(|| {
                    // Past the last one in this file: the next file's first.
                    let last_here = list.iter().rposition(|(p, _)| same(p))?;
                    Some((last_here + 1) % list.len())
                })
                .unwrap_or(0)
        } else {
            list.iter()
                .rposition(|(p, l)| same(p) && *l < line)
                .or_else(|| {
                    let first_here = list.iter().position(|(p, _)| same(p))?;
                    Some((first_here + list.len() - 1) % list.len())
                })
                .unwrap_or(list.len() - 1)
        };
        let (file, line) = list[pick].clone();
        self.errors_open = true;
        self.go_to(&file, line, true);
    }

    // ---------- File tree operations ----------

    pub fn prompt_new_file(&mut self, dir: PathBuf) {
        // The name to type over, in the interface's language.
        let stem = t("new");
        let name = format!("{stem}.tex");
        self.dialogs.push_back(Dialog::Prompt(Prompt::new(t("New file"), &name, t("Create"), PromptAction::NewFile(dir)).select(0, stem.chars().count())));
    }

    pub fn prompt_new_folder(&mut self, dir: PathBuf) {
        self.dialogs.push_back(Dialog::Prompt(Prompt::new(t("New folder"), "", t("Create"), PromptAction::NewFolder(dir))));
    }

    pub fn prompt_rename(&mut self, path: PathBuf) {
        let name = project::file_name(&path);
        let stem_len = match name.rfind('.') {
            Some(d) if d > 0 && !path.is_dir() => name[..d].chars().count(),
            _ => name.chars().count(),
        };
        self.dialogs.push_back(Dialog::Prompt(
            Prompt::new(&tf!("Rename {name}", name), &name, t("Rename"), PromptAction::Rename(path)).select(0, stem_len),
        ));
    }

    pub fn confirm_delete(&mut self, path: PathBuf) {
        let name = project::file_name(&path);
        let msg = if path.is_dir() {
            t("The folder and everything in it will be deleted from the disk. This cannot be undone.")
        } else {
            t("The file will be deleted from the disk. This cannot be undone.")
        };
        self.dialogs.push_back(Dialog::Confirm(Confirm::new(&tf!("Delete {name}?", name), msg, t("Delete"), ConfirmAction::Delete(path))));
    }

    /// Deleting an image says where the text still uses it.
    pub fn confirm_remove_image(&mut self, path: PathBuf, uses: usize) {
        let name = project::file_name(&path);
        let msg = if uses == 0 {
            t("The image will be deleted from the disk. This cannot be undone.").to_string()
        } else {
            tn(
                "The image will be deleted from the disk. This cannot be undone. The text uses it in {n} place, which will no longer compile.",
                "The image will be deleted from the disk. This cannot be undone. The text uses it in {n} places, which will no longer compile.",
                uses,
            )
        };
        self.dialogs.push_back(Dialog::Confirm(Confirm::new(&tf!("Delete {name}?", name), &msg, t("Delete"), ConfirmAction::Delete(path))));
    }

    pub fn delete_node(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else { return };
        let name = project::file_name(path);
        match project::delete(&root, path) {
            Ok(()) => {
                self.remove_tabs_under(path);
                if self.main_file.as_deref().map(|m| m.starts_with(path)).unwrap_or(false) {
                    self.main_file = None;
                }
                self.refresh_tree();
                self.refresh_project_data();
                self.set_status(tf!("{name} deleted.", name), StatusKind::Normal);
            }
            Err(e) => self.set_status(e, StatusKind::Error),
        }
    }

    pub fn reveal(&mut self, path: &Path) {
        let Some(root) = self.root.clone() else { return };
        if let Err(e) = project::reveal(&root, path) {
            self.set_status(tf!("Could not open the file manager: {e}", e), StatusKind::Error);
        }
    }

    pub fn on_prompt(&mut self, action: PromptAction, value: String) {
        let root = self.root.clone();
        match action {
            PromptAction::OpenPath => {
                let path = PathBuf::from(value.trim());
                match self.open_project_path(&path) {
                    Ok(()) => {
                        self.set_status(t("Project opened."), StatusKind::Normal);
                        self.finish_opening_project();
                    }
                    Err(e) => self.set_status(tf!("Could not open the project: {e}", e), StatusKind::Error),
                }
            }
            PromptAction::NewFile(dir) => {
                let Some(root) = root else { return };
                match project::create_file(&root, &dir, &value) {
                    Ok(path) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&path);
                        }
                        self.expanded.insert(dir);
                        self.refresh_tree();
                        let kind = project::kind_of(&project::file_name(&path), false);
                        self.open_file(&path, kind);
                        self.set_status(tf!("Created {name}.", name = project::file_name(&path)), StatusKind::Normal);
                    }
                    Err(e) => self.set_status(e, StatusKind::Error),
                }
            }
            PromptAction::NewFolder(dir) => {
                let Some(root) = root else { return };
                match project::create_folder(&root, &dir, &value) {
                    Ok(path) => {
                        self.expanded.insert(dir);
                        self.expanded.insert(path.clone());
                        self.refresh_tree();
                        self.set_status(tf!("Created the folder {name}.", name = project::file_name(&path)), StatusKind::Normal);
                    }
                    Err(e) => self.set_status(e, StatusKind::Error),
                }
            }
            PromptAction::Rename(old) => {
                let Some(root) = root else { return };
                if value.trim() == project::file_name(&old) {
                    return;
                }
                match project::rename(&root, &old, &value) {
                    Ok(new) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&new);
                        }
                        for t in &mut self.tabs {
                            if let Ok(rest) = t.path.strip_prefix(&old) {
                                t.path = if rest.as_os_str().is_empty() { new.clone() } else { new.join(rest) };
                                t.name = project::file_name(&t.path);
                            }
                        }
                        self.s.rename_favorites(&old, &new);
                        if let Some(m) = &self.main_file {
                            if let Ok(rest) = m.strip_prefix(&old) {
                                self.main_file = Some(if rest.as_os_str().is_empty() { new.clone() } else { new.join(rest) });
                            }
                        }
                        if self.expanded.remove(&old) {
                            self.expanded.insert(new.clone());
                        }
                        self.refresh_tree();
                        self.set_status(tf!("Renamed to {name}.", name = value.trim()), StatusKind::Normal);
                    }
                    Err(e) => self.set_status(e, StatusKind::Error),
                }
            }
            PromptAction::ExportVersion(pdf) => {
                let Some(root) = root else { return };
                match project::export_version(&root, &self.s.folders.path(FolderKind::Versions), &pdf, &value) {
                    Ok(dest) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&dest);
                        }
                        self.refresh_tree();
                        self.set_status(tf!("Version saved in {path}.", path = project::relative(&dest, Some(&root))), StatusKind::Success);
                    }
                    Err(e) => self.set_status(tf!("Could not make the version: {e}", e), StatusKind::Error),
                }
            }
            PromptAction::CreateNote => {
                let Some(root) = root else { return };
                match crate::backend::notes::create(&root, &self.s.folders.path(FolderKind::Notes), &value) {
                    Ok(note) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&note.path);
                        }
                        self.refresh_tree();
                        self.refresh_project_data();
                        self.open_file(&note.path, Kind::Text);
                    }
                    Err(e) => self.set_status(tf!("Could not create the note: {e}", e), StatusKind::Error),
                }
            }
        }
    }

    pub fn add_reference(&mut self, new: NewReference) {
        let Some(root) = self.root.clone() else { return };
        let fields = match new {
            NewReference::Import(id) => {
                self.set_status(t("Looking up the reference…"), StatusKind::Normal);
                self.spawn(move || super::Msg::Imported(references::import(&root, &id)));
                return;
            }
            NewReference::Manual(fields) => fields,
        };
        match references::add_manual(&root, &fields) {
            Ok(entry) => {
                if let Some(w) = &self.watcher {
                    w.suppress(&entry.file);
                }
                self.reload_clean_tab(&entry.file);
                self.refresh_tree();
                self.refresh_project_data();
                self.set_status(tf!("Reference added: {key}.", key = entry.key), StatusKind::Success);
            }
            Err(e) => self.set_status(tf!("Could not add the reference: {e}", e), StatusKind::Error),
        }
    }

    pub fn confirm_remove_reference(&mut self, file: PathBuf, key: String, used: bool) {
        let name = project::file_name(&file);
        let message = if used {
            tf!("The entry is deleted from {name}. The text still cites it: those citations will show as \"?\" in the PDF.", name)
        } else {
            tf!("The entry is deleted from {name}.", name)
        };
        self.dialogs.push_back(Dialog::Confirm(Confirm::new(&tf!("Remove {key}?", key = key.as_str()), &message, t("Remove"), ConfirmAction::RemoveReference { file, key })));
    }

    /// Moves a reference before another one of its .bib (or to its end),
    /// in the sidebar at once and then in the file.
    pub fn move_reference(&mut self, file: &Path, key: &str, before: Option<&str>) {
        if let Some(refs) = &mut self.refs {
            if let Some(from) = refs.iter().position(|e| e.key == key && e.file == file) {
                let entry = refs.remove(from);
                let to = match before {
                    Some(b) => refs.iter().position(|e| e.key == b && e.file == file).unwrap_or(refs.len()),
                    None => refs.iter().rposition(|e| e.file == file).map(|i| i + 1).unwrap_or(refs.len()),
                };
                refs.insert(to, entry);
            }
        }
        let (key, before) = (key.to_string(), before.map(String::from));
        self.edit_bib(file, |f| references::move_entry(f, &key, before.as_deref()), String::new());
    }

    /// Changes a .bib on disk, unless its tab has unsaved edits, then reloads
    /// what shows it.
    pub fn edit_bib(&mut self, file: &Path, change: impl FnOnce(&Path) -> project::Res<()>, done: String) {
        if self.tab_index(file).is_some_and(|i| self.tabs[i].dirty) {
            self.set_status(tf!("Save {file} first: it has unsaved changes.", file = project::file_name(file)), StatusKind::Error);
            self.refresh_project_data();
            return;
        }
        if let Some(w) = &self.watcher {
            w.suppress(file);
        }
        match change(file) {
            Ok(()) => {
                self.reload_clean_tab(file);
                if !done.is_empty() {
                    self.set_status(&done, StatusKind::Success);
                }
            }
            Err(e) => self.set_status(tf!("Could not change {file}: {e}", file = project::file_name(file), e), StatusKind::Error),
        }
        self.refresh_project_data();
    }

    /// Opens the .bib at the entry `key`.
    pub fn show_reference(&mut self, file: &Path, key: &str) {
        let text = std::fs::read_to_string(file).unwrap_or_default();
        let line = crate::backend::index::bib_spans(&text).into_iter().find(|(k, _)| k == key).map(|(_, s)| text[..s.start].matches('\n').count() + 1).unwrap_or(1);
        self.go_to(file, line, true);
    }

    pub fn edit_paper_meta(&mut self, name: String, path: PathBuf, cite_key: String, summary: String) {
        self.paper_meta = Some(PaperMeta { name, path, cite_key, summary });
    }

    pub fn confirm_remove_paper(&mut self, name: String, path: PathBuf) {
        self.dialogs.push_back(Dialog::Confirm(Confirm::new(
            &tf!("Remove {name}?", name),
            t("The PDF will be deleted from the disk. This cannot be undone."),
            t("Remove"),
            ConfirmAction::RemovePaper { path },
        )));
    }

    pub fn prompt_export_version(&mut self, pdf: PathBuf) {
        self.dialogs.push_back(Dialog::Prompt(
            Prompt::new(t("Save a PDF version"), "", t("Save"), PromptAction::ExportVersion(pdf))
                .message(t("An optional label for the file name (e.g. advisor's review).")),
        ));
    }

    // ---------- Insertions into the active editor ----------

    fn active_tex_index(&mut self, otherwise: &'static str) -> Option<usize> {
        match self.active {
            Some(i) if self.tabs[i].is_tex() => Some(i),
            _ => {
                self.set_status(otherwise, StatusKind::Normal);
                None
            }
        }
    }

    /// Replaces the selection with a template (`$SEL`, `$|`).
    /// Inserts `latex` as typed (no `$|` or `$SEL` markers in it) in place of
    /// the selection, on lines of its own, and leaves the cursor after it.
    /// The formula dialog; on a formula already in the text (the cursor in
    /// its `$…$` or `\[…\]`), to edit that one.
    pub fn open_math_builder(&mut self) {
        let Some(i) = self.active_tex_index(t("Open a .tex file to insert the formula.")) else { return };
        let tab = &self.tabs[i];
        let sel = tab.editor.selection(&self.ctx);
        let found = sel.and_then(|(a, _)| crate::mathpreview::find_math_at(&tab.content, a));
        self.math_builder = Some(match found {
            Some(m) => {
                let span: String = tab.content.chars().skip(m.from).take(m.to - m.from).collect();
                MathBuilder::new(if m.display { MathKind::Display } else { MathKind::Inline }, &m.formula, Some((m.from, m.to, span)))
            }
            None => {
                // A selection becomes the formula.
                let selected: String = sel.filter(|(a, b)| a != b).map(|(a, b)| tab.content.chars().skip(a).take(b - a).collect()).unwrap_or_default();
                MathBuilder::new(MathKind::Inline, &selected, None)
            }
        });
    }

    /// Puts the formula the dialog built in the text: over the one it edits,
    /// at the cursor in a sentence, or on lines of its own.
    pub fn insert_math(&mut self, kind: MathKind, replaces: Option<(usize, usize, String)>, latex: &str) {
        let Some(i) = self.active.filter(|&i| self.tabs[i].editable()) else { return };
        let ctx = self.ctx.clone();
        // Only over the formula it was opened on: if the text changed
        // meanwhile (reloaded from the disk), it goes in at the cursor.
        let replaces = replaces.filter(|(a, b, span)| self.tabs[i].content.chars().skip(*a).take(b - a).eq(span.chars())).map(|(a, b, _)| (a, b));
        let at = match replaces {
            Some(range) => Some(range),
            None if kind == MathKind::Inline => Some(self.tabs[i].editor.selection(&ctx).unwrap_or_else(|| {
                let n = self.tabs[i].content.chars().count();
                (n, n)
            })),
            None => None,
        };
        match at {
            // Written as it is: the formula may hold `$|`, which the
            // templates of `insert_block` would take for the cursor.
            Some((a, b)) => {
                let t = &mut self.tabs[i];
                t.editor.undo_point(&ctx, &t.content);
                let text = if replaces.is_some() { latex.trim_end_matches('\n') } else { latex };
                let end = editor::replace_chars(&mut t.content, a, b, text);
                t.editor.pending_selection = Some((end, end));
                t.editor.request_focus = true;
                self.on_edit();
            }
            None => self.insert_text(latex),
        }
    }

    pub fn insert_text(&mut self, latex: &str) {
        let Some(i) = self.active.filter(|&i| self.tabs[i].editable()) else { return };
        let ctx = self.ctx.clone();
        let t = &mut self.tabs[i];
        let sel = t.editor.selection(&ctx).unwrap_or_else(|| {
            let n = t.content.chars().count();
            (n, n)
        });
        let before = sel.0.checked_sub(1).and_then(|p| t.content.chars().nth(p));
        let after = t.content.chars().nth(sel.1);
        let mut block = latex.trim_end_matches('\n').to_string();
        if before.is_some_and(|c| c != '\n') {
            block.insert(0, '\n');
        }
        if after != Some('\n') {
            block.push('\n');
        }
        t.editor.undo_point(&ctx, &t.content);
        let end = editor::replace_chars(&mut t.content, sel.0, sel.1, &block);
        t.editor.pending_selection = Some((end, end));
        t.editor.request_focus = true;
        self.on_edit();
    }

    pub fn insert_block(&mut self, template: &str) {
        let Some(i) = self.active.filter(|&i| self.tabs[i].editable()) else { return };
        let ctx = self.ctx.clone();
        let t = &mut self.tabs[i];
        let sel = t.editor.selection(&ctx).unwrap_or_else(|| {
            let n = t.content.chars().count();
            (n, n)
        });
        t.editor.undo_point(&ctx, &t.content);
        let cursor = editor::insert_block(&mut t.content, sel, template);
        t.editor.pending_selection = Some((cursor, cursor));
        t.editor.request_focus = true;
        self.on_edit();
    }

    pub fn insert_at(&mut self, at: usize, template: &str) {
        let Some(i) = self.active.filter(|&i| self.tabs[i].editable()) else { return };
        let t = &mut self.tabs[i];
        t.editor.undo_point(&self.ctx, &t.content);
        let at = at.min(t.content.chars().count());
        let cursor = editor::insert_block(&mut t.content, (at, at), &template.replace("$SEL", ""));
        t.editor.pending_selection = Some((cursor, cursor));
        t.editor.request_focus = true;
        self.on_edit();
    }

    pub fn surround(&mut self, before: &str, after: &str) {
        let Some(i) = self.active.filter(|&i| self.tabs[i].editable()) else { return };
        let ctx = self.ctx.clone();
        let t = &mut self.tabs[i];
        let sel = t.editor.selection(&ctx).unwrap_or((0, 0));
        t.editor.undo_point(&ctx, &t.content);
        let (a, b) = editor::surround(&mut t.content, sel, before, after);
        t.editor.pending_selection = Some((a, b));
        t.editor.request_focus = true;
        self.on_edit();
    }

    /// `\cite{key}` at the cursor; inside the braces of a `\cite` already,
    /// the key joins its list.
    pub fn insert_citation(&mut self, key: &str) {
        let Some(i) = self.active_tex_index(t("Open a .tex file to insert the citation.")) else { return };
        let ctx = self.ctx.clone();
        let tab = &self.tabs[i];
        let joined = tab.editor.selection(&ctx).filter(|(a, b)| a == b).and_then(|(at, _)| editor::refs::cite_append_at(&tab.content, at, key));
        match joined {
            Some((at, text)) => {
                let t = &mut self.tabs[i];
                t.editor.undo_point(&ctx, &t.content);
                let end = editor::replace_chars(&mut t.content, at, at, &text);
                // After the closing brace.
                t.editor.pending_selection = Some((end + 1, end + 1));
                t.editor.request_focus = true;
                self.on_edit();
            }
            None => self.insert_block(&format!("\\cite{{{key}}}$|")),
        }
    }

    /// The title of the section the cursor is in.
    fn cursor_section(&self) -> Option<String> {
        self.outline.iter().take_while(|e| e.line <= self.cursor_line).last().map(|e| e.title.clone())
    }

    /// Inserts the figure the image dialog built, copying its image into
    /// the project first when it comes from outside.
    pub fn insert_built_image(&mut self, builder: ImageBuilder) {
        let Some(picked) = builder.picked.clone() else { return };
        let rel_path = match picked.rel_path {
            Some(rel) => rel,
            None => {
                let Some(root) = self.root.clone() else { return };
                match crate::backend::images::import_file(&root, self.main_file.as_deref(), &self.s.folders.path(FolderKind::Images), &picked.path) {
                    Ok(img) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&img.path);
                        }
                        self.refresh_tree();
                        self.refresh_project_data();
                        img.rel_path
                    }
                    Err(e) => {
                        self.set_status(tf!("Could not copy the image: {e}", e), StatusKind::Error);
                        return;
                    }
                }
            }
        };
        self.insert_text(&builder.latex(&rel_path));
    }

    /// The label folds in the current section so two "diagrama.png" in
    /// different chapters do not collide; `$|` lands after the caption.
    fn figure_snippet(&self, rel_path: &str) -> String {
        let name = rel_path.rsplit('/').next().unwrap_or(rel_path);
        let stem = name.rsplit_once('.').map(|(s, _)| s).unwrap_or(name);
        let section = self.cursor_section();
        let label = match section {
            Some(s) => format!("{}-{}", label_slug(&s), label_slug(stem)),
            None => label_slug(stem),
        };
        let mut caption: String = stem.replace(['_', '-'], " ").split_whitespace().collect::<Vec<_>>().join(" ");
        if let Some(first) = caption.chars().next() {
            caption = first.to_uppercase().collect::<String>() + &caption[first.len_utf8()..];
        }
        format!(
            "\\begin{{figure}}[htb]\n    \\centering\n    \\includegraphics[width=0.8\\linewidth]{{{rel_path}}}\n    \\caption{{{caption}$|}}\n    \\label{{fig:{label}}}\n\\end{{figure}}\n"
        )
    }

    pub fn insert_image(&mut self, rel_path: &str, at: Option<usize>) {
        if self.active_tex_index(t("Open a .tex file to insert the figure.")).is_none() {
            return;
        }
        let snippet = self.figure_snippet(rel_path);
        match at {
            Some(at) => self.insert_at(at, &snippet),
            None => self.insert_block(&snippet),
        }
    }

    /// Saves the clipboard image into `images/` and, in a `.tex`, inserts a
    /// figure for it.
    pub fn paste_image(&mut self, at: usize) {
        let Some(root) = self.root.clone() else { return };
        let image = match arboard::Clipboard::new().and_then(|mut c| c.get_image()) {
            Ok(img) => img,
            Err(_) => {
                self.set_status(t("The clipboard holds no image."), StatusKind::Normal);
                return;
            }
        };
        let Some(buffer) = image::RgbaImage::from_raw(image.width as u32, image.height as u32, image.bytes.into_owned()) else {
            self.set_status(t("The pasted image has an unexpected format."), StatusKind::Error);
            return;
        };
        let mut png = Vec::new();
        if let Err(e) = buffer.write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png) {
            self.set_status(tf!("Could not paste the image: {e}", e), StatusKind::Error);
            return;
        }
        match crate::backend::images::save_clipboard(&root, self.main_file.as_deref(), &self.s.folders.path(FolderKind::Images), &png, "png") {
            Ok(img) => {
                if let Some(w) = &self.watcher {
                    w.suppress(&img.path);
                }
                self.refresh_tree();
                self.refresh_project_data();
                if self.active_tab().map(|t| t.is_tex()).unwrap_or(false) {
                    let snippet = self.figure_snippet(&img.rel_path);
                    self.insert_at(at, &snippet);
                }
                self.set_status(tf!("Image pasted: {name}.", name = img.name), StatusKind::Success);
            }
            Err(e) => self.set_status(tf!("Could not paste the image: {e}", e), StatusKind::Error),
        }
    }

    /// Files dropped on the window from the system: a folder opens as the
    /// project, an image becomes a figure at the cursor, a PDF from outside
    /// becomes a paper, and anything inside the project opens in a tab.
    pub fn handle_dropped_files(&mut self, paths: Vec<PathBuf>) {
        for path in paths {
            if path.is_dir() {
                self.save_dirty_tabs();
                match self.open_project_path(&path) {
                    Ok(()) => {
                        self.set_status(t("Project opened."), StatusKind::Normal);
                        self.finish_opening_project();
                    }
                    Err(e) => self.set_status(tf!("Could not open the project: {e}", e), StatusKind::Error),
                }
                return;
            }
            let Some(root) = self.root.clone() else {
                // No project yet: a .tex brings its folder along.
                if project::is_document_source(&path) {
                    if let Some(dir) = path.parent().map(Path::to_path_buf) {
                        if self.open_project_path(&dir).is_ok() {
                            self.finish_opening_project();
                            self.open_file(&path, Kind::Tex);
                        }
                    }
                } else {
                    self.set_status(t("Drag the project folder (or a .tex) here to open it."), StatusKind::Normal);
                }
                continue;
            };
            let inside = project::resolve(&root, &path).is_ok() && path.exists();
            let kind = project::kind_of(&project::file_name(&path), false);
            match kind {
                Kind::Image | Kind::Svg => match crate::backend::images::import_file(&root, self.main_file.as_deref(), &self.s.folders.path(FolderKind::Images), &path) {
                    Ok(img) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&img.path);
                        }
                        self.refresh_tree();
                        self.refresh_project_data();
                        if self.active_tab().map(|t| t.is_tex()).unwrap_or(false) {
                            let snippet = self.figure_snippet(&img.rel_path);
                            self.insert_block(&snippet);
                            self.set_status(tf!("Figure inserted: {path}.", path = img.rel_path), StatusKind::Success);
                        } else {
                            self.set_status(tf!("Image added to the project: {path}.", path = img.rel_path), StatusKind::Success);
                        }
                    }
                    Err(e) => self.set_status(tf!("Could not add the image: {e}", e), StatusKind::Error),
                },
                Kind::Pdf if !inside => self.add_papers(vec![path]),
                _ if inside => {
                    let kind = if kind == Kind::Pdf || kind.editable() { kind } else { Kind::Text };
                    self.open_file(&path, kind);
                }
                _ => self.set_status(
                    tf!("{name} is outside the project. Only images and PDFs are brought in.", name = project::file_name(&path)),
                    StatusKind::Normal,
                ),
            }
        }
    }

    /// Replaces in every file of the search, skipping files with unsaved
    /// edits (their tab has content the disk does not).
    pub fn replace_all_in_project(&mut self, query: &str, replacement: &str, files: &[PathBuf]) {
        let Some(root) = self.root.clone() else { return };
        let (mut occurrences, mut changed, mut skipped) = (0, 0, 0);
        for path in files {
            let tab = self.tab_index(path);
            if tab.map(|i| self.tabs[i].dirty).unwrap_or(false) {
                skipped += 1;
                continue;
            }
            let Ok(read) = project::read(&root, path) else {
                skipped += 1;
                continue;
            };
            let (text, count) = crate::backend::search::replace_all_case_insensitive(&read.content, query, replacement);
            if count == 0 {
                continue;
            }
            match project::write(&root, path, &text, read.mod_time) {
                Ok(r) if !r.conflict => {
                    if let Some(w) = &self.watcher {
                        w.suppress(path);
                    }
                    occurrences += count;
                    changed += 1;
                    if let Some(i) = tab {
                        let t = &mut self.tabs[i];
                        t.editor.undo_point(&self.ctx, &t.content);
                        t.content = text;
                        t.disk_mtime = r.mod_time;
                        t.mark_saved();
                        t.editor.invalidate();
                    }
                }
                _ => skipped += 1,
            }
        }
        self.schedule_index();
        let mut msg = format!(
            "{} {}",
            tn("{n} occurrence replaced", "{n} occurrences replaced", occurrences),
            tn("in {n} file.", "in {n} files.", changed)
        );
        if skipped > 0 {
            msg.push(' ');
            msg.push_str(&tn("{n} skipped for unsaved changes or a conflict.", "{n} skipped for unsaved changes or a conflict.", skipped));
        }
        let ps = &mut self.project_search;
        ps.summary = Some(msg.clone());
        ps.results = crate::backend::search::find(&root, query).unwrap_or_default();
        self.set_status(msg, StatusKind::Success);
    }

    pub fn set_tab_view(&mut self, i: usize, view: TabView) {
        let Some(root) = self.root.clone() else { return };
        let t = &mut self.tabs[i];
        if t.view == view {
            return;
        }
        if view == TabView::Editor && !t.loaded {
            match project::read(&root, &t.path) {
                Ok(r) => {
                    t.content = r.content;
                    t.disk_mtime = r.mod_time;
                    t.loaded = true;
                    t.mark_saved();
                }
                Err(e) => {
                    let name = t.name.clone();
                    self.set_status(tf!("Could not read {name}: {e}", name, e), StatusKind::Error);
                    return;
                }
            }
        }
        t.view = view;
        self.refresh_outline_now();
    }
}
