//! Go to definition and the hover card: resolving what `editor::refs` found
//! under the pointer or cursor against the project index.

use super::{App, StatusKind};
use crate::backend::project::{self, Kind};
use crate::editor::latex_db;
use crate::editor::refs::RefTarget;
use crate::i18n::{t, tn};
use crate::tf;
use std::path::{Path, PathBuf};

/// How long the pointer rests before the card shows.
const HOVER_DELAY: f64 = 0.45;

impl App {
    /// Where relative paths in the source resolve: the main file's folder
    /// (LaTeX runs there), else the project root.
    fn source_base(&self) -> Option<PathBuf> {
        self.main_file.as_ref().and_then(|m| m.parent().map(Path::to_path_buf)).or_else(|| self.root.clone())
    }

    /// The first existing path among `name` and `name` plus each extension.
    fn existing(&self, name: &str, extensions: &[&str]) -> Option<PathBuf> {
        let base = self.source_base()?;
        let plain = base.join(name);
        if plain.is_file() {
            return Some(plain);
        }
        extensions.iter().map(|e| base.join(format!("{name}.{e}"))).find(|p| p.is_file())
    }

    pub fn go_to_definition(&mut self, target: RefTarget) {
        match &target {
            RefTarget::Label(name) => match self.index.labels.iter().find(|l| &l.name == name) {
                Some(l) => {
                    let (file, line) = (l.file.clone(), l.line);
                    self.go_to(&file, line, true);
                }
                None => self.set_status(tf!("Label \"{name}\" not found in the project.", name), StatusKind::Error),
            },
            RefTarget::Cite(key) => match self.index.citations.iter().find(|c| &c.key == key) {
                Some(c) => {
                    let file = c.file.clone();
                    let line = bib_line(&file, key).unwrap_or(1);
                    self.go_to(&file, line, true);
                }
                None => self.set_status(tf!("Reference \"{key}\" is in no .bib of the project.", key), StatusKind::Error),
            },
            RefTarget::TexFile(name) => match self.existing(name, &["tex"]) {
                Some(p) => {
                    self.open_file(&p, Kind::Tex);
                }
                None => self.set_status(tf!("File \"{name}\" not found.", name), StatusKind::Error),
            },
            RefTarget::BibFile(name) => {
                for part in name.split(',') {
                    if let Some(p) = self.existing(part.trim(), &["bib"]) {
                        self.open_file(&p, Kind::Tex);
                        return;
                    }
                }
                self.set_status(tf!("File \"{name}\" not found.", name), StatusKind::Error);
            }
            RefTarget::Graphic(name) => match self.existing(name, &["pdf", "png", "jpg", "jpeg", "eps", "svg"]) {
                Some(p) => {
                    let kind = project::kind_of(&project::file_name(&p), false);
                    self.open_file(&p, kind);
                }
                None => self.set_status(tf!("Image \"{name}\" not found.", name), StatusKind::Error),
            },
            RefTarget::Command(name) => match self.index.macros.iter().find(|m| !m.environment && &m.name == name) {
                Some(m) => {
                    let (file, line) = (m.file.clone(), m.line);
                    self.go_to(&file, line, true);
                }
                None => self.set_status(tf!("{name} is not defined in the project.", name), StatusKind::Normal),
            },
        }
    }

    /// The text of the hover card for a target, if there is anything to say.
    fn hover_text(&self, target: &RefTarget) -> Option<String> {
        let rel = |p: &Path| project::relative(p, self.root.as_deref());
        match target {
            RefTarget::Label(name) => Some(match self.index.labels.iter().find(|l| &l.name == name) {
                Some(l) if !l.context.is_empty() => format!("{}\n{}:{} · {}", l.context, rel(&l.file), l.line, t("Ctrl+click to go")),
                Some(l) => format!("{}:{} · {}", rel(&l.file), l.line, t("Ctrl+click to go")),
                None => tf!("Label \"{name}\" not found: the PDF will show \"??\".", name),
            }),
            RefTarget::Cite(key) => Some(match self.index.citations.iter().find(|c| &c.key == key) {
                Some(c) => {
                    let who = [c.author.as_str(), c.year.as_str()].iter().filter(|s| !s.is_empty()).cloned().collect::<Vec<_>>().join(", ");
                    let title = if c.title.is_empty() { String::new() } else { format!("\n{}", c.title) };
                    format!("{who}{title}\n{} · {}", rel(&c.file), t("Ctrl+click to go"))
                }
                None => tf!("\"{key}\" is in no .bib of the project.", key),
            }),
            RefTarget::Command(name) => {
                if let Some(m) = self.index.macros.iter().find(|m| !m.environment && &m.name == name) {
                    let args = if m.args > 0 { format!(" · {}", tn("{n} argument", "{n} arguments", m.args as usize)) } else { String::new() };
                    return Some(format!("{}{args}\n{}:{} · {}", t("Defined in the project"), rel(&m.file), m.line, t("Ctrl+click to go")));
                }
                let (_, desc, package) = latex_db::COMMANDS.iter().find(|(t, _, _)| {
                    t.strip_prefix(name.as_str()).map(|rest| rest.is_empty() || !rest.starts_with(|c: char| c.is_ascii_alphabetic())).unwrap_or(false)
                })?;
                let desc = t(desc);
                Some(if package.is_empty() { desc.to_string() } else { tf!("{desc} (package {package})", desc, package) })
            }
            RefTarget::TexFile(_) | RefTarget::Graphic(_) | RefTarget::BibFile(_) => Some(t("Ctrl+click to open").into()),
        }
    }

