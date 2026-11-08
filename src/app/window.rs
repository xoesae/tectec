//! The window's own frame: the app draws no system title bar, so the menu
//! bar carries the minimize, maximize and close buttons, moves the window
//! when dragged and maximizes it on a double click, and the window's edges
//! resize it.

use crate::i18n::t;
use crate::icons::{self, Icon};
use egui::{CursorIcon, ResizeDirection, Sense, ViewportCommand};

/// How far from the window's edge a press resizes it, in points.
const EDGE: f32 = 6.0;

fn maximized(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().maximized.unwrap_or(false) || i.viewport().fullscreen.unwrap_or(false))
}

fn fullscreen(ctx: &egui::Context) -> bool {
    ctx.input(|i| i.viewport().fullscreen.unwrap_or(false))
}

/// Maximize, or restore. Out of full screen (which hides the system's own
/// bar), restoring goes to a maximized window, which leaves it in view.
fn toggle_maximized(ctx: &egui::Context) {
    if fullscreen(ctx) {
        ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
        ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
    } else {
        ctx.send_viewport_cmd(ViewportCommand::Maximized(!maximized(ctx)));
    }
}

/// eframe brings the window back as it was closed, in full screen too when
/// that was zen mode. Outside zen mode, it opens maximized instead. Returns
/// whether that is settled (the window's state may not be known yet).
pub fn leave_saved_fullscreen(ctx: &egui::Context, zen: bool) -> bool {
    match ctx.input(|i| i.viewport().fullscreen) {
        None => false,
        Some(true) if !zen => {
            ctx.send_viewport_cmd(ViewportCommand::Fullscreen(false));
            ctx.send_viewport_cmd(ViewportCommand::Maximized(true));
            true
        }
        Some(_) => true,
    }
}

/// A window neither maximized nor in full screen that still covers the whole
/// monitor (its size saved from full screen) gets a smaller one, so that
/// restoring it shows a change and the system's bar.
pub fn keep_restorable(ctx: &egui::Context) {
    let (maximized, fullscreen, monitor) = ctx.input(|i| (i.viewport().maximized, i.viewport().fullscreen, i.viewport().monitor_size));
    if maximized != Some(false) || fullscreen != Some(false) {
        return;
    }
    let Some(monitor) = monitor else { return };
    let size = ctx.content_rect().size();
    if size.x >= monitor.x - 1.0 && size.y >= monitor.y - 64.0 {
        ctx.send_viewport_cmd(ViewportCommand::InnerSize((monitor * 0.75).round()));
    }
}

/// Minimize, maximize (or restore) and close, right to left: call it in a
/// right-to-left layout.
pub fn controls(ui: &mut egui::Ui) {
    let ctx = ui.ctx().clone();
    ui.spacing_mut().item_spacing.x = 0.0;
    // Wide targets, as the system's own buttons are.
    let size = egui::vec2(40.0, ui.available_height().max(26.0));
    let close = ui.add(icons::icon_button(Icon::X).frame(false).min_size(size)).on_hover_text(t("Close"));
    if close.hovered() {
        // The usual red of a close button, behind the icon.
        ui.painter().rect_filled(close.rect, 0.0, egui::Color32::from_rgb(0xC4, 0x2B, 0x1C).gamma_multiply(0.85));
        icons::show_at(ui, Icon::X, close.rect, egui::Color32::WHITE);
    }
    if close.clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Close);
    }
    let max = maximized(&ctx);
    let (icon, tip) = if max { (Icon::Copy, t("Restore")) } else { (Icon::Square, t("Maximize")) };
    if ui.add(icons::icon_button(icon).min_size(size)).on_hover_text(tip).clicked() {
        toggle_maximized(&ctx);
    }
    if ui.add(icons::icon_button(Icon::Minus).min_size(size)).on_hover_text(t("Minimize")).clicked() {
        ctx.send_viewport_cmd(ViewportCommand::Minimized(true));
    }
}

/// The empty part of the menu bar: dragging it moves the window, a double
/// click maximizes or restores it.
pub fn drag_area(ui: &mut egui::Ui) {
    let rect = ui.available_rect_before_wrap();
    if rect.width() <= 0.0 {
        return;
    }
    let response = ui.interact(rect, ui.id().with("window-drag"), Sense::click_and_drag());
    let ctx = ui.ctx();
    if response.double_clicked() {
        toggle_maximized(ctx);
    } else if response.drag_started() {
        ctx.send_viewport_cmd(ViewportCommand::StartDrag);
    }
}

/// Resizing from the window's edges and corners. Called once a frame, after
/// the widgets.
pub fn edges(ctx: &egui::Context) {
    if maximized(ctx) {
        return;
    }
    let screen = ctx.content_rect();
    let Some(pos) = ctx.input(|i| i.pointer.hover_pos()) else { return };
    let (left, right) = (pos.x - screen.left() < EDGE, screen.right() - pos.x < EDGE);
    let (top, bottom) = (pos.y - screen.top() < EDGE, screen.bottom() - pos.y < EDGE);
    let direction = match (left, right, top, bottom) {
        (true, _, true, _) => ResizeDirection::NorthWest,
        (_, true, true, _) => ResizeDirection::NorthEast,
        (true, _, _, true) => ResizeDirection::SouthWest,
        (_, true, _, true) => ResizeDirection::SouthEast,
        (true, ..) => ResizeDirection::West,
        (_, true, ..) => ResizeDirection::East,
        (_, _, true, _) => ResizeDirection::North,
        (.., true) => ResizeDirection::South,
        _ => return,
    };
    ctx.set_cursor_icon(match direction {
        ResizeDirection::North => CursorIcon::ResizeNorth,
        ResizeDirection::South => CursorIcon::ResizeSouth,
        ResizeDirection::East => CursorIcon::ResizeEast,
        ResizeDirection::West => CursorIcon::ResizeWest,
        ResizeDirection::NorthEast => CursorIcon::ResizeNorthEast,
        ResizeDirection::NorthWest => CursorIcon::ResizeNorthWest,
        ResizeDirection::SouthEast => CursorIcon::ResizeSouthEast,
        ResizeDirection::SouthWest => CursorIcon::ResizeSouthWest,
    });
    if ctx.input(|i| i.pointer.primary_pressed()) {
        ctx.send_viewport_cmd(ViewportCommand::BeginResize(direction));
    }
}

/// A thin line around the window, which has no system frame to set it
/// apart from what is behind it.
pub fn border(ctx: &egui::Context) {
    if maximized(ctx) {
        return;
    }
    let stroke = ctx.global_style().visuals.widgets.noninteractive.bg_stroke;
    let painter = ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("window-border")));
    painter.rect_stroke(ctx.content_rect(), 0.0, stroke, egui::StrokeKind::Inside);
}
