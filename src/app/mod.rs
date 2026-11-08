//! The application shell, the hub every panel reports to: it owns the
//! debounced save/compile/sync/index/outline pipeline, the open project and
//! its tabs, and the dialogs. Drawing lives in `view.rs` and `sidebar.rs`;
//! the actions the menus and shortcuts trigger live in `actions.rs`.

mod actions;
#[cfg(debug_assertions)]
mod debug;
pub(crate) mod navigate;
mod sidebar;
mod tour;
mod view;
mod window;

use crate::backend::images::Image;
use crate::backend::index::Index;
use crate::backend::latex::{CompileError, CompileResult, EngineInfo};
use crate::backend::notes::Note;
use crate::backend::papers::Paper;
use crate::backend::project::{self, Kind, Node};
use crate::backend::synctex::{self, Map};
use crate::backend::watch::Watcher;
use crate::backend::{document, latex, outline, references, todos};
use crate::dialogs::{self, AddReference, CommandPalette, Confirm, PaperMeta, ProjectSearch, Prompt, SnippetManager};
use crate::inserts::{ImageBuilder, ListBuilder, TableBuilder};
use crate::editor::completion::Completions;
use crate::editor::highlight::Lang;
use crate::editor::TabEditor;
use crate::pdfview::PdfView;
use crate::settings::Settings;
use crate::mathpreview::MathPreview;
use crate::spell::SpellCache;
use crate::i18n::{t, tn};
use crate::tf;
use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