    /// Shows the hover card once the pointer has rested on the same target.
    pub fn hover_card(&mut self, ctx: &egui::Context, hover: Option<(RefTarget, egui::Rect)>) {
        let Some((target, rect)) = hover else {
            self.ref_hover = None;
            return;
        };
        let now = ctx.input(|i| i.time);
        let since = match &self.ref_hover {
            Some((t, at)) if *t == target => *at,
            _ => {
                self.ref_hover = Some((target.clone(), now));
                now
            }
        };
        if now - since < HOVER_DELAY {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64(HOVER_DELAY - (now - since) + 0.01));
            return;
        }
        let Some(text) = self.hover_text(&target) else { return };
        egui::Area::new(egui::Id::new("ref-hover"))
            .order(egui::Order::Tooltip)
            .fixed_pos(rect.left_bottom() + egui::vec2(0.0, 4.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).show(ui, |ui| {
                    ui.set_max_width(380.0);
                    let mut lines = text.lines();
                    if let Some(first) = lines.next() {
                        ui.label(first);
                    }
                    for l in lines {
                        ui.label(egui::RichText::new(l).small().weak());
                    }
                });
            });
    }
}

/// The line of `@type{key,` in a .bib file.
fn bib_line(file: &Path, key: &str) -> Option<usize> {
    let text = std::fs::read_to_string(file).ok()?;
    let re = regex::Regex::new(&format!(r"@\w+\s*\{{\s*{}\s*,", regex::escape(key))).ok()?;
    let m = re.find(&text)?;
    Some(text[..m.start()].matches('\n').count() + 1)
}

// ---------- Back and forward ----------

/// How many places back (and forward) are kept.
const HISTORY_LEN: usize = 100;
/// A jump inside the same file farther than this many lines is a place of
/// its own in the history.
const FAR_JUMP: usize = 15;

/// A place the user was: a file, and the cursor's line in it (0 when the
/// tab has no cursor, as a PDF).
#[derive(Clone, Debug, PartialEq)]
pub struct Spot {
    path: PathBuf,
    kind: Kind,
    line: usize,
}

/// The back and forward stacks, as in a browser.
#[derive(Default)]
pub struct History {
    back: Vec<Spot>,
    forward: Vec<Spot>,
    /// Going back or forward: the moves it makes are not new places.
    replaying: bool,
}

impl History {
    pub fn can_back(&self) -> bool {
        !self.back.is_empty()
    }

    pub fn can_forward(&self) -> bool {
        !self.forward.is_empty()
    }

    pub fn clear(&mut self) {
        self.back.clear();
        self.forward.clear();
    }

    /// The user left `spot` for somewhere new.
    fn leave(&mut self, spot: Spot) {
        if self.replaying {
            return;
        }
        self.forward.clear();
        if self.back.last() != Some(&spot) {
            self.back.push(spot);
        }
        if self.back.len() > HISTORY_LEN {
            self.back.remove(0);
        }
    }
}

impl App {
    /// Where the user is now.
    fn spot(&self) -> Option<Spot> {
        let t = self.active_tab()?;
        let line = if t.editable() { t.editor.cursor_line() } else { 0 };
        Some(Spot { path: t.path.clone(), kind: t.kind, line })
    }

    /// Called before the active tab changes to `to`: the current file
    /// becomes a place to come back to.
    pub(super) fn nav_leaving_for(&mut self, to: &Path) {
        if let Some(here) = self.spot().filter(|s| s.path != to) {
            self.nav.leave(here);
        }
    }

    /// Called before the cursor jumps to `line` of the active file: a far
    /// jump (go to definition, an outline entry) is a place of its own.
    pub(super) fn nav_jumping_to(&mut self, file: &Path, line: usize) {
        if let Some(here) = self.spot().filter(|s| s.path == file && s.line.abs_diff(line) > FAR_JUMP) {
            self.nav.leave(here);
        }
    }

