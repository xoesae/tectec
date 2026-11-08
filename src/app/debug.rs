//! Debug-build-only hooks used to check the UI without a human at the
//! keyboard: `TECTEC_OPEN` opens files (`;`-separated) once the project
//! is up, and `TECTEC_SCREENSHOT=out.png` saves a screenshot after
//! `TECTEC_SCREENSHOT_AFTER` seconds (default 5) and closes the app.
//! `TECTEC_TYPE` types a script into the focused widget, one token per
//! frame from `TECTEC_TYPE_AFTER` seconds (default 3): plain text, or
//! keys in angle brackets: `<Enter>`, `<Tab>`, `<S-Tab>`, `<BS>`, `<Esc>`,
//! `<Up>`, `<Down>`, `<Left>`, `<Right>`, `<End>`, `<Home>`, `<C-Space>`,
//! `<C-/>`, `<C-.>`, `<F7>`, `<A-Up>`, `<A-Down>`, `<C-S-D>`, `<C-Z>`, `<F12>`, `<wait>`, and
//! `<Mx,y>` to move the pointer, `<Dx,y>`/`<Ux,y>` to press/release it, `<Rx,y>`
//! to right-click, and
//! `<WS>`, `<W-40>`, `<WE>` for a touchpad scroll gesture (`<WX40>` moves
//! sideways: positive is fingers to the right).
//! `TECTEC_DROP=path` hovers that file over the window once typing is
//! done, then drops it.

use super::App;
use crate::backend::project;

#[derive(Default)]
pub struct DebugHooks {
    opened: bool,
    requested: bool,
    script: Option<Vec<String>>,
    drop_frames: u32,
    /// Frame times (ms) while the typing script runs, for TECTEC_FRAME_STATS.
    frame_ms: Vec<f64>,
    frame_start: Option<std::time::Instant>,
    typing: bool,
    reported: bool,
}

/// A file "dropped" by the debug hooks.
#[derive(Debug)]
struct DebugDrop(std::path::PathBuf);

impl egui::DroppedFile for DebugDrop {
    fn path(&self) -> &std::path::Path {
        &self.0
    }
    fn bytes(&self) -> Result<Vec<u8>, String> {
        std::fs::read(&self.0).map_err(|e| e.to_string())
    }
}

/// Splits a typing script into tokens: `<Key>` or a single character.
fn tokens(script: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = script.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '<' {
            let rest: String = chars.clone().take_while(|c| *c != '>').collect();
            if !rest.is_empty() && rest.len() < 12 && !rest.contains(' ') {
                for _ in 0..=rest.chars().count() {
                    chars.next();
                }
                out.push(format!("<{rest}>"));
                continue;
            }
        }
        out.push(c.to_string());
    }
    out
}

