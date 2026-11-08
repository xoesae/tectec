//! The tray item (fastframe-tray): brings the window forward, compiles, or
//! quits. Nothing keeps running without the window yet, so it never hides it.

use crate::i18n::t;
use fastframe_tray::{Config, MenuItem, Tray};

/// The icon, drawn from `assets/app-icon.svg` at whatever size is asked.
const APP_ICON: &[u8] = include_bytes!("../assets/app-icon.svg");

/// `None` where the desktop has no tray (GNOME without the AppIndicator
/// extension, say).
pub fn spawn(ctx: &egui::Context) -> Option<Tray> {
    let ctx = ctx.clone();
    let config = Config { id: "tectec", title: "TecTec".into(), icon: app_icon_rgba, template_icon: None, menu: menu() };
    let mut tray = Tray::spawn(config, move || ctx.request_repaint())?;
    tray.attach();
    Some(tray)
}

fn menu() -> Vec<MenuItem> {
    vec![
        MenuItem::action("show", t("Show TecTec")),
        MenuItem::Separator,
        MenuItem::action("compile", t("Save and compile")),
        MenuItem::Separator,
        MenuItem::action("quit", t("Quit")),
    ]
}

/// After a change of language.
pub fn relabel(tray: &mut Tray) {
    for item in menu() {
        if let MenuItem::Action { id, label, .. } = item {
            tray.set_label(id, label);
        }
    }
}

/// The app icon as straight (not premultiplied) RGBA, `size` pixels square.
pub fn app_icon_rgba(size: usize) -> Vec<u8> {
    let blank = vec![0; size * size * 4];
    let Ok(tree) = resvg::usvg::Tree::from_data(APP_ICON, &resvg::usvg::Options::default()) else { return blank };
    let Some(mut pixmap) = resvg::tiny_skia::Pixmap::new(size as u32, size as u32) else { return blank };
    let scale = size as f32 / tree.size().width();
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
    pixmap.pixels().iter().flat_map(|p| {
        let c = p.demultiply();
        [c.red(), c.green(), c.blue(), c.alpha()]
    }).collect()
}

/// The same icon for the window.
pub fn window_icon() -> egui::IconData {
    const SIZE: usize = 128;
    egui::IconData { rgba: app_icon_rgba(SIZE), width: SIZE as u32, height: SIZE as u32 }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The keycap's T comes from a `<use>`: it must render, not vanish.
    #[test]
    fn app_icon_renders() {
        const SIZE: usize = 256;
        let rgba = app_icon_rgba(SIZE);
        let at = |x: usize, y: usize| &rgba[(y * SIZE + x) * 4..][..4];
        assert_eq!(at(2, 2)[3], 0, "corner outside the tile is transparent");
        let stem = at(128, 110);
        assert!(stem[0] > 180 && stem[1] < 120 && stem[2] < 90, "the T is terracotta: {stem:?}");
        let face = at(80, 165);
        assert!(face.iter().take(3).all(|c| *c > 220), "the keycap face is cream: {face:?}");
    }
}