// Saving is cheap and protects against losing work; compiling spawns LaTeX
// and therefore waits for the user to really stop typing.
const SAVE_IDLE: f64 = 0.8;
// Opening waits far less than typing does.
const OPEN_COMPILE_IDLE: f64 = 0.4;
// Forward search is a lookup in a loaded map; the delay only avoids one call
// per line while holding an arrow key.
const SYNC_IDLE: f64 = 0.15;
// Reindexing reads every .tex and .bib in the project.
const INDEX_IDLE: f64 = 0.6;
// The outline is the one you watch while typing.
const OUTLINE_IDLE: f64 = 0.25;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum StatusKind {
    Normal,
    Success,
    Error,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TabView {
    Editor,
    Viewer,
}

pub struct Tab {
    pub path: PathBuf,
    pub name: String,
    pub kind: Kind,
    pub view: TabView,
    pub content: String,
    pub loaded: bool,
    /// The text differs from what was last read or saved.
    pub dirty: bool,
    /// Hash of the text last read or saved.
    saved: u64,
    pub disk_mtime: i64,
    pub pinned: bool,
    pub editor: TabEditor,
    pub pdf: Option<PdfView>,
}

impl Tab {
    /// The text is now what the disk has.
    pub fn mark_saved(&mut self) {
        self.saved = crate::editor::text_hash(&self.content, &[]);
        self.dirty = false;
    }

    pub fn editable(&self) -> bool {
        self.view == TabView::Editor
    }

    pub fn is_tex(&self) -> bool {
        self.editable() && project::ext_lower(&self.path) == "tex"
    }

    pub fn lang(&self) -> Lang {
        Lang::for_path(&self.path)
    }
}

/// A heading of the outline, and the file it is in.
pub struct OutlineRow {
    pub file: PathBuf,
    pub entry: outline::Entry,
    /// Prose words under it, subsections included (whole-document outline only).
    pub words: Option<usize>,
}

pub enum Msg {
    Compiled { seq: u64, main: PathBuf, quiet: bool, result: Result<CompileResult, String> },
    SyncMap { path: PathBuf, mtime: i64, map: Result<Arc<Map>, String> },
    ProjectData {
        root: PathBuf,
        index: Option<Index>,
        refs: Option<Vec<references::Entry>>,
        notes: Vec<Note>,
        todos: Vec<todos::Entry>,
        papers: Vec<Paper>,
        images: Vec<Image>,
    },
    Packages(Vec<String>),
    FsChanged(Vec<PathBuf>),
    Imported(Result<references::Entry, String>),
    Engine(EngineInfo),
    FolderPicked(Option<PathBuf>),
    NewProjectPicked(Option<PathBuf>, crate::backend::templates::Template),
    PapersPicked(Vec<PathBuf>),
    ImagesPicked(Vec<PathBuf>),
}

/// What a prompt does once confirmed.
pub enum PromptAction {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Rename(PathBuf),
    ExportVersion(PathBuf),
    CreateNote,
    OpenPath,
}

/// What a confirmation does, by answer.
pub enum ConfirmAction {
    Delete(PathBuf),
    /// Saving hit a file changed on disk: overwrite (yes) or reload (no).
    SaveConflict { path: PathBuf, disk_mtime: i64 },
    /// An external change landed on a dirty tab: reload (yes) or keep (no).
    ExternalChange(PathBuf),
    CompileOverwrite { main: PathBuf, quiet: bool },
    ReplaceAll { query: String, replacement: String, files: Vec<PathBuf> },
    RemovePaper { path: PathBuf },
    RemoveReference { file: PathBuf, key: String },
}

pub enum Dialog {
    Prompt(Prompt<PromptAction>),
    Confirm(Confirm<ConfirmAction>),
}

pub struct App {
    pub s: Settings,
    root: Option<PathBuf>,
    tree: Option<Node>,
    expanded: HashSet<PathBuf>,
    main_file: Option<PathBuf>,
    tabs: Vec<Tab>,
    active: Option<usize>,
    /// The files shown in a tab lately, most recent first: the palette
    /// ranks them higher.
    recent_files: Vec<PathBuf>,
    next_tab_id: u64,

    status: (String, StatusKind),
    /// When the status was set: informative messages fade out after a while.
    status_at: f64,
    save_indicator: String,
    errors: Vec<CompileError>,
    /// The line each error points at, read from disk when the compile
    /// finished: the error panel shows it for files not open in a tab.
    error_lines: HashMap<(PathBuf, usize), String>,
    errors_open: bool,
    outline: Vec<outline::Entry>,
    outline_hint: Option<String>,
    /// The whole document from the main file, `\\input`s followed: its
    /// outline, words and preamble facts.
    doc: Option<Arc<document::Document>>,
    index: Index,
    refs: Option<Vec<references::Entry>>,
    notes: Vec<Note>,
    todos: Vec<todos::Entry>,
    papers: Vec<Paper>,
    images: Vec<Image>,
    completions: Completions,
    spell: SpellCache,
    math: MathPreview,
    /// The \ref/\cite/command under the pointer and since when.
    ref_hover: Option<(crate::editor::refs::RefTarget, f64)>,
    /// Back and forward between the places visited.
    nav: navigate::History,
    swipe: navigate::Swipe,
    preview: PdfView,
    synctex: Option<(PathBuf, i64, Arc<Map>)>,
    synctex_loading: Option<(PathBuf, i64)>,
    watcher: Option<Watcher>,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    ctx: egui::Context,

    save_at: Option<f64>,
    compile_at: Option<(f64, bool)>,
    sync_at: Option<f64>,
    index_at: Option<f64>,
    outline_at: Option<f64>,
    compile_seq: u64,
    compile_cancel: Option<Arc<AtomicBool>>,
    compiling: bool,
    /// When the running compilation started: the preview shows a spinner
    /// only for one that takes a while.
    compile_started: f64,
    /// The LaTeX toolchain found on the PATH (None until detected).
    engine: Option<EngineInfo>,
    /// Edits since the last compilation started: the preview is behind.
    edited_since_compile: bool,
    /// How the last finished compilation went, and when.
    last_compile: Option<(bool, String)>,
    overwrite_confirmed: HashSet<PathBuf>,
    /// Set while an inverse search moves the cursor, so the forward search it
    /// would trigger does not scroll the preview back.
    suppress_sync_line: Option<usize>,
    cursor_line: usize,

    zen: bool,
    /// Zen mode showing the PDF, alone, instead of the text.
    zen_reading: bool,
    fullscreen_before_zen: bool,
    /// The window no longer opens in full screen (see window.rs).
    window_settled: bool,
    /// Whether the window had the focus last frame.
    was_focused: bool,
    status_before_zen: (String, StatusKind),

    dialogs: VecDeque<Dialog>,
    palette: Option<CommandPalette>,
    /// Where the preview panel was drawn last frame: Ctrl+F over it searches
    /// the PDF.
    preview_rect: Option<egui::Rect>,
    /// The search panel's state, kept while the sidebar shows something else.
    project_search: ProjectSearch,
    snippet_manager: Option<SnippetManager>,
    shortcuts_open: bool,
    /// The first-run tour, while it runs, and where its stops were drawn.
    tour: Option<tour::Tour>,
    spots: tour::Spots,
    preferences: Option<crate::preferences::Preferences>,
    table_builder: Option<TableBuilder>,
    list_builder: Option<ListBuilder>,
    math_builder: Option<crate::inserts::MathBuilder>,
    image_builder: Option<ImageBuilder>,
    add_ref: Option<AddReference>,
    paper_meta: Option<PaperMeta>,
    history_dialog: Option<dialogs::HistoryDialog>,

    tree_filter: String,
    tree_filter_focus: bool,
    ref_filter: String,
    /// The references sidebar lists only the entries the text does not cite.
    ref_unused_only: bool,
    image_filter: String,
    /// The images sidebar lists only the images the text does not use.
    image_unused_only: bool,
    window_title: String,
    close_confirmed: bool,
    /// Brings the active tab into view in the tab strip once.
    scroll_active_tab: bool,
    /// The tray item, where the desktop has a tray.
    tray: Option<fastframe_tray::Tray>,
    /// Keeps a touchpad flick scrolling after the fingers lift.
    kinetic: crate::kinetic::Kinetic,
    #[cfg(debug_assertions)]
    debug: debug::DebugHooks,
}

pub fn now(ctx: &egui::Context) -> f64 {
    ctx.input(|i| i.time)
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        egui_extras::install_image_loaders(&cc.egui_ctx);
        fastframe_icons::install::<crate::icons::Icon>(&cc.egui_ctx);
        crate::theme::install(&cc.egui_ctx);
        let mut s: Settings = cc.storage.and_then(|st| eframe::get_value(st, eframe::APP_KEY)).unwrap_or_default();
        s.migrate();
        crate::i18n::set(s.language);
        crate::settings::watch_system_accent(&cc.egui_ctx);
        crate::theme::set_accent(&cc.egui_ctx, s.accent.color());
        latex::set_system_tex(s.system_tex);
        let (tx, rx) = channel();
        let mut completions = Completions::new();
        completions.set_snippets(&s.snippets);
        let spell = SpellCache::new(s.ignored_words.iter().cloned().collect());
        let mut app = Self {
            root: None,
            tree: None,
            expanded: HashSet::new(),
            main_file: None,
            tabs: vec![],
            active: None,
            recent_files: Vec::new(),
            next_tab_id: 1,
            status: (t("Ready.").into(), StatusKind::Normal),
            status_at: 0.0,
            save_indicator: String::new(),
            errors: vec![],
            error_lines: HashMap::new(),
            errors_open: false,
            outline: vec![],
            outline_hint: Some(t("No file open.").into()),
            doc: None,
            index: Index::default(),
            refs: None,
            notes: vec![],
            todos: vec![],
            papers: vec![],
            images: vec![],
            completions,
            spell,
            math: MathPreview::default(),
            ref_hover: None,
            nav: Default::default(),
            swipe: Default::default(),
            preview: PdfView::new("preview", t("Open a project and compile to see the PDF here.")),
            synctex: None,
            synctex_loading: None,
            watcher: None,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            save_at: None,
            compile_at: None,
            sync_at: None,
            index_at: None,
            outline_at: None,
            compile_seq: 0,
            compile_cancel: None,
            compiling: false,
            compile_started: 0.0,
            engine: None,
            edited_since_compile: false,
            last_compile: None,
            overwrite_confirmed: HashSet::new(),
            suppress_sync_line: None,
            cursor_line: 0,
            zen: false,
            zen_reading: false,
            fullscreen_before_zen: false,
            window_settled: false,
            was_focused: true,
            status_before_zen: (String::new(), StatusKind::Normal),
            dialogs: VecDeque::new(),
            palette: None,
            project_search: ProjectSearch::default(),
            preview_rect: None,
            snippet_manager: None,
            shortcuts_open: false,
            tour: None,
            spots: tour::Spots::default(),
            preferences: None,
            table_builder: None,
            list_builder: None,
            math_builder: None,
            image_builder: None,
            add_ref: None,
            paper_meta: None,
            history_dialog: None,
            tree_filter: String::new(),
            tree_filter_focus: false,
            ref_filter: String::new(),
            ref_unused_only: false,
            image_filter: String::new(),
            image_unused_only: false,
            window_title: String::new(),
            close_confirmed: false,
            scroll_active_tab: false,
            tray: crate::tray::spawn(&cc.egui_ctx),
            kinetic: Default::default(),
            #[cfg(debug_assertions)]
            debug: Default::default(),
            s,
        };
        app.apply_theme();
        app.spawn(|| Msg::Engine(latex::detect_engine()));
        if app.s.spell_check {
            app.spell.ensure_loading(&cc.egui_ctx);
        }
        // A folder given on the command line wins over the last session; a
        // file (opened from the file manager) opens with its folder.
        let arg = std::env::args_os().nth(1).map(PathBuf::from);
        let file = arg.as_ref().filter(|p| p.is_file()).and_then(|p| std::fs::canonicalize(p).ok());
        let folder = match &file {
            Some(f) => f.parent().map(Path::to_path_buf),
            None => arg,
        };
        match folder {
            Some(dir) => match app.open_project_path(&dir) {
                Ok(()) => {
                    app.set_status(t("Project opened."), StatusKind::Normal);
                    app.finish_opening_project();
                    if let Some(f) = file {
                        let kind = project::kind_of(&project::file_name(&f), false);
                        app.open_file(&f, kind);
                    }
                }
                Err(e) => app.set_status(tf!("Could not open the project: {e}", e), StatusKind::Error),
            },
            None => app.restore_session(),
        }
        app
    }

    /// Switches the interface language; menus, dialogs and the tray follow
    /// on the next frame.
    pub fn set_language(&mut self, language: crate::i18n::Language) {
        self.s.language = language;
        crate::i18n::set(language);
        self.window_title.clear();
        self.relabel_tray();
        // The autocomplete keeps its descriptions translated.
        self.completions.set_snippets(&self.s.snippets);
        self.completions.retranslate();
        self.completions.set_index(&self.index);
    }

    fn relabel_tray(&mut self) {
        if let Some(tray) = &mut self.tray {
            crate::tray::relabel(tray);
        }
    }

    /// Clicks on the tray item and its menu.
    fn handle_tray(&mut self, ctx: &egui::Context) {
        let Some(tray) = &self.tray else { return };
        for event in tray.events() {
            match event {
                fastframe_tray::Event::Toggle | fastframe_tray::Event::Show | fastframe_tray::Event::Menu("show") => {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
                }
                fastframe_tray::Event::Menu("compile") => self.force_save_and_compile(),
                fastframe_tray::Event::Menu("quit") => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                fastframe_tray::Event::Menu(_) => {}
            }
        }
    }

    pub fn apply_theme(&self) {
        self.ctx.set_theme(if self.s.dark { egui::Theme::Dark } else { egui::Theme::Light });
    }

    fn set_status(&mut self, text: impl Into<String>, kind: StatusKind) {
        self.status = (text.into(), kind);
        self.status_at = self.ctx.input(|i| i.time);
    }

    fn now(&self) -> f64 {
        now(&self.ctx)
    }

    fn spawn(&self, job: impl FnOnce() -> Msg + Send + 'static) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(job());
            ctx.request_repaint();
        });
    }

    fn active_tab(&self) -> Option<&Tab> {
        self.active.and_then(|i| self.tabs.get(i))
    }

    fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        self.active.and_then(|i| self.tabs.get_mut(i))
    }

    fn tab_index(&self, path: &Path) -> Option<usize> {
        self.tabs.iter().position(|t| t.path == path)
    }

    fn now_label() -> String {
        chrono::Local::now().format("%H:%M").to_string()
    }

    // ---------- Project ----------

    /// Everything shared by the ways a project gets opened (dialog, recent
    /// list, session restore).
    fn open_project_path(&mut self, path: &Path) -> Result<(), String> {
        let path = std::fs::canonicalize(path).map_err(|_| tf!("folder not found: {path}", path = path.display()))?;
        if !path.is_dir() {
            return Err(tf!("folder not found: {path}", path = path.display()));
        }
        // The app was called latex-editor: the project's local history moves over.
        let (old, new) = (path.join(".latex-editor"), path.join(".tectec"));
        if old.is_dir() && !new.exists() {
            let _ = std::fs::rename(old, new);
        }
        let tree = project::build(&path)?;
        self.save_dirty_tabs();
        self.tabs.clear();
        self.active = None;
        self.nav.clear();
        self.errors.clear();
        self.errors_open = false;
        self.synctex = None;
        self.overwrite_confirmed.clear();
        self.main_file = project::guess_main_file(&tree);
        self.expanded = HashSet::from([path.clone()]);
        self.tree = Some(tree);
        self.root = Some(path.clone());
        self.s.push_recent(path.clone(), project::file_name(&path));
        self.preview.clear(t("Compiling…"));

        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.watcher = Watcher::new(&path, move |paths| {
            let _ = tx.send(Msg::FsChanged(paths));
            ctx.request_repaint();
        })
        .ok();

        self.refresh_project_data();
        self.spawn(|| Msg::Packages(crate::backend::index::packages()));
        Ok(())
    }

    fn finish_opening_project(&mut self) {
        self.open_preview();
        self.spawn(|| Msg::Engine(latex::detect_engine()));
    }

    fn restore_session(&mut self) {
        let Some(session) = self.s.session.clone() else { return };
        if self.open_project_path(&session.project_root).is_err() {
            self.s.session = None;
            self.s.remove_recent(&session.project_root);
            return;
        }
        if let (Some(m), Some(tree)) = (&session.main_file, &self.tree) {
            if tree.find(m).is_some() {
                self.main_file = Some(m.clone());
            }
        }
        for t in &session.tabs {
            let Some(kind) = self.tree.as_ref().and_then(|tr| tr.find(&t.path)).map(|n| n.kind) else { continue };
            if self.open_file(&t.path.clone(), kind) {
                if let Some(i) = self.tab_index(&t.path) {
                    self.tabs[i].pinned = t.pinned;
                }
            }
        }
        if let Some(a) = &session.active {
            if let Some(i) = self.tab_index(a) {
                self.activate(i);
                let t = &mut self.tabs[i];
                if session.cursor_line > 0 && t.editable() {
                    t.editor.go_to_line(&t.content, session.cursor_line, None);
                    self.cursor_line = session.cursor_line;
                }
            }
        }
        self.sort_pinned();
        self.open_preview();
        self.spawn(|| Msg::Engine(latex::detect_engine()));
        self.set_status(t("Project resumed where you left off."), StatusKind::Normal);
    }

    pub fn current_session(&self) -> Option<crate::settings::Session> {
        let root = self.root.clone()?;
        Some(crate::settings::Session {
            project_root: root,
            main_file: self.main_file.clone(),
            tabs: self.tabs.iter().map(|t| crate::settings::SessionTab { path: t.path.clone(), pinned: t.pinned }).collect(),
            active: self.active_tab().map(|t| t.path.clone()),
            cursor_line: self.cursor_line,
        })
    }

    fn refresh_tree(&mut self) {
        let Some(root) = self.root.clone() else { return };
        if let Ok(tree) = project::build(&root) {
            self.tree = Some(tree);
        }
        self.schedule_index();
    }

    // ---------- Tabs ----------

    /// Opens (or focuses) a file. Returns false when it could not be read.
    pub fn open_file(&mut self, path: &Path, kind: Kind) -> bool {
        if let Some(i) = self.tab_index(path) {
            self.activate(i);
            return true;
        }
        let view = if kind.editable() { TabView::Editor } else { TabView::Viewer };
        let mut tab = Tab {
            path: path.to_path_buf(),
            name: project::file_name(path),
            kind,
            view,
            content: String::new(),
            loaded: false,
            dirty: false,
            saved: 0,
            disk_mtime: 0,
            pinned: false,
            editor: TabEditor::new(egui::Id::new(("tab-editor", self.next_tab_id))),
            pdf: None,
        };
        self.next_tab_id += 1;
        if view == TabView::Editor {
            let Some(root) = self.root.clone() else { return false };
            match project::read(&root, path) {
                Ok(r) => {
                    tab.content = r.content;
                    tab.disk_mtime = r.mod_time;
                    tab.loaded = true;
                    tab.mark_saved();
                }
                Err(e) => {
                    self.set_status(tf!("Could not read {name}: {e}", name = tab.name, e), StatusKind::Error);
                    return false;
                }
            }
        }
        if kind == Kind::Pdf {
            let mut v = PdfView::new(("tab-pdf", self.next_tab_id), t("Loading…"));
            v.load(&self.ctx, path);
            tab.pdf = Some(v);
        }
        self.tabs.push(tab);
        self.activate(self.tabs.len() - 1);
        if self.main_file.is_none() && project::is_document_source(path) {
            self.set_main_file(path.to_path_buf(), true);
        }
        true
    }

    fn activate(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        let to = self.tabs[i].path.clone();
        self.nav_leaving_for(&to);
        self.active = Some(i);
        self.scroll_active_tab = true;
        let path = self.tabs[i].path.clone();
        self.recent_files.retain(|p| *p != path);
        self.recent_files.insert(0, path);
        self.recent_files.truncate(40);
        let t = &mut self.tabs[i];
        t.editor.request_focus = true;
        self.cursor_line = t.editor.cursor_line();
        self.refresh_outline_now();
    }

    fn sort_pinned(&mut self) {
        let active = self.active_tab().map(|t| t.path.clone());
        self.tabs.sort_by_key(|t| !t.pinned);
        self.active = active.and_then(|p| self.tab_index(&p));
    }

    /// Closes by saving: the app saves on every pause anyway.
    pub fn close_tab(&mut self, i: usize) {
        if i >= self.tabs.len() {
            return;
        }
        if self.tabs[i].dirty && !self.save_tab(i) {
            return;
        }
        self.remove_tab(i);
    }

    fn remove_tab(&mut self, i: usize) {
        let active_path = self.active_tab().map(|t| t.path.clone());
        self.tabs.remove(i);
        self.active = match active_path.and_then(|p| self.tab_index(&p)) {
            Some(a) => Some(a),
            None if self.tabs.is_empty() => None,
            None => Some(i.min(self.tabs.len() - 1)),
        };
        if let Some(a) = self.active {
            self.activate(a);
        } else {
            self.refresh_outline_now();
        }
    }

    fn remove_tabs_under(&mut self, path: &Path) {
        while let Some(i) = self.tabs.iter().position(|t| t.path.starts_with(path)) {
            self.remove_tab(i);
        }
    }

    fn save_dirty_tabs(&mut self) {
        for i in 0..self.tabs.len() {
            if self.tabs[i].dirty {
                self.save_tab(i);
            }
        }
    }

    /// Writes a tab; a disk conflict asks the user instead. Returns whether
    /// the tab is clean afterwards.
    fn save_tab(&mut self, i: usize) -> bool {
        let Some(root) = self.root.clone() else { return false };
        let tab = &self.tabs[i];
        if !tab.editable() || !tab.loaded {
            return true;
        }
        match project::write(&root, &tab.path, &tab.content, tab.disk_mtime) {
            Ok(r) if r.conflict => {
                let name = tab.name.clone();
                let path = tab.path.clone();
                if !self.dialogs.iter().any(|d| matches!(d, Dialog::Confirm(c) if matches!(&c.action, ConfirmAction::SaveConflict { path: p, .. } if *p == path))) {
                    self.dialogs.push_back(Dialog::Confirm(
                        Confirm::new(
                            &tf!("{name} changed outside the editor", name),
                            t("The file was modified on disk since it was last read. Overwrite it with the text open here, or reload from disk and lose the unsaved changes?"),
                            t("Overwrite"),
                            ConfirmAction::SaveConflict { path, disk_mtime: r.mod_time },
                        )
                        .cancel_label(t("Reload from disk")),
                    ));
                }
                false
            }
            Ok(r) => {
                let path = tab.path.clone();
                // Local history, so any saved state can be brought back.
                if let Err(e) = crate::backend::history::snapshot(&root, &path, &tab.content) {
                    log::warn!("local history: {e}");
                }
                let t = &mut self.tabs[i];
                t.disk_mtime = r.mod_time;
                t.mark_saved();
                if let Some(w) = &self.watcher {
                    w.suppress(&path);
                }
                self.save_indicator = tf!("Saved at {time}", time = Self::now_label());
                self.schedule_index();
                true
            }
            Err(e) => {
                let name = tab.name.clone();
                self.set_status(tf!("Could not save {name}: {e}", name, e), StatusKind::Error);
                false
            }
        }
    }

    fn reload_tab_from_disk(&mut self, i: usize) {
        let Some(root) = self.root.clone() else { return };
        let path = self.tabs[i].path.clone();
        // Unsaved edits being thrown away still go to the local history.
        if self.tabs[i].dirty && self.tabs[i].loaded {
            let _ = crate::backend::history::snapshot(&root, &path, &self.tabs[i].content);
        }
        match project::read(&root, &path) {
            Ok(r) => {
                let t = &mut self.tabs[i];
                t.content = r.content;
                t.disk_mtime = r.mod_time;
                t.loaded = true;
                t.mark_saved();
                t.editor.invalidate();
            }
            Err(e) => {
                let name = self.tabs[i].name.clone();
                self.set_status(tf!("Could not reload {name}: {e}", name, e), StatusKind::Error);
            }
        }
    }

    /// The editor reported an edit.
    fn on_edit(&mut self) {
        let now = self.now();
        if let Some(t) = self.active_tab_mut() {
            // Not every reported edit changes the text (a key with nothing to
            // delete, an undo back to the saved text).
            t.dirty = crate::editor::text_hash(&t.content, &[]) != t.saved;
        }
        self.save_indicator = t("Editing…").into();
        self.save_at = Some(now + SAVE_IDLE);
        // Quiet: a document mid-edit is often briefly broken, and that must
        // not pop the diagnostics panel up under the cursor.
        self.edited_since_compile = true;
        if self.s.auto_compile {
            let delay = self.s.compile_delay.clamp(0.3, 10.0) as f64;
            self.compile_at = Some((now + delay, true));
        }
        self.outline_at = Some(now + OUTLINE_IDLE);
    }

    fn on_cursor_line(&mut self, line: usize) {
        self.cursor_line = line;
        if self.suppress_sync_line == Some(line) {
            self.suppress_sync_line = None;
            return;
        }
        if self.s.sync_follow {
            self.sync_at = Some(self.now() + SYNC_IDLE);
        }
    }

    pub fn force_save_and_compile(&mut self) {
        self.save_at = None;
        self.compile_at = None;
        if let Some(i) = self.active {
            if self.tabs[i].dirty {
                self.save_tab(i);
            }
        }
        self.compile(false);
    }

    // ---------- Compilation and preview ----------

    fn set_main_file(&mut self, path: PathBuf, quiet: bool) {
        self.main_file = Some(path.clone());
        self.synctex = None;
        self.schedule_index();
        self.refresh_outline_now();
        if quiet {
            self.open_preview();
            return;
        }
        self.set_status(tf!("Main file set: {name}", name = project::file_name(&path)), StatusKind::Normal);
        // The previous document's PDF must not stay on screen as if it were this one's.
        self.open_preview();
        self.force_save_and_compile();
    }

    /// Mounts the PDF already on disk and, when it is older than its source,
    /// schedules a compilation, which keeps the preview from being born empty.
    fn open_preview(&mut self) {
        let Some(main) = self.main_file.clone() else {
            self.preview.clear(t("Set a main .tex file to make the preview."));
            return;
        };
        let status = latex::inspect_pdf(&main);
        if status.exists {
            self.preview.load(&self.ctx, &status.path);
            self.load_synctex(&main);
        } else {
            self.preview.clear(t("Compiling…"));
        }
        if status.stale {
            self.compile_at = Some((self.now() + OPEN_COMPILE_IDLE, true));
        }
    }

    /// `quiet` marks a compilation nobody asked for: it may show the result,
    /// but it may not take the screen.
    fn compile(&mut self, quiet: bool) {
        let Some(main) = self.main_file.clone() else {
            if !quiet {
                self.set_status(t("Set a main file to make the preview."), StatusKind::Normal);
            }
            return;
        };
        let Some(root) = self.root.clone() else { return };
        let abs = match project::resolve(&root, &main) {
            Ok(a) => a,
            Err(e) => {
                self.set_status(tf!("Could not compile: {e}", e), StatusKind::Error);
                return;
            }
        };
        // LaTeX reads from disk: whatever is being edited has to be there.
        self.save_at = None;
        self.save_dirty_tabs();
        self.edited_since_compile = false;
        if let Some(c) = self.compile_cancel.take() {
            c.store(true, Ordering::Relaxed); // drop the previous compilation
        }
        let cancel = Arc::new(AtomicBool::new(false));
        self.compile_cancel = Some(cancel.clone());
        self.compile_seq += 1;
        let seq = self.compile_seq;
        self.compiling = true;
        self.compile_started = self.now();
        self.set_status(t("Compiling…"), StatusKind::Normal);
        let confirmed = self.overwrite_confirmed.contains(&main);
        self.spawn(move || {
            if !confirmed && latex::foreign_pdf(&abs) {
                let result = CompileResult { overwrite_risk: true, ..Default::default() };
                return Msg::Compiled { seq, main, quiet, result: Ok(result) };
            }
            Msg::Compiled { seq, main, quiet, result: latex::compile(cancel, &abs) }
        });
    }

    fn on_compiled(&mut self, seq: u64, main: PathBuf, quiet: bool, result: Result<CompileResult, String>) {
        if seq != self.compile_seq {
            return; // a newer compilation already started
        }
        self.compiling = false;
        let result = match result {
            Ok(r) => r,
            Err(e) => {
                self.set_status(tf!("Could not compile: {e}", e), StatusKind::Error);
                // Nothing will come to replace "Compilando…": say why there.
                if !self.preview.has_document() {
                    self.preview.clear(&tf!("Could not compile: {e}", e));
                }
                return;
            }
        };
        if result.canceled {
            return;
        }
        if result.overwrite_risk {
            let pdf_name = project::file_name(&main.with_extension("pdf"));
            if quiet {
                self.set_status(
                    tf!("Automatic compilation paused: {pdf_name} already exists and was not made by this app.", pdf_name),
                    StatusKind::Error,
                );
                return;
            }
            self.dialogs.push_back(Dialog::Confirm(Confirm::new(
                t("Overwrite the existing PDF?"),
                &tf!("{pdf_name} already exists in this folder and does not seem to have been made by this app. Compiling will overwrite it, and this cannot be undone.", pdf_name),
                t("Compile anyway"),
                ConfirmAction::CompileOverwrite { main, quiet },
            )));
            return;
        }
        self.last_compile = Some((result.success, Self::now_label()));
        if result.success {
            if let Some(pdf) = &result.pdf_path {
                self.preview.load(&self.ctx, pdf);
                if let Some(w) = &self.watcher {
                    w.suppress(pdf);
                }
            }
            let passes = if result.passes >= 2 { format!(" · {}", tn("{n} pass", "{n} passes", result.passes as usize)) } else { String::new() };
            self.set_status(tf!("Compiled at {time} with {engine}", time = Self::now_label(), engine = result.engine) + &passes, StatusKind::Success);
            self.load_synctex(&main);
        } else if quiet {
            self.set_status(t("The automatic compilation found errors. See the marks in the editor."), StatusKind::Error);
        } else {
            self.set_status(t("Compilation error. See the details below."), StatusKind::Error);
        }
        // No PDF yet to keep showing: say so instead of "Compiling…".
        if !result.success && !self.preview.has_document() {
            self.preview.clear(t("The compilation failed. See the errors."));
        }
        if !result.errors.is_empty() {
            self.errors = result.errors;
            self.error_lines = source_lines(&self.errors);
            if !quiet {
                self.errors_open = true;
            }
        } else if !result.success {
            let tail: String = {
                let chars: Vec<char> = result.log_text.chars().collect();
                chars[chars.len().saturating_sub(2000)..].iter().collect()
            };
            self.errors = vec![CompileError { file: None, line: 0, message: tail, warning: false }];
            if !quiet {
                self.errors_open = true;
            }
        } else {
            self.errors.clear();
            self.errors_open = false;
        }
    }

    /// Parses the SyncTeX map in the background when it changed on disk.
    fn load_synctex(&mut self, main: &Path) {
        let Some(path) = synctex::file_for(main) else { return };
        let mtime = project::mtime_millis(&path);
        if let Some((p, m, _)) = &self.synctex {
            if *p == path && *m == mtime {
                self.sync_at = Some(self.now());
                return;
            }
        }
        if self.synctex_loading.as_ref() == Some(&(path.clone(), mtime)) {
            return;
        }
        self.synctex_loading = Some((path.clone(), mtime));
        self.spawn(move || {
            let map = synctex::parse(&path).map(Arc::new);
            Msg::SyncMap { path, mtime, map }
        });
    }

    /// Forward search: brings the preview to the material of the cursor line.
    fn sync_to_preview(&mut self) {
        if self.zen && !self.zen_reading {
            return;
        }
        let Some((_, _, map)) = &self.synctex else { return };
        let Some(tab) = self.active_tab() else { return };
        if !tab.editable() || self.cursor_line == 0 {
            return;
        }
        if let Some(pos) = map.forward(&tab.path, self.cursor_line) {
            let now = self.now();
            self.preview.reveal(pos, now);
        }
    }

    /// Inverse search: a click on the preview opens the source line and,
    /// when the words clicked can be found on or near it, puts the cursor
    /// right on them. A selection in the preview (`select`) selects the same
    /// text in the source.
    fn sync_to_editor(&mut self, page: usize, x: f64, y: f64, text: Option<&str>, select: bool) {
        let Some((_, _, map)) = &self.synctex else {
            self.set_status(t("No SyncTeX map yet. Compile the document to sync."), StatusKind::Normal);
            return;
        };
        let Some(source) = map.inverse(page, x, y) else { return };
        let Some(root) = self.root.clone() else { return };
        // synctex points at system files too (.cls, .sty).
        let Ok(file) = project::resolve(&root, &source.file) else { return };
        self.go_to(&file, source.line, true);
        let Some(text) = text.filter(|t| !t.trim().is_empty()) else { return };
        let Some(i) = self.active.filter(|&i| self.tabs[i].path == file && self.tabs[i].view == TabView::Editor) else { return };
        // The paragraph a line belongs to can start a few lines up.
        let radius = if select { 12 } else { 4 };
        let t = &mut self.tabs[i];
        match crate::textmatch::locate(&t.content, source.line, radius, text) {
            Some((a, b)) => {
                t.editor.pending_selection = Some(if select { (a, b) } else { (a, a) });
                t.editor.scroll_to_cursor = true;
                let line = t.content.chars().take(a).filter(|c| *c == '\n').count() + 1;
                let until = self.ctx.input(|i| i.time) + 1.6;
                t.editor.flash_line = Some((line, until));
                self.suppress_sync_line = Some(line);
            }
            None if select => {
                self.set_status(crate::i18n::t("That passage is not exactly in the source, so the cursor went to the matching line."), StatusKind::Normal);
            }
            None => {}
        }
    }

    fn go_to(&mut self, file: &Path, line: usize, flash: bool) {
        let kind = project::kind_of(&project::file_name(file), false);
        let kind = if kind.editable() { kind } else { Kind::Text };
        let same_file = self.active_tab().is_some_and(|t| t.path == file);
        if same_file {
            self.nav_jumping_to(file, line);
        }
        if !self.open_file(file, kind) {
            return;
        }
        let now = self.now();
        let Some(i) = self.active else { return };
        let t = &mut self.tabs[i];
        if t.view != TabView::Editor {
            return;
        }
        t.editor.go_to_line(&t.content, line, flash.then_some(now + 1.6));
        if flash {
            self.suppress_sync_line = Some(line);
        }
    }

    // ---------- Index, outline, project data ----------

    fn schedule_index(&mut self) {
        self.index_at = Some(self.now() + INDEX_IDLE);
    }

    fn refresh_project_data(&mut self) {
        let Some(root) = self.root.clone() else { return };
        let main = self.main_file.clone();
        let versions = self.s.folders.path(crate::settings::FolderKind::Versions);
        self.spawn(move || Msg::ProjectData {
            index: crate::backend::index::build(&root, main.as_deref()).ok(),
            refs: references::build(&root).ok(),
            notes: crate::backend::notes::list(&root).unwrap_or_default(),
            todos: todos::build(&root),
            papers: crate::backend::papers::list(&root, main.as_deref(), &versions).unwrap_or_default(),
            images: crate::backend::images::list(&root, main.as_deref()).unwrap_or_default(),
            root,
        });
    }

    fn refresh_outline_now(&mut self) {
        self.outline_at = None;
        match self.active_tab() {
            Some(t) if t.is_tex() => {
                self.outline = outline::parse(&t.content);
                self.outline_hint = None;
            }
            Some(_) => {
                self.outline.clear();
                self.outline_hint = Some(t("The outline is shown for .tex files.").into());
            }
            None => {
                self.outline.clear();
                self.outline_hint = Some(t("No file open.").into());
            }
        }
        self.refresh_document();
    }

    /// Rereads the whole document: open files from their tabs (what is being
    /// typed), the others from disk.
    fn refresh_document(&mut self) {
        let main = self.main_file.clone().filter(|m| project::is_document_source(m));
        let tabs = &self.tabs;
        let read = |p: &Path| tabs.iter().find(|t| t.path == p && t.loaded).map(|t| t.content.clone()).or_else(|| std::fs::read_to_string(p).ok());
        self.doc = main.map(|main| Arc::new(document::build(&main, &read)));
        // Words of the text, to tell whether it is written in the language
        // its preamble declares.
        let mut sample = Vec::new();
        for file in self.doc.iter().flat_map(|d| d.files.iter()) {
            let Some(text) = read(file) else { continue };
            let body = &text[crate::editor::highlight::body(&text)];
            for start in crate::editor::highlight::prose_word_starts(body) {
                let word: String = body[start..].chars().take_while(|c| c.is_alphabetic()).collect();
                if word.chars().count() > 2 {
                    sample.push(word.to_lowercase());
                }
            }
            if sample.len() >= 400 {
                break;
            }
        }
        sample.truncate(400);
        self.spell.set_language(self.doc.as_ref().and_then(|d| d.language), sample);
    }

    /// The outline to show: the whole document's when the open file is part
    /// of it, else the open file's. Words under each heading include its
    /// subsections'. Also returns the row the cursor is under.
    pub fn outline_rows(&self) -> (Vec<OutlineRow>, Option<usize>) {
        let active = self.active_tab().map(|t| t.path.clone());
        let doc = self.doc.as_ref().filter(|d| active.as_ref().is_none_or(|a| d.contains(a)) && !d.headings.is_empty());
        let (mut rows, current): (Vec<OutlineRow>, Option<usize>) = match (doc, &active) {
            (Some(doc), _) => (
                doc.headings.iter().map(|h| OutlineRow { file: h.file.clone(), entry: h.entry.clone(), words: Some(h.words) }).collect(),
                active.as_ref().and_then(|a| doc.heading_at(a, self.cursor_line)),
            ),
            (None, Some(active)) => (
                self.outline.iter().map(|e| OutlineRow { file: active.clone(), entry: e.clone(), words: None }).collect(),
                self.outline.iter().rposition(|e| e.line <= self.cursor_line),
            ),
            (None, None) => (Vec::new(), None),
        };
        // A chapter counts its sections' words too.
        let own: Vec<Option<usize>> = rows.iter().map(|r| r.words).collect();
        for i in 0..rows.len() {
            if let Some(w) = own[i] {
                let deeper = rows[i + 1..].iter().zip(&own[i + 1..]).take_while(|(r, _)| r.entry.level > rows[i].entry.level);
                rows[i].words = Some(w + deeper.map(|(_, w)| w.unwrap_or(0)).sum::<usize>());
            }
        }
        (rows, current)
    }

    /// Prose words of the whole document, when the open file is part of one
    /// with more than one file.
    pub fn document_words(&self) -> Option<usize> {
        let active = self.active_tab()?;
        self.doc.as_ref().filter(|d| d.files.len() > 1 && d.contains(&active.path)).map(|d| d.words())
    }

    // ---------- Messages and timers ----------

    fn poll_messages(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::Compiled { seq, main, quiet, result } => self.on_compiled(seq, main, quiet, result),
                Msg::SyncMap { path, mtime, map } => {
                    if self.synctex_loading.as_ref() == Some(&(path.clone(), mtime)) {
                        self.synctex_loading = None;
                    }
                    if let Ok(map) = map {
                        self.synctex = Some((path, mtime, map));
                        self.sync_at = Some(self.now());
                    }
                }
                Msg::ProjectData { root, index, refs, notes, todos, papers, images } => {
                    if self.root.as_deref() != Some(root.as_path()) {
                        continue;
                    }
                    if let Some(index) = index {
                        self.completions.set_index(&index);
                        self.math.set_preamble(&index.math_preamble);
                        self.index = index;
                    }
                    self.refs = refs;
                    self.notes = notes;
                    self.todos = todos;
                    self.papers = papers;
                    self.images = images;
                }
                Msg::Packages(list) => self.completions.set_packages(&list),
                Msg::FsChanged(paths) => self.handle_external_changes(paths),
                Msg::Imported(result) => match result {
                    Ok(entry) => {
                        if let Some(w) = &self.watcher {
                            w.suppress(&entry.file);
                        }
                        self.reload_clean_tab(&entry.file);
                        self.refresh_tree();
                        self.refresh_project_data();
                        self.set_status(tf!("Reference imported: {key}.", key = entry.key), StatusKind::Success);
                    }
                    Err(e) => self.set_status(tf!("Could not import the reference: {e}", e), StatusKind::Error),
                },
                Msg::Engine(engine) => {
                    self.engine = Some(engine.clone());
                    if !engine.available {
                        self.set_status(
                            t("No LaTeX compiler found on the PATH. Install a LaTeX distribution (TeX Live, MacTeX or MiKTeX)."),
                            StatusKind::Error,
                        );
                    } else if !engine.bibtex && self.status.0 == t("Project opened.") {
                        // Only what changes the result is worth the status bar.
                        self.set_status(t("Project opened. No latexmk or bibtex: \\cite{} will not resolve."), StatusKind::Normal);
                    }
                }
                Msg::FolderPicked(Some(path)) => match self.open_project_path(&path) {
                    Ok(()) => {
                        self.set_status(t("Project opened."), StatusKind::Normal);
                        self.finish_opening_project();
                    }
                    Err(e) => self.set_status(tf!("Could not open the project: {e}", e), StatusKind::Error),
                },
                Msg::FolderPicked(None) | Msg::NewProjectPicked(None, _) => {}
                Msg::NewProjectPicked(Some(dir), template) => match crate::backend::templates::create(&dir, template) {
                    Ok(main) => match self.open_project_path(&dir) {
                        Ok(()) => {
                            self.finish_opening_project();
                            self.set_main_file(main.clone(), true);
                            self.open_file(&main, Kind::Tex);
                            self.set_status(tf!("Project created: {template}.", template = template.label()), StatusKind::Success);
                        }
                        Err(e) => self.set_status(tf!("Could not open the project: {e}", e), StatusKind::Error),
                    },
                    Err(e) => self.set_status(tf!("Could not create the project: {e}", e), StatusKind::Error),
                },
                Msg::PapersPicked(paths) => self.add_papers(paths),
                Msg::ImagesPicked(paths) => self.add_images(paths),
            }
        }
    }

    /// A tab showing a file the app itself just changed on disk (an import
    /// appended to the .bib) gets the new content when it has no edits.
    fn reload_clean_tab(&mut self, path: &Path) {
        if let Some(i) = self.tab_index(path) {
            if !self.tabs[i].dirty && self.tabs[i].editable() {
                self.reload_tab_from_disk(i);
            }
        }
    }

    /// The disk watcher reported changes: refresh the tree, reload clean
    /// tabs quietly and ask about dirty ones.
    fn handle_external_changes(&mut self, paths: Vec<PathBuf>) {
        self.refresh_tree();
        for path in paths {
            let Some(i) = self.tab_index(&path) else { continue };
            if self.tabs[i].kind == Kind::Pdf {
                if let Some(v) = self.tabs[i].pdf.as_mut() {
                    v.load(&self.ctx, &path);
                }
                continue;
            }
            if self.tabs[i].kind == Kind::Image || self.tabs[i].kind == Kind::Svg {
                self.ctx.forget_image(&format!("file://{}", path.display()));
            }
            if !self.tabs[i].editable() {
                continue;
            }
            if !self.tabs[i].dirty {
                self.reload_tab_from_disk(i);
                continue;
            }
            let name = self.tabs[i].name.clone();
            self.dialogs.push_back(Dialog::Confirm(
                Confirm::new(
                    &tf!("{name} changed outside the editor", name),
                    t("This tab has unsaved changes. Reloading discards what you edited here; keeping them keeps your changes, which will overwrite the disk the next time you save."),
                    t("Reload from disk"),
                    ConfirmAction::ExternalChange(path),
                )
                .cancel_label(t("Keep my changes")),
            ));
        }
    }

    fn run_timers(&mut self) {
        let now = self.now();
        let due = |t: Option<f64>| t.map(|t| now >= t).unwrap_or(false);
        if due(self.save_at) {
            self.save_at = None;
            if let Some(i) = self.active {
                if self.tabs[i].dirty {
                    self.save_tab(i);
                }
            }
        }
        if let Some((t, quiet)) = self.compile_at {
            if now >= t {
                self.compile_at = None;
                self.compile(quiet);
            }
        }
        if due(self.outline_at) {
            self.refresh_outline_now();
        }
        if due(self.sync_at) {
            self.sync_at = None;
            self.sync_to_preview();
        }
        if due(self.index_at) {
            self.index_at = None;
            self.refresh_project_data();
        }
        let next = [self.save_at, self.compile_at.map(|c| c.0), self.outline_at, self.sync_at, self.index_at]
            .into_iter()
            .flatten()
            .fold(f64::INFINITY, f64::min);
        if next.is_finite() {
            self.ctx.request_repaint_after(std::time::Duration::from_secs_f64((next - now).max(0.0) + 0.01));
        }
    }

    // ---------- Dialog outcomes ----------

    fn show_dialogs(&mut self, ctx: &egui::Context) {
        let Some(front) = self.dialogs.front_mut() else { return };
        match front {
            Dialog::Prompt(p) => {
                if let Some(outcome) = p.show(ctx) {
                    let Some(Dialog::Prompt(p)) = self.dialogs.pop_front() else { return };
                    if let Some(value) = outcome {
                        self.on_prompt(p.action, value);
                    }
                }
            }
            Dialog::Confirm(c) => {
                if let Some(yes) = c.show(ctx) {
                    let Some(Dialog::Confirm(c)) = self.dialogs.pop_front() else { return };
                    self.on_confirm(c.action, yes);
                }
            }
        }
    }

    fn on_confirm(&mut self, action: ConfirmAction, yes: bool) {
        match action {
            ConfirmAction::Delete(path) if yes => self.delete_node(&path),
            ConfirmAction::SaveConflict { path, disk_mtime } => {
                let Some(i) = self.tab_index(&path) else { return };
                if yes {
                    self.tabs[i].disk_mtime = disk_mtime;
                    self.save_tab(i);
                } else {
                    self.reload_tab_from_disk(i);
                }
            }
            ConfirmAction::ExternalChange(path) => {
                let Some(i) = self.tab_index(&path) else { return };
                if yes {
                    self.reload_tab_from_disk(i);
                } else {
                    // Keep local edits, but acknowledge the disk's new state so
                    // the next save does not trip the same conflict again.
                    self.tabs[i].disk_mtime = project::mtime_millis(&path);
                }
            }
            ConfirmAction::CompileOverwrite { main, quiet } => {
                if yes {
                    self.overwrite_confirmed.insert(main);
                    self.compile(quiet);
                } else {
                    self.set_status(t("Compilation canceled so as not to overwrite the existing PDF."), StatusKind::Normal);
                }
            }
            ConfirmAction::ReplaceAll { query, replacement, files } if yes => self.replace_all_in_project(&query, &replacement, &files),
            ConfirmAction::RemoveReference { file, key } if yes => {
                self.edit_bib(&file, |f| references::remove_entry(f, &key), tf!("{key} removed.", key = key.as_str()));
            }
            ConfirmAction::RemovePaper { path } if yes => {
                let Some(root) = self.root.clone() else { return };
                match crate::backend::papers::remove(&root, &path) {
                    Ok(()) => {
                        if let Some(i) = self.tab_index(&path) {
                            self.remove_tab(i);
                        }
                        self.refresh_tree();
                        self.refresh_project_data();
                    }
                    Err(e) => self.set_status(tf!("Could not remove the paper: {e}", e), StatusKind::Error),
                }
            }
            _ => {}
        }
    }

    fn show_windows(&mut self, ctx: &egui::Context) {
        let items = if self.palette.is_some() { self.palette_items() } else { vec![] };
        if let Some(p) = &mut self.palette {
            if let Some(outcome) = p.show(ctx, &items, &self.s.palette_recent) {
                self.palette = None;
                if let Some(cmd) = outcome {
                    self.run_from_palette(cmd, &items);
                }
            }
        }
        if let Some(sm) = &mut self.snippet_manager {
            let builtin = crate::editor::completion::builtin_triggers();
            let (open, changed) = sm.show(ctx, &mut self.s.snippets, &builtin);
            if changed {
                self.completions.set_snippets(&self.s.snippets);
            }
            if !open {
                self.snippet_manager = None;
            }
        }
        if self.shortcuts_open {
            let groups = self.shortcut_groups();
            self.shortcuts_open = dialogs::shortcuts(ctx, &groups);
        }
        if let Some(preferences) = &mut self.preferences {
            // What changing a setting sets off, besides storing it.
            let before = (self.s.language, self.s.dark, self.s.spell_check, self.s.system_tex, self.s.accent);
            if !preferences.show(ctx, &mut self.s) {
                self.preferences = None;
            }
            if self.s.language != before.0 {
                self.set_language(self.s.language);
            }
            if self.s.dark != before.1 {
                self.apply_theme();
            }
            if self.s.accent != before.4 {
                crate::theme::set_accent(ctx, self.s.accent.color());
            }
            if self.s.system_tex != before.3 {
                latex::set_system_tex(self.s.system_tex);
                self.spawn(|| Msg::Engine(latex::detect_engine()));
                if self.main_file.is_some() {
                    self.force_save_and_compile();
                }
            }
            if self.s.spell_check != before.2 {
                if self.s.spell_check {
                    self.spell.ensure_loading(ctx);
                }
                for t in &mut self.tabs {
                    t.editor.invalidate();
                }
            }
        }
        if let Some(tb) = &mut self.table_builder {
            if let Some(outcome) = tb.show(ctx) {
                self.table_builder = None;
                if let Some(latex) = outcome {
                    self.insert_text(&latex);
                }
            }
        }
        if let Some(ib) = &mut self.image_builder {
            if let Some(insert) = ib.show(ctx) {
                let builder = self.image_builder.take();
                if let Some(builder) = builder.filter(|_| insert) {
                    self.insert_built_image(builder);
                }
            }
        }
        if let Some(mb) = &mut self.math_builder {
            if let Some(outcome) = mb.show(ctx) {
                let (kind, replaces) = (mb.kind, mb.replaces.take());
                self.math_builder = None;
                if let Some(latex) = outcome {
                    self.insert_math(kind, replaces, &latex);
                }
            }
        }
        if let Some(lb) = &mut self.list_builder {
            if let Some(outcome) = lb.show(ctx) {
                self.list_builder = None;
                if let Some(latex) = outcome {
                    self.insert_text(&latex);
                }
            }
        }
        if let Some(ar) = &mut self.add_ref {
            if let Some(outcome) = ar.show(ctx) {
                self.add_ref = None;
                if let Some(new) = outcome {
                    self.add_reference(new);
                }
            }
        }
        if let Some(hd) = &mut self.history_dialog {
            if let Some(action) = hd.show(ctx) {
                let file = hd.file.clone();
                self.history_dialog = None;
                if let dialogs::HistoryAction::Restore(text) = action {
                    self.restore_version(&file, text);
                }
            }
        }
        if let Some(pm) = &mut self.paper_meta {
            let keys: Vec<String> = self.index.citations.iter().map(|c| c.key.clone()).collect();
            if let Some(outcome) = pm.show(ctx, &keys) {
                let path = pm.path.clone();
                self.paper_meta = None;
                if let (Some((key, summary)), Some(root)) = (outcome, self.root.clone()) {
                    match crate::backend::papers::set_meta(&root, &path, &key, &summary) {
                        Ok(()) => self.refresh_project_data(),
                        Err(e) => self.set_status(tf!("Could not save: {e}", e), StatusKind::Error),
                    }
                }
            }
        }
    }

    /// Puts a version from the local history back into the file's tab (the
    /// text being replaced goes to the history first, and Ctrl+Z brings it
    /// back).
    fn restore_version(&mut self, file: &Path, text: String) {
        let Some(root) = self.root.clone() else { return };
        if !self.open_file(file, Kind::Tex) {
            return;
        }
        let Some(i) = self.active else { return };
        let _ = crate::backend::history::snapshot(&root, file, &self.tabs[i].content);
        let t = &mut self.tabs[i];
        t.editor.undo_point(&self.ctx, &t.content);
        t.content = text;
        t.editor.invalidate();
        t.editor.pending_selection = Some((0, 0));
        self.on_edit();
        self.set_status(crate::i18n::t("Version restored. The previous text is in the local history."), StatusKind::Success);
    }

    /// Files dragged over the window get a hint of what dropping will do;
    /// dropped ones are handled.
    fn file_drop(&mut self, ctx: &egui::Context) {
        let (hovered, dropped) = ctx.input(|i| {
            (
                i.raw.hovered_files.iter().filter_map(|f| f.path.clone()).collect::<Vec<_>>(),
                i.raw.dropped_files.iter().map(|f| f.path().to_path_buf()).collect::<Vec<_>>(),
            )
        });
        if !dropped.is_empty() {
            self.handle_dropped_files(dropped);
            return;
        }
        if hovered.is_empty() {
            return;
        }
        let what = if hovered.iter().any(|p| p.is_dir()) {
            t("Drop to open the folder as a project")
        } else if self.root.is_none() {
            t("Drop a .tex to open its folder")
        } else if hovered.iter().all(|p| matches!(project::kind_of(&project::file_name(p), false), Kind::Image | Kind::Svg)) {
            t("Drop to add the image to the project and insert the figure")
        } else {
            t("Drop to open in the project")
        };
        let screen = ctx.content_rect();
        let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("file-drop")));
        painter.rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
        painter.rect_stroke(screen.shrink(10.0), 12.0, egui::Stroke::new(2.0, crate::theme::accent()), egui::StrokeKind::Inside);
        painter.text(screen.center(), egui::Align2::CENTER_CENTER, what, egui::FontId::proportional(20.0), egui::Color32::WHITE);
    }

    /// The search panel in the sidebar: results stay listed while they are
    /// opened; "Replace all" still asks first.
    pub fn search_view(&mut self, ui: &mut egui::Ui) {
        let root = self.root.clone();
        match self.project_search.ui(ui, root.as_deref()) {
            Some(dialogs::SearchAction::Search(q)) => {
                if let Some(root) = &root {
                    let ps = &mut self.project_search;
                    ps.results = crate::backend::search::find(root, &q).unwrap_or_default();
                    ps.searched = Some(q);
                    ps.summary = None;
                }
            }
            Some(dialogs::SearchAction::GoTo(m)) => self.go_to(&m.file, m.line, true),
            Some(dialogs::SearchAction::ReplaceAll(q, r)) => {
                let mut files: Vec<PathBuf> = self.project_search.results.iter().map(|m| m.file.clone()).collect();
                files.dedup();
                let n = files.len();
                self.dialogs.push_back(Dialog::Confirm(Confirm::new(
                    t("Replace in the whole project?"),
                    &crate::i18n::tn(
                        "This replaces every occurrence of \"{query}\" with \"{replacement}\" in {n} file. This cannot be undone.",
                        "This replaces every occurrence of \"{query}\" with \"{replacement}\" in {n} files. This cannot be undone.",
                        n,
                    )
                    .replace("{query}", &q)
                    .replace("{replacement}", if r.is_empty() { t("(nothing)") } else { &r }),
                    t("Replace"),
                    ConfirmAction::ReplaceAll { query: q, replacement: r, files },
                )));
            }
            None => {}
        }
    }

    fn any_modal_open(&self) -> bool {
        !self.dialogs.is_empty()
            || self.palette.is_some()
            || self.snippet_manager.is_some()
            || self.shortcuts_open
            || self.table_builder.is_some()
            || self.list_builder.is_some()
            || self.math_builder.is_some()
            || self.image_builder.is_some()
            || self.add_ref.is_some()
            || self.paper_meta.is_some()
            || self.history_dialog.is_some()
            || self.tour.is_some()
    }

    /// Starts the tour once the first project is open, then draws it.
    fn run_tour(&mut self, ctx: &egui::Context) {
        if self.tour.is_none() && !self.s.tour_seen && self.root.is_some() && !self.zen && !self.any_modal_open() && self.spots.editor.is_some() {
            self.s.tour_seen = true;
            self.tour = Some(tour::Tour::default());
        }
        if let Some(t) = &mut self.tour {
            if !t.show(ctx, &self.spots) {
                self.tour = None;
            }
        }
    }
}