fn key_event(token: &str) -> Vec<egui::Event> {
    use egui::{Key, Modifiers};
    let inner = &token[1..token.len() - 1];
    // A touchpad gesture: <WS> fingers down, <W-40> move 40 points (up is
    // negative), <WE> fingers lifted.
    let wheel = |delta: egui::Vec2, phase| vec![egui::Event::MouseWheel { unit: egui::MouseWheelUnit::Point, delta, phase, modifiers: Default::default() }];
    match inner {
        "WS" => return wheel(egui::Vec2::ZERO, egui::TouchPhase::Start),
        "WE" => return wheel(egui::Vec2::ZERO, egui::TouchPhase::End),
        _ => {
            if let Some(dx) = inner.strip_prefix("WX").and_then(|d| d.parse::<f32>().ok()) {
                return wheel(egui::vec2(dx, 0.0), egui::TouchPhase::Move);
            }
            if let Some(dy) = inner.strip_prefix('W').and_then(|d| d.parse::<f32>().ok()) {
                return wheel(egui::vec2(0.0, dy), egui::TouchPhase::Move);
            }
        }
    }
    // <Mx,y> moves the pointer; <Dx,y>/<Ux,y> press/release the primary button there;
    // <Rx,y> clicks the secondary one (a context menu).
    if let Some((x, y)) = inner.strip_prefix('R').and_then(|r| r.split_once(',')) {
        if let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) {
            let pos = egui::pos2(x, y);
            let button = |pressed| egui::Event::PointerButton { pos, button: egui::PointerButton::Secondary, pressed, modifiers: Default::default() };
            return vec![egui::Event::PointerMoved(pos), button(true), button(false)];
        }
    }
    for (tag, press) in [('M', None), ('D', Some(true)), ('U', Some(false))] {
        if let Some((x, y)) = inner.strip_prefix(tag).and_then(|r| r.split_once(',')) {
            if let (Ok(x), Ok(y)) = (x.parse::<f32>(), y.parse::<f32>()) {
                let pos = egui::pos2(x, y);
                let mut ev = vec![egui::Event::PointerMoved(pos)];
                if let Some(pressed) = press {
                    ev.push(egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
                }
                return ev;
            }
        }
    }
    let (mods, name) = match inner.rsplit_once('-').filter(|(m, _)| !m.is_empty()) {
        Some((m, n)) => {
            let mut mods = Modifiers::NONE;
            for p in m.split('-') {
                match p {
                    "C" => mods |= Modifiers::COMMAND | Modifiers::CTRL,
                    "S" => mods |= Modifiers::SHIFT,
                    "A" => mods |= Modifiers::ALT,
                    _ => {}
                }
            }
            (mods, n)
        }
        None => (Modifiers::NONE, inner),
    };
    let key = match name {
        "Enter" => Key::Enter,
        "Tab" => Key::Tab,
        "BS" => Key::Backspace,
        "Esc" => Key::Escape,
        "Up" => Key::ArrowUp,
        "Down" => Key::ArrowDown,
        "Left" => Key::ArrowLeft,
        "Right" => Key::ArrowRight,
        "End" => Key::End,
        "Home" => Key::Home,
        "Space" => Key::Space,
        "/" => Key::Slash,
        "." => Key::Period,
        "," => Key::Comma,
        other => match Key::from_name(other) {
            Some(k) => k,
            None => return vec![],
        },
    };
    let ev = |pressed| egui::Event::Key { key, physical_key: None, pressed, repeat: false, modifiers: mods };
    vec![ev(true), ev(false)]
}

