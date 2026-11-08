//! Everything that survives a restart: preferences, the layout, recent
//! projects, favorites, snippets and the last session (project, tabs, active
//! tab and cursor line). Stored through eframe's persistence.

use crate::i18n::{n, Language};
use crate::icons::Icon;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Dock {
    #[default]
    Right,
    Left,
    Top,
    Bottom,
}

impl Dock {
    pub const ALL: [(Dock, &'static str); 4] =
        [(Dock::Right, n("Right")), (Dock::Left, n("Left")), (Dock::Top, n("Top")), (Dock::Bottom, n("Bottom"))];
}

/// The editor's typeface.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum EditorFont {
    /// Every character the same width: markup and tables line up.
    #[default]
    Monospace,
    /// The interface's face: long prose reads more easily.
    Proportional,
}

/// How the references sidebar lists the entries.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum RefSort {
    /// As in the .bib file, where they can be dragged into another order.
    #[default]
    File,
    Author,
    Year,
    Title,
}

impl RefSort {
    pub const ALL: [(RefSort, &'static str); 4] =
        [(RefSort::File, n("File order")), (RefSort::Author, n("Author")), (RefSort::Year, n("Year")), (RefSort::Title, n("Title"))];

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(s, _)| *s == self).map(|(_, l)| crate::i18n::t(l)).unwrap_or("")
    }
}

/// The accent colour: the app's terracotta, one of the colours GNOME offers
/// (the same nine, so the app can match a desktop set to any of them), or
/// whatever the desktop is set to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Accent {
    #[default]
    Terracotta,
    /// GNOME's accent (Settings → Appearance), read when the app starts and
    /// when its window gets the focus.
    System,
    Blue,
    Teal,
    Green,
    Yellow,
    Orange,
    Red,
    Pink,
    Purple,
    Slate,
}

impl Accent {
    /// The fixed ones, in GNOME's order after the app's own.
    pub const COLORS: [(Accent, &'static str, [u8; 3]); 10] = [
        (Accent::Terracotta, n("Terracotta"), [0xD9, 0x77, 0x57]),
        (Accent::Blue, n("Blue"), [0x35, 0x84, 0xE4]),
        (Accent::Teal, n("Teal"), [0x21, 0x90, 0xA4]),
        (Accent::Green, n("Green"), [0x3A, 0x94, 0x4A]),
        (Accent::Yellow, n("Yellow"), [0xC8, 0x88, 0x00]),
        (Accent::Orange, n("Orange"), [0xED, 0x5B, 0x00]),
        (Accent::Red, n("Red"), [0xE6, 0x2D, 0x42]),
        (Accent::Pink, n("Pink"), [0xD5, 0x61, 0x99]),
        (Accent::Purple, n("Purple"), [0x91, 0x41, 0xAC]),
        (Accent::Slate, n("Slate"), [0x6F, 0x83, 0x96]),
    ];

    /// Its colour; the system's, or the app's own when there is none.
    pub fn color(self) -> egui::Color32 {
        let fixed = |a: Accent| Self::COLORS.iter().find(|(c, _, _)| *c == a).map(|(_, _, [r, g, b])| egui::Color32::from_rgb(*r, *g, *b));
        match self {
            Accent::System => system_accent().and_then(fixed).unwrap_or(crate::theme::TERRACOTTA),
            other => fixed(other).unwrap_or(crate::theme::TERRACOTTA),
        }
    }
}

/// What the desktop said its accent is, and whether it is being asked.
struct SystemAccent {
    value: Option<Accent>,
    asked: Option<std::time::Instant>,
    busy: bool,
    ctx: Option<egui::Context>,
}

static SYSTEM_ACCENT: std::sync::Mutex<SystemAccent> = std::sync::Mutex::new(SystemAccent { value: None, asked: None, busy: false, ctx: None });

/// The desktop's accent, where GNOME (47 and later) says it: the last answer,
/// at once. Asking runs `gsettings`, off the interface's thread (a process
/// that hangs must not freeze the window), at most every few seconds; a new
/// answer repaints the window.
pub fn system_accent() -> Option<Accent> {
    let mut state = SYSTEM_ACCENT.lock().unwrap_or_else(|e| e.into_inner());
    let stale = state.asked.is_none_or(|at| at.elapsed() > std::time::Duration::from_secs(3));
    if stale && !state.busy {
        state.busy = true;
        state.asked = Some(std::time::Instant::now());
        std::thread::spawn(|| {
            let value = read_system_accent();
            let mut state = SYSTEM_ACCENT.lock().unwrap_or_else(|e| e.into_inner());
            let changed = state.value != value;
            state.value = value;
            state.busy = false;
            if let Some(ctx) = state.ctx.as_ref().filter(|_| changed) {
                ctx.request_repaint();
            }
        });
    }
    state.value
}

/// Where to ask for a repaint when the desktop's accent changes.
pub fn watch_system_accent(ctx: &egui::Context) {
    SYSTEM_ACCENT.lock().unwrap_or_else(|e| e.into_inner()).ctx = Some(ctx.clone());
}

/// Asks the desktop again on the next call (the window got the focus back).
pub fn recheck_system_accent() {
    SYSTEM_ACCENT.lock().unwrap_or_else(|e| e.into_inner()).asked = None;
}

fn read_system_accent() -> Option<Accent> {
    let out = std::process::Command::new("gsettings").args(["get", "org.gnome.desktop.interface", "accent-color"]).output().ok()?;
    let name = String::from_utf8_lossy(&out.stdout).trim().trim_matches('\'').to_lowercase();
    Some(match name.as_str() {
        "blue" => Accent::Blue,
        "teal" => Accent::Teal,
        "green" => Accent::Green,
        "yellow" => Accent::Yellow,
        "orange" => Accent::Orange,
        "red" => Accent::Red,
        "pink" => Accent::Pink,
        "purple" => Accent::Purple,
        "slate" => Accent::Slate,
        _ => return None,
    })
}

/// How the images sidebar lists them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum ImageSort {
    #[default]
    Name,
    /// The biggest first.
    Size,
    /// The least used first: what could go.
    Uses,
}