    /// Back (or forward) to the previous (next) place, skipping files that
    /// are gone.
    pub fn nav_step(&mut self, back: bool) {
        loop {
            let next = if back { self.nav.back.pop() } else { self.nav.forward.pop() };
            let Some(spot) = next else {
                self.set_status(if back { t("Nothing to go back to.") } else { t("Nothing to go forward to.") }, StatusKind::Normal);
                return;
            };
            if !spot.path.exists() {
                continue;
            }
            let here = self.spot();
            self.nav.replaying = true;
            let opened = self.open_file(&spot.path, spot.kind);
            if opened && spot.line > 0 {
                if let Some(t) = self.active_tab_mut().filter(|t| t.editable()) {
                    t.editor.go_to_line(&t.content, spot.line, None);
                }
            }
            self.nav.replaying = false;
            if !opened {
                continue;
            }
            if let Some(here) = here.filter(|h| *h != spot) {
                if back { self.nav.forward.push(here) } else { self.nav.back.push(here) }
            }
            return;
        }
    }
}

// ---------- The two-finger swipe ----------

/// How far two fingers travel sideways before the swipe counts, in points.
const SWIPE_DISTANCE: f32 = 140.0;
/// Sideways must beat up-and-down by this factor: a scroll that drifts a
/// little is still a scroll.
const SWIPE_DOMINANCE: f32 = 2.5;
/// Systems that report no Start/End (X11) end a gesture with a pause.
const SWIPE_PAUSE: f64 = 0.3;
/// Roughly a wheel line, for systems that count the touchpad in lines.
const LINE_POINTS: f32 = 40.0;

/// Detects a sideways two-finger swipe on the touchpad, once per gesture,
/// from the raw wheel events, before egui scrolls with them.
#[derive(Default)]
pub struct Swipe {
    travel: egui::Vec2,
    last: f64,
    fired: bool,
    /// A swipe waiting for the frame to run it: true for back.
    pub pending: Option<bool>,
}

impl Swipe {
    /// Fingers moving right mean back, left mean forward. `blocked` while
    /// the pointer is over something that scrolls sideways (a zoomed PDF):
    /// there the swipe scrolls. Once fired, the rest of the gesture's
    /// sideways movement is dropped, so nothing scrolls or glides with it.
    pub fn feed(&mut self, input: &mut egui::RawInput, now: f64, blocked: bool) {
        for event in &mut input.events {
            let egui::Event::MouseWheel { unit, delta, phase, modifiers } = event else { continue };
            if now - self.last > SWIPE_PAUSE {
                self.reset();
            }
            self.last = now;
            let fired = self.fired;
            if fired {
                delta.x = 0.0;
            }
            match phase {
                egui::TouchPhase::Start | egui::TouchPhase::Cancel | egui::TouchPhase::End => {
                    self.reset();
                    continue;
                }
                egui::TouchPhase::Move => {}
            }
            // Ctrl/Shift + wheel zoom or scroll on purpose.
            if fired || blocked || modifiers.any() {
                continue;
            }
            let scale = match unit {
                egui::MouseWheelUnit::Point => 1.0,
                egui::MouseWheelUnit::Line => LINE_POINTS,
                egui::MouseWheelUnit::Page => continue,
            };
            self.travel += *delta * scale;
            let (x, y) = (self.travel.x.abs(), self.travel.y.abs());
            if x >= SWIPE_DISTANCE && x >= SWIPE_DOMINANCE * y {
                self.fired = true;
                self.pending = Some(self.travel.x > 0.0);
                delta.x = 0.0;
            }
        }
    }

    fn reset(&mut self) {
        self.travel = egui::Vec2::ZERO;
        self.fired = false;
    }
}

fn sideways_id() -> egui::Id {
    egui::Id::new("scrolls-sideways")
}

/// Called by a scroll area that can scroll sideways while the pointer is
/// over it, so a sideways swipe there scrolls instead of going back.
pub fn claim_sideways(ctx: &egui::Context, output_rect: egui::Rect, content_width: f32) {
    let hovered = ctx.pointer_hover_pos().is_some_and(|p| output_rect.contains(p));
    if hovered && content_width > output_rect.width() + 1.0 {
        let frame = ctx.cumulative_frame_nr();
        ctx.data_mut(|d| d.insert_temp(sideways_id(), frame));
    }
}

/// Whether the pointer was over something that scrolls sideways last frame.
pub fn sideways_claimed(ctx: &egui::Context) -> bool {
    let now = ctx.cumulative_frame_nr();
    ctx.data(|d| d.get_temp::<u64>(sideways_id())).is_some_and(|f| f + 2 >= now)
}