impl App {
    /// Feeds the next token of `TECTEC_TYPE` into the raw input of the
    /// coming frame (so pointer moves update hover state like real ones).
    pub fn debug_inject(&mut self, ctx: &egui::Context, raw: &mut egui::RawInput) {
        if self.debug.script.is_none() {
            let Ok(script) = std::env::var("TECTEC_TYPE") else { return };
            self.debug.script = Some(tokens(&script));
        }
        // Under Xvfb with no window manager the window never gets OS focus,
        // and egui only reports widget focus in a focused window.
        raw.focused = true;
        let after: f64 = std::env::var("TECTEC_TYPE_AFTER").ok().and_then(|s| s.parse().ok()).unwrap_or(3.0);
        if ctx.input(|i| i.time) < after {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        let Some(script) = self.debug.script.as_mut() else { return };
        if script.is_empty() {
            // Then the file drop, if any: a few frames hovering, then drop.
            if let Ok(p) = std::env::var("TECTEC_DROP") {
                self.debug.drop_frames += 1;
                let path = std::path::PathBuf::from(p);
                if self.debug.drop_frames < 20 {
                    raw.hovered_files.push(egui::HoveredFile { path: Some(path), mime: String::new() });
                    ctx.request_repaint_after(std::time::Duration::from_millis(30));
                } else if self.debug.drop_frames == 20 {
                    raw.dropped_files.push(std::sync::Arc::new(DebugDrop(path)));
                }
            }
            return;
        }
        let token = script.remove(0);
        let events = if token.len() > 2 && token.starts_with('<') && token.ends_with('>') {
            key_event(&token)
        } else {
            vec![egui::Event::Text(token)]
        };
        // The modifier state lasts for this token's frame only.
        let mods = events
            .iter()
            .find_map(|e| match e {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or_default();
        raw.events.push(egui::Event::ModifiersChanged(mods));
        raw.events.extend(events);
        ctx.request_repaint_after(std::time::Duration::from_millis(30));
    }

    /// With TECTEC_FRAME_STATS set, times every frame while the typing
    /// script runs and prints a summary when it ends.
    pub fn debug_frame_begin(&mut self) {
        self.debug.frame_start = Some(std::time::Instant::now());
    }

    pub fn debug_frame_end(&mut self) {
        if std::env::var("TECTEC_FRAME_STATS").is_err() {
            return;
        }
        let Some(start) = self.debug.frame_start.take() else { return };
        let typing = self.debug.script.as_ref().map(|s| !s.is_empty()).unwrap_or(false);
        if typing {
            self.debug.typing = true;
            let ms = start.elapsed().as_secs_f64() * 1000.0;
            if ms > 40.0 {
                let left = self.debug.script.as_ref().map(|s| s.len()).unwrap_or(0);
                eprintln!("frame lento {} ({left} tokens restantes): {ms:.1}ms", self.debug.frame_ms.len());
            }
            self.debug.frame_ms.push(ms);
        } else if self.debug.typing && !self.debug.reported {
            self.debug.reported = true;
            let mut v = self.debug.frame_ms.clone();
            v.sort_by(f64::total_cmp);
            let pct = |p: f64| v[((v.len() as f64 - 1.0) * p) as usize];
            eprintln!(
                "FRAMES n={} médio={:.1}ms p50={:.1}ms p95={:.1}ms máx={:.1}ms",
                v.len(),
                v.iter().sum::<f64>() / v.len() as f64,
                pct(0.5),
                pct(0.95),
                v[v.len() - 1]
            );
        }
    }

    pub fn debug_hooks(&mut self, ctx: &egui::Context) {
        if !self.debug.opened && self.root.is_some() {
            self.debug.opened = true;
            if let Ok(list) = std::env::var("TECTEC_OPEN") {
                for p in list.split(';').filter(|p| !p.is_empty()) {
                    let path = std::path::PathBuf::from(p);
                    let kind = project::kind_of(&project::file_name(&path), false);
                    self.open_file(&path, kind);
                }
            }
            if let Ok(line) = std::env::var("TECTEC_LINE") {
                if let (Ok(line), Some(i)) = (line.parse::<usize>(), self.active) {
                    let t = &mut self.tabs[i];
                    t.editor.go_to_line(&t.content, line, None);
                }
            }
        }
        if std::env::var("TECTEC_TRACE_FOCUS").is_ok() {
            let f = ctx.memory(|m| m.focused());
            let ed = self.active.map(|i| self.tabs[i].editor.id);
            eprintln!("t={:.2} focus={:?} editor={:?} popup={}", ctx.input(|i| i.time), f, ed, self.active.map(|i| self.tabs[i].editor.popup_open()).unwrap_or(false));
        }
        let Ok(out) = std::env::var("TECTEC_SCREENSHOT") else { return };
        let after: f64 = std::env::var("TECTEC_SCREENSHOT_AFTER").ok().and_then(|s| s.parse().ok()).unwrap_or(5.0);
        let now = ctx.input(|i| i.time);
        if !self.debug.requested && now >= after {
            self.debug.requested = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
        let shot = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(img) = shot {
            let [w, h] = img.size;
            let bytes: Vec<u8> = img.pixels.iter().flat_map(|c| c.to_array()).collect();
            if let Some(buf) = image::RgbaImage::from_raw(w as u32, h as u32, bytes) {
                let _ = buf.save(&out);
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
}