impl ImageSort {
    pub const ALL: [(ImageSort, &'static str); 3] = [(ImageSort::Name, n("Name")), (ImageSort::Size, n("Size")), (ImageSort::Uses, n("Use"))];

    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(s, _)| *s == self).map(|(_, l)| crate::i18n::t(l)).unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum SidebarView {
    #[default]
    Tree,
    References,
    Outline,
    Knowledge,
    Images,
    Search,
}

impl SidebarView {
    pub const ALL: [(SidebarView, Icon, &'static str); 6] = [
        (SidebarView::Tree, Icon::Files, n("Files")),
        (SidebarView::References, Icon::Library, n("References")),
        (SidebarView::Outline, Icon::ListTree, n("Outline")),
        (SidebarView::Knowledge, Icon::Notebook, n("Notes")),
        (SidebarView::Images, Icon::Images, n("Images")),
        (SidebarView::Search, Icon::Search, n("Search")),
    ];

    /// Translated.
    pub fn label(self) -> &'static str {
        Self::ALL.iter().find(|(v, _, _)| *v == self).map(|(_, _, l)| crate::i18n::t(l)).unwrap_or("")
    }
}

/// What the project keeps in a folder of its own.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FolderKind {
    Images,
    Papers,
    Notes,
    Versions,
}

impl FolderKind {
    pub const ALL: [(FolderKind, &'static str); 4] = [
        (FolderKind::Images, n("Images")),
        (FolderKind::Papers, n("Papers")),
        (FolderKind::Notes, n("Notes")),
        (FolderKind::Versions, n("PDF versions")),
    ];
}

/// Where what is added to the project goes, relative to its root. Only
/// where new files land: the sidebar lists every file of a kind, wherever
/// it is.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Folders {
    pub images: String,
    pub papers: String,
    pub notes: String,
    pub versions: String,
}

impl Default for Folders {
    fn default() -> Self {
        Self { images: "images".into(), papers: "papers".into(), notes: "notes".into(), versions: "versions".into() }
    }
}

impl Folders {
    pub fn get_mut(&mut self, kind: FolderKind) -> &mut String {
        match kind {
            FolderKind::Images => &mut self.images,
            FolderKind::Papers => &mut self.papers,
            FolderKind::Notes => &mut self.notes,
            FolderKind::Versions => &mut self.versions,
        }
    }

