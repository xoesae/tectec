//! The app's look: fonts (fastframe's Inter with installed fonts for other
//! scripts), text rendered the way the desktop asks (fastframe-text), and the
//! warm light/dark palettes the old React UI used, applied to egui's visuals.
//! The icons are SVGs, listed in `icons.rs`.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Theme, Visuals};
use fastframe_fonts::{FontSetup, Monospace, Weight};

/// The app's own accent, a terracotta: what it starts with.
pub const TERRACOTTA: Color32 = Color32::from_rgb(0xD9, 0x77, 0x57);

/// The accent in use, as 0xRRGGBB: chosen in the preferences (`set_accent`).
static ACCENT_RGB: AtomicU32 = AtomicU32::new(0xD97757);

/// The accent: buttons that lead, the cursor, selections, what is active.
pub fn accent() -> Color32 {
    let rgb = ACCENT_RGB.load(Ordering::Relaxed);
    Color32::from_rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

/// Switches the accent, in both themes' visuals.
pub fn set_accent(ctx: &egui::Context, color: Color32) {
    let rgb = (color.r() as u32) << 16 | (color.g() as u32) << 8 | color.b() as u32;
    if ACCENT_RGB.swap(rgb, Ordering::Relaxed) == rgb {
        return;
    }
    for (theme, tones) in [(Theme::Dark, &DARK), (Theme::Light, &LIGHT)] {
        ctx.style_mut_of(theme, |style| apply_accent(&mut style.visuals, tones));
    }
}

/// What the accent colours: the selection is a wash of it over the window.
fn apply_accent(v: &mut Visuals, t: &Tones) {
    let accent = accent();
    let wash = if v.dark_mode { 0.32 } else { 0.3 };
    v.hyperlink_color = accent;
    v.selection.bg_fill = t.bg.lerp_to_gamma(accent, wash);
    v.selection.stroke = Stroke::new(1.0, accent);
    v.text_cursor.stroke = Stroke::new(2.0, accent);
    v.widgets.active.bg_stroke = Stroke::new(1.0, accent);
}
pub const SUCCESS: Color32 = Color32::from_rgb(0x5B, 0xB3, 0x76);
/// How long the interface's small transitions take, in seconds: panels
/// sliding, arrows turning, states fading in. Short enough to be felt
/// rather than watched.
pub const MOTION: f32 = 0.1;

struct Tones {
    bg: Color32,
    panel: Color32,
    raised: Color32,
    editor: Color32,
    text: Color32,
    weak: Color32,
    border: Color32,
    hover: Color32,
    active: Color32,
    /// The well of a checkbox, a radio button or a slider's rail: it has to
    /// stand out from the windows and panels it sits on.
    control: Color32,
    control_hover: Color32,
    /// A button at rest: a fill and an edge that set it apart from the
    /// window or panel behind it.
    button: Color32,
    button_edge: Color32,
}

const DARK: Tones = Tones {
    bg: Color32::from_rgb(0x1E, 0x1C, 0x1B),
    panel: Color32::from_rgb(0x23, 0x21, 0x1F),
    raised: Color32::from_rgb(0x2C, 0x29, 0x27),
    editor: Color32::from_rgb(0x1A, 0x19, 0x18),
    text: Color32::from_rgb(0xE6, 0xE1, 0xDA),
    weak: Color32::from_rgb(0x9A, 0x93, 0x8B),
    border: Color32::from_rgb(0x38, 0x34, 0x31),
    hover: Color32::from_rgb(0x36, 0x32, 0x2F),
    active: Color32::from_rgb(0x42, 0x3C, 0x38),
    control: Color32::from_rgb(0x4A, 0x44, 0x3F),
    control_hover: Color32::from_rgb(0x5A, 0x53, 0x4D),
    button: Color32::from_rgb(0x36, 0x33, 0x30),
    button_edge: Color32::from_rgb(0x4B, 0x46, 0x41),
};

const LIGHT: Tones = Tones {
    bg: Color32::from_rgb(0xFA, 0xF9, 0xF7),
    panel: Color32::from_rgb(0xF3, 0xF1, 0xED),
    raised: Color32::from_rgb(0xFF, 0xFF, 0xFF),
    editor: Color32::from_rgb(0xFF, 0xFE, 0xFC),
    text: Color32::from_rgb(0x26, 0x22, 0x1F),
    weak: Color32::from_rgb(0x74, 0x6E, 0x66),
    border: Color32::from_rgb(0xDD, 0xD8, 0xD0),
    hover: Color32::from_rgb(0xE9, 0xE5, 0xDE),
    active: Color32::from_rgb(0xDF, 0xD9, 0xD0),
    control: Color32::from_rgb(0xE2, 0xDD, 0xD5),
    control_hover: Color32::from_rgb(0xD3, 0xCC, 0xC2),
    button: Color32::from_rgb(0xF5, 0xF3, 0xEF),
    button_edge: Color32::from_rgb(0xCF, 0xC9, 0xC0),
};

/// Fonts plus both themes' visuals and spacing. Called once at startup;
/// switching theme afterwards is only `ctx.set_theme`.
pub fn install(ctx: &egui::Context) {
    // Inter for the interface, egui's Hack for the code, then egui's own fonts
    // and the installed ones.
    let mut fonts = FontSetup::default().weights(&[Weight::SemiBold]).monospace(Monospace::EguiDefault).definitions();
    add_symbol_fallbacks(&mut fonts);
    let rendering = fastframe_text::detect();
    rendering.apply_to(&mut fonts);
    ctx.set_fonts(fonts);

    for (theme, tones) in [(Theme::Dark, &DARK), (Theme::Light, &LIGHT)] {
        ctx.set_visuals_of(theme, visuals(theme == Theme::Dark, tones));
        ctx.style_mut_of(theme, |style| {
            style.spacing.item_spacing = egui::vec2(8.0, 5.0);
            style.spacing.button_padding = egui::vec2(8.0, 4.0);
            style.spacing.menu_margin = Margin::same(8);
            // Dialogs breathe: their content away from the border.
            style.spacing.window_margin = Margin::symmetric(20, 14);
            style.spacing.interact_size.y = 24.0;
            style.spacing.icon_width = 16.0;
            style.spacing.icon_width_inner = 9.0;
            style.spacing.slider_width = 180.0;
            // egui's scroll-edge fade dims the first and last lines of the
            // text being edited.
            style.spacing.scroll.fade.strength = 0.0;
            // Panels and collapsing sections: egui's 0.2 s reads as a wait.
            style.animation_time = MOTION;
            style.text_styles.insert(TextStyle::Body, FontId::new(13.5, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Button, FontId::new(13.5, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Small, FontId::new(11.0, FontFamily::Proportional));
            style.text_styles.insert(TextStyle::Heading, Weight::SemiBold.font_id(20.0));
            style.text_styles.insert(TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace));
        });
    }
    // After the visuals above, which would otherwise reset it.
    ctx.all_styles_mut(|style| rendering.apply_to_visuals(&mut style.visuals));
}

/// System fonts tried, in order, for the math operators neither Inter nor
/// fastframe's per-script fallbacks draw (∉, ∅, ⟹…, which LaTeX writers meet
/// in the autocomplete and type in Unicode sources). Loaded at runtime, last,
/// so nothing changes where none is installed.
const SYMBOL_FONTS: &[&str] = &[
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/TTF/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu/DejaVuSans.ttf",
    "/usr/share/fonts/dejavu-sans-fonts/DejaVuSans.ttf",
    "/usr/share/fonts/truetype/noto/NotoSansMath-Regular.ttf",
    "/usr/share/fonts/noto/NotoSansMath-Regular.ttf",
    "/System/Library/Fonts/Apple Symbols.ttf",
    "C:\\Windows\\Fonts\\seguisym.ttf",
];

fn add_symbol_fallbacks(fonts: &mut egui::FontDefinitions) {
    let Some(bytes) = SYMBOL_FONTS.iter().find_map(|p| std::fs::read(p).ok()) else { return };
    fonts.font_data.insert("math-symbols".into(), Arc::new(egui::FontData::from_owned(bytes)));
    // Every family, the SemiBold one included.
    for family in fonts.families.values_mut() {
        family.push("math-symbols".into());
    }
}

fn visuals(dark: bool, t: &Tones) -> Visuals {
    let mut v = if dark { Visuals::dark() } else { Visuals::light() };
    let radius = CornerRadius::same(4);
    v.panel_fill = t.panel;
    v.window_fill = t.raised;
    v.extreme_bg_color = t.editor;
    v.faint_bg_color = t.bg;
    v.code_bg_color = t.raised;
    v.window_stroke = Stroke::new(1.0, t.border);
    v.window_corner_radius = CornerRadius::same(8);
    v.menu_corner_radius = CornerRadius::same(6);
    v.window_shadow = Shadow { offset: [0, 6], blur: 18, spread: 0, color: Color32::from_black_alpha(if dark { 110 } else { 40 }) };
    v.popup_shadow = Shadow { offset: [0, 4], blur: 12, spread: 0, color: Color32::from_black_alpha(if dark { 100 } else { 35 }) };
    v.warn_fg_color = Color32::from_rgb(0xE0, 0xA8, 0x3C);
    v.error_fg_color = if dark { Color32::from_rgb(0xF0, 0x6A, 0x5A) } else { Color32::from_rgb(0xC0, 0x30, 0x20) };
    v.weak_text_color = Some(t.weak);
    // Text fields stand out from the panels they sit on.
    v.text_edit_bg_color = Some(if dark { Color32::from_rgb(0x17, 0x16, 0x15) } else { Color32::WHITE });

    let w = &mut v.widgets;
    w.noninteractive.bg_fill = t.panel;
    w.noninteractive.weak_bg_fill = t.panel;
    w.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    w.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    w.noninteractive.corner_radius = radius;
    for (wv, fill) in [(&mut w.inactive, t.raised), (&mut w.hovered, t.hover), (&mut w.active, t.active), (&mut w.open, t.hover)] {
        wv.bg_fill = fill;
        wv.weak_bg_fill = fill;
        wv.corner_radius = radius;
        wv.fg_stroke = Stroke::new(1.0, t.text);
        wv.bg_stroke = Stroke::NONE;
    }
    // Buttons (`weak_bg_fill`) are flat until hovered, but always keep the
    // room of their frame, so hovering one moves nothing around it. Dialogs
    // show the frame at rest (`framed`). Checkboxes, radio buttons and
    // slider rails (`bg_fill`) always show their well.
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.hovered.weak_bg_fill = t.control;
    w.active.weak_bg_fill = t.control_hover;
    w.inactive.bg_fill = t.control;
    w.hovered.bg_fill = t.control_hover;
    w.active.bg_fill = t.control_hover;
    w.hovered.bg_stroke = Stroke::new(1.0, t.border);
    // A slider reads as one: the filled part of the rail and a round knob.
    v.slider_trailing_fill = true;
    v.handle_shape = egui::style::HandleShape::Circle;
    v.interact_cursor = Some(egui::CursorIcon::PointingHand);
    apply_accent(&mut v, t);
    v
}

/// The pointing hand over whatever can be clicked. egui shows it only over
/// plain buttons; this covers checkboxes, radio buttons, sliders, combo
/// boxes, collapsing headers, tabs and rows made clickable by hand. Called
/// once a frame, after the interface is laid out. A widget that picked its
/// own cursor (the text I-beam) keeps it; large click areas (a PDF page)
/// are left alone.
pub fn pointing_hand(ctx: &egui::Context) {
    if ctx.output(|o| o.cursor_icon) != egui::CursorIcon::Default {
        return;
    }
    let hovered: Vec<egui::Id> = ctx.interaction_snapshot(|s| s.hovered.iter().copied().collect());
    let clickable = hovered.into_iter().filter_map(|id| ctx.read_response(id)).any(|r| {
        let small = r.interact_rect.height() < 80.0 || r.interact_rect.width() < 80.0;
        r.enabled() && r.sense.senses_click() && (small || !r.sense.senses_drag())
    });
    if clickable {
        ctx.set_cursor_icon(egui::CursorIcon::PointingHand);
    }
}

/// Buttons that look like buttons at rest, with more room inside: for
/// dialogs, where a flat "Cancel" reads as text. Applies to `ui` and what is
/// added to it.
pub fn framed(ui: &mut egui::Ui) {
    let tones = if ui.visuals().dark_mode { &DARK } else { &LIGHT };
    let w = &mut ui.visuals_mut().widgets;
    w.inactive.weak_bg_fill = tones.button;
    w.inactive.bg_stroke = Stroke::new(1.0, tones.button_edge);
    ui.spacing_mut().button_padding = egui::vec2(14.0, 6.0);
}