impl eframe::App for App {
    fn raw_input_hook(&mut self, ctx: &egui::Context, raw_input: &mut egui::RawInput) {
        #[cfg(debug_assertions)]
        self.debug_inject(ctx, raw_input);
        let now = raw_input.time.unwrap_or_else(|| ctx.input(|i| i.time));
        self.swipe.feed(raw_input, now, navigate::sideways_claimed(ctx));
        self.kinetic.feed(ctx, raw_input);
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        #[cfg(debug_assertions)]
        self.debug_frame_begin();
        // Following the desktop's accent: asked again when the window gets
        // the focus back (it may have changed meanwhile), applied once the
        // answer is in.
        if self.s.accent == crate::settings::Accent::System {
            let focused = ctx.input(|i| i.viewport().focused.unwrap_or(true));
            if focused && !self.was_focused {
                crate::settings::recheck_system_accent();
            }
            self.was_focused = focused;
            crate::theme::set_accent(&ctx, self.s.accent.color());
        }
        if !self.window_settled {
            self.window_settled = window::leave_saved_fullscreen(&ctx, self.zen);
        }
        window::keep_restorable(&ctx);
        self.poll_messages();
        self.handle_tray(&ctx);
        self.spell.poll();
        if let Some(e) = self.spell.take_error() {
            self.set_status(e, StatusKind::Error);
        }
        if !self.any_modal_open() {
            self.handle_shortcuts(&ctx);
            if let Some(back) = self.swipe.pending.take() {
                self.nav_step(back);
            }
        }
        self.swipe.pending = None;
        self.run_timers();

        // Save everything before the window goes away.
        if ctx.input(|i| i.viewport().close_requested()) && !self.close_confirmed {
            self.close_confirmed = true;
            self.save_dirty_tabs();
        }

        self.draw(ui);
        self.run_tour(&ctx);
        self.file_drop(&ctx);
        self.show_dialogs(&ctx);
        self.show_windows(&ctx);
        self.update_title(&ctx);
        crate::theme::pointing_hand(&ctx);
        // Last, so its cursor wins over the widgets under the edges.
        window::edges(&ctx);
        window::border(&ctx);
        #[cfg(debug_assertions)]
        self.debug_hooks(&ctx);
        #[cfg(debug_assertions)]
        self.debug_frame_end();
    }

    fn save(&mut self, storage: &mut dyn eframe::Storage) {
        self.s.session = self.current_session();
        self.s.ignored_words = self.spell.ignored.iter().cloned().collect();
        eframe::set_value(storage, eframe::APP_KEY, &self.s);
    }

    fn auto_save_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(10)
    }
}

/// The source line each error points at, read once from disk.
fn source_lines(errors: &[CompileError]) -> HashMap<(PathBuf, usize), String> {
    let mut texts: HashMap<&Path, Option<String>> = HashMap::new();
    let mut out = HashMap::new();
    for e in errors {
        let Some(file) = e.file.as_deref().filter(|_| e.line > 0) else { continue };
        let text = texts.entry(file).or_insert_with(|| std::fs::read_to_string(file).ok());
        if let Some(line) = text.as_deref().and_then(|t| t.lines().nth(e.line - 1)) {
            out.insert((file.to_path_buf(), e.line), line.to_string());
        }
    }
    out
}