    pub fn get(&self, kind: FolderKind) -> &str {
        match kind {
            FolderKind::Images => &self.images,
            FolderKind::Papers => &self.papers,
            FolderKind::Notes => &self.notes,
            FolderKind::Versions => &self.versions,
        }
    }

    /// The folder, relative to the project's root: the default's while
    /// what was typed is not a valid one.
    pub fn path(&self, kind: FolderKind) -> PathBuf {
        let default = Self::default();
        PathBuf::from(check_folder(self.get(kind)).unwrap_or_else(|_| default.get(kind).to_string()))
    }
}

/// A folder inside the project, `/`-separated, or why it cannot be one.
/// Hidden folders are refused too: the sidebar never looks in them.
pub fn check_folder(value: &str) -> Result<String, &'static str> {
    let value = value.trim().replace('\\', "/");
    if value.starts_with('/') || value.contains(':') {
        return Err(n("Must be a folder inside the project, not an absolute path."));
    }
    let parts: Vec<&str> = value.split('/').map(str::trim).filter(|p| !p.is_empty()).collect();
    if parts.is_empty() {
        return Err(n("Type a folder name."));
    }
    for part in &parts {
        if *part == ".." {
            return Err(n("Must be a folder inside the project."));
        }
        if part.starts_with('.') {
            return Err(n("Hidden folders (starting with a dot) are not allowed."));
        }
        if part.contains(['<', '>', '"', '|', '?', '*']) {
            return Err(n("Contains characters not allowed in folder names."));
        }
    }
    Ok(parts.join("/"))
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Snippet {
    pub id: u64,
    /// Starts with `\`: what the autocomplete matches against.
    pub trigger: String,
    /// `|` marks where the cursor lands.
    pub body: String,
    pub description: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct RecentProject {
    pub path: PathBuf,
    pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SessionTab {
    pub path: PathBuf,
    pub pinned: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub project_root: PathBuf,
    pub main_file: Option<PathBuf>,
    pub tabs: Vec<SessionTab>,
    pub active: Option<PathBuf>,
    pub cursor_line: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Which one-time updates of saved settings have run (see
    /// `Settings::migrate`). Missing in older files, hence 0.
    #[serde(default)]
    pub revision: u32,
    /// The interface language.
    pub language: Language,
    pub dark: bool,
    pub accent: Accent,
    pub typewriter: bool,
    pub spell_check: bool,
    /// The writing reviewer underlines what it finds in the prose.
    pub review: bool,
    /// Lines wrap at a comfortable length instead of the window's edge.
    pub comfortable_width: bool,
    pub font_size: f32,
    pub editor_font: EditorFont,
    pub preview_dock: Dock,
    pub sidebar_hidden: bool,
    pub preview_hidden: bool,
    pub sidebar_view: SidebarView,
    pub ref_sort: RefSort,
    pub image_sort: ImageSort,
    /// The command palette's latest commands, newest first (`Cmd::recent_id`).
    pub palette_recent: Vec<String>,
    pub recent: Vec<RecentProject>,
    pub favorites: Vec<PathBuf>,
    pub snippets: Vec<Snippet>,
    pub ignored_words: Vec<String>,
    pub session: Option<Session>,
    /// Compile by itself after a pause in typing.
    pub auto_compile: bool,
    /// Seconds of pause before that compilation.
    pub compile_delay: f32,
    /// Compile with the TeX installed on the system (pdfLaTeX, LuaLaTeX,
    /// biber…) instead of the one built into the app.
    pub system_tex: bool,
    /// Scroll the preview to the cursor's position as it moves.
    pub sync_follow: bool,
    /// Show the PDF with inverted, warm-tinted colours.
    pub pdf_dark: bool,
    /// Fit whole pages instead of the page width.
    pub pdf_fit_page: bool,
    pub folders: Folders,
    /// The first-run tour was shown (or skipped).
    pub tour_seen: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            revision: REVISION,
            language: Language::System,
            dark: true,
            accent: Accent::Terracotta,
            typewriter: false,
            spell_check: true,
            review: true,
            comfortable_width: true,
            font_size: 14.0,
            editor_font: EditorFont::Monospace,
            preview_dock: Dock::Right,
            sidebar_hidden: false,
            preview_hidden: false,
            sidebar_view: SidebarView::Tree,
            ref_sort: RefSort::File,
            image_sort: ImageSort::Name,
            palette_recent: vec![],
            recent: vec![],
            favorites: vec![],
            snippets: vec![],
            ignored_words: vec![],
            session: None,
            auto_compile: true,
            compile_delay: 1.0,
            system_tex: false,
            sync_follow: true,
            pdf_dark: false,
            pdf_fit_page: false,
            folders: Folders::default(),
            tour_seen: false,
        }
    }
}

pub const MAX_RECENT: usize = 8;

/// The settings' current revision.
const REVISION: u32 = 2;

impl Settings {
    /// Brings settings saved by an older version up to date, once.
    /// 1: spell checking became the default, and now checks the document's
    ///    own language, worth turning on for everyone, once.
    pub fn migrate(&mut self) {
        if self.revision < 1 {
            self.spell_check = true;
        }
        // 2: the first-run tour; whoever already has projects knows the window.
        if self.revision < 2 {
            self.tour_seen = !self.recent.is_empty();
        }
        self.revision = REVISION;
    }
}

impl Settings {
    pub fn push_recent(&mut self, path: PathBuf, name: String) {
        self.recent.retain(|r| r.path != path);
        self.recent.insert(0, RecentProject { path, name });
        self.recent.truncate(MAX_RECENT);
    }

    pub fn remove_recent(&mut self, path: &std::path::Path) {
        self.recent.retain(|r| r.path != path);
    }

    pub fn is_favorite(&self, path: &std::path::Path) -> bool {
        self.favorites.iter().any(|p| p == path)
    }

    pub fn toggle_favorite(&mut self, path: &std::path::Path) {
        if self.is_favorite(path) {
            self.favorites.retain(|p| p != path);
        } else {
            self.favorites.push(path.to_path_buf());
        }
    }

    /// Repoints favorites after a rename (a folder moves everything under it).
    pub fn rename_favorites(&mut self, old: &std::path::Path, new: &std::path::Path) {
        for f in &mut self.favorites {
            if let Ok(rest) = f.strip_prefix(old) {
                *f = if rest.as_os_str().is_empty() { new.to_path_buf() } else { new.join(rest) };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn older_settings_get_spell_checking_once() {
        // A file saved before revisions existed, with spelling off.
        let mut old: Settings = serde_json::from_str(r#"{"spell_check": false, "dark": false}"#).unwrap();
        assert_eq!(old.revision, 0);
        old.migrate();
        assert!(old.spell_check && !old.dark && old.revision == REVISION);
        // Turned off again afterwards: stays off.
        old.spell_check = false;
        let mut again: Settings = serde_json::from_str(&serde_json::to_string(&old).unwrap()).unwrap();
        again.migrate();
        assert!(!again.spell_check);
    }

    #[test]
    fn the_tour_is_only_for_newcomers() {
        assert!(!Settings::default().tour_seen);
        let mut veteran: Settings = serde_json::from_str(r#"{"revision": 1, "recent": [{"path": "/tmp/tese", "name": "tese"}]}"#).unwrap();
        veteran.migrate();
        assert!(veteran.tour_seen);
        let mut newcomer: Settings = serde_json::from_str(r#"{"revision": 1}"#).unwrap();
        newcomer.migrate();
        assert!(!newcomer.tour_seen);
    }

    #[test]
    fn folders_stay_inside_the_project() {
        assert_eq!(check_folder(" assets\\figuras/ ").unwrap(), "assets/figuras");
        assert_eq!(check_folder("a//b").unwrap(), "a/b");
        for bad in ["", "  /  ", "/abs", "C:\\x", "../fora", "a/../b", ".oculta", "a/.git", "x?y"] {
            assert!(check_folder(bad).is_err(), "{bad:?}");
        }
        let mut f = Folders::default();
        f.images = "../fora".into();
        assert_eq!(f.path(FolderKind::Images), PathBuf::from("images"), "invalid falls back to the default");
        f.images = "fig".into();
        assert_eq!(f.path(FolderKind::Images), PathBuf::from("fig"));
        let old: Settings = serde_json::from_str(r#"{"dark": false}"#).unwrap();
        assert_eq!(old.folders, Folders::default(), "missing in older files");
    }
}
