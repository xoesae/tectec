//! Checkboxes, radio buttons and sliders that read at a glance: egui's
//! draw the box and the rail in about the colour of the window behind them.
//! Here an empty box has an edge, a checked one is filled with the accent,
//! and a slider fills its rail up to the knob in the accent. Text fields
//! get room around their text: egui's default margin is 4 by 2 pixels.

use egui::{Color32, Rect, Response, Sense, Stroke, StrokeKind, TextStyle, Ui, WidgetInfo, WidgetText, WidgetType};

/// Height of a bar: a panel's header or a toolbar.
pub const BAR: f32 = 32.0;
/// Side of a button that shows only an icon.
pub const ICON_BUTTON: f32 = 26.0;

/// A row `BAR` tall across the available width, its contents centred
/// vertically: panel headers and toolbars line up with each other, and an
/// icon sits level with the text next to it.
pub fn bar<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    bar_of(ui, BAR, add)
}

/// A row like [`bar`], `height` tall.
pub fn bar_of<R>(ui: &mut Ui, height: f32, add: impl FnOnce(&mut Ui) -> R) -> R {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), height), Sense::hover());
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    add(&mut row)
}

/// Room between a text field's edge and its text.
const FIELD_MARGIN: egui::Margin = egui::Margin::symmetric(8, 5);

/// A one-line text field.
pub fn field(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::singleline(text).margin(FIELD_MARGIN)
}

/// A text field of several lines.
pub fn area(text: &mut dyn egui::TextBuffer) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text).margin(FIELD_MARGIN)
}

/// Size of a dialog's action buttons, at least.
const ACTION_SIZE: egui::Vec2 = egui::vec2(96.0, 32.0);

/// A dialog's main action (Insert, Save…): filled with the accent.
pub fn primary(text: &str) -> egui::Button<'_> {
    egui::Button::new(egui::RichText::new(text).strong().color(Color32::WHITE)).fill(crate::theme::accent()).min_size(ACTION_SIZE)
}

/// A dialog's other actions (Cancel, Close…).
pub fn secondary(text: &str) -> egui::Button<'_> {
    egui::Button::new(text).min_size(ACTION_SIZE)
}

/// How long a copy button says it copied, in seconds.
const COPIED_FOR: f64 = 1.2;

/// A small "Copy" button that turns into "Copied" with a check for a moment
/// after a click: the clipboard gives no sign of its own. The caller copies.
pub fn copy_button(ui: &mut Ui, id: impl std::hash::Hash + std::fmt::Debug) -> Response {
    let id = ui.id().with(id).with("copied-at");
    let now = ui.input(|i| i.time);
    let copied = ui.data(|d| d.get_temp::<f64>(id)).filter(|at| now - at < COPIED_FOR);
    let button = match copied {
        Some(_) => crate::icons::button(crate::icons::Icon::Check, egui::RichText::new(crate::i18n::t("Copied")).color(crate::theme::SUCCESS)),
        None => crate::icons::button(crate::icons::Icon::Copy, crate::i18n::t("Copy")),
    };
    let response = ui.add(button.small());
    if response.clicked() {
        ui.data_mut(|d| d.insert_temp(id, now));
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(COPIED_FOR));
    } else if let Some(at) = copied {
        ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64(COPIED_FOR - (now - at) + 0.01));
    }
    response
}

/// A checkbox. Flips `checked` when clicked.
pub fn checkbox(ui: &mut Ui, checked: &mut bool, text: impl Into<WidgetText>) -> Response {
    let mut response = toggle(ui, *checked, text.into(), false);
    if response.clicked() {
        *checked = !*checked;
        response.mark_changed();
    }
    response
}

/// A slider with its rail filled in the accent up to the knob.
pub fn slider(ui: &mut Ui, slider: egui::Slider<'_>) -> Response {
    ui.scope(|ui| {
        ui.visuals_mut().selection.bg_fill = crate::theme::accent();
        ui.add(slider.trailing_fill(true))
    })
    .inner
}

fn toggle(ui: &mut Ui, on: bool, text: WidgetText, round: bool) -> Response {
    let icon = ui.spacing().icon_width;
    let gap = ui.spacing().icon_spacing;
    let wrap = ui.available_width() - icon - gap;
    let galley = text.into_galley(ui, None, wrap, TextStyle::Button);
    let size = egui::vec2(icon + gap + galley.size().x, galley.size().y.max(ui.spacing().interact_size.y));
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    let kind = if round { WidgetType::RadioButton } else { WidgetType::Checkbox };
    response.widget_info(|| WidgetInfo::selected(kind, ui.is_enabled(), on, galley.text()));
    if ui.is_rect_visible(rect) {
        let visuals = ui.style().interact(&response);
        let square = Rect::from_center_size(egui::pos2(rect.left() + icon / 2.0, rect.center().y), egui::vec2(icon, icon));
        let radius = if round { icon / 2.0 } else { 4.0 };
        let painter = ui.painter();
        // The accent fills in and the mark draws itself; the edge warms on
        // hover. All within theme::MOTION.
        let ctx = ui.ctx();
        let k = ctx.animate_bool_with_time(response.id.with("on"), on, crate::theme::MOTION);
        let hover = ctx.animate_bool_with_time(response.id.with("hover"), response.hovered(), crate::theme::MOTION);
        if k < 1.0 {
            let edge = ui.visuals().weak_text_color().lerp_to_gamma(crate::theme::accent(), hover);
            painter.rect(square, radius, visuals.bg_fill, Stroke::new(1.0, edge), StrokeKind::Inside);
        }
        if k > 0.0 {
            let fill = crate::theme::accent().lerp_to_gamma(crate::theme::accent().gamma_multiply(0.85), hover).gamma_multiply(k);
            painter.rect(square, radius, fill, Stroke::NONE, StrokeKind::Inside);
            if round {
                painter.circle_filled(square.center(), icon * 0.2 * k, Color32::WHITE);
            } else {
                // The check traced from its left end.
                let r = square.shrink(icon * 0.22);
                let a = egui::pos2(r.left(), r.center().y);
                let b = egui::pos2(r.left() + r.width() * 0.38, r.bottom());
                let c = egui::pos2(r.right(), r.top());
                let (first, second) = (a.distance(b), b.distance(c));
                let drawn = k * (first + second);
                let mut points = vec![a, a.lerp(b, (drawn / first).min(1.0))];
                if drawn > first {
                    points.push(b.lerp(c, (drawn - first) / second));
                }
                painter.add(egui::Shape::line(points, Stroke::new(2.0, Color32::WHITE.gamma_multiply(k))));
            }
        }
        let pos = egui::pos2(rect.left() + icon + gap, rect.center().y - galley.size().y / 2.0);
        painter.galley(pos, galley, visuals.text_color());
    }
    response
}

/// An on/off switch: a pill that fills with the accent when on, its knob
/// sliding across. Flips `on` when clicked.
pub fn switch(ui: &mut Ui, on: &mut bool) -> Response {
    let size = egui::vec2(34.0, 20.0);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::selected(WidgetType::Checkbox, ui.is_enabled(), *on, ""));
    if ui.is_rect_visible(rect) {
        let ctx = ui.ctx();
        let k = ctx.animate_bool_with_time(response.id.with("on"), *on, crate::theme::MOTION);
        let hover = ctx.animate_bool_with_time(response.id.with("hover"), response.hovered(), crate::theme::MOTION);
        let w = &ui.visuals().widgets;
        let off = w.inactive.bg_fill.lerp_to_gamma(w.hovered.bg_fill, hover);
        let accent = crate::theme::accent();
        let on_fill = accent.lerp_to_gamma(accent.gamma_multiply(0.85), hover);
        let radius = rect.height() / 2.0;
        ui.painter().rect_filled(rect, radius, off.lerp_to_gamma(on_fill, k));
        let knob = radius - 2.5;
        let x = egui::lerp((rect.left() + radius)..=(rect.right() - radius), k);
        ui.painter().circle_filled(egui::pos2(x, rect.center().y), knob, Color32::WHITE);
    }
    response
}

/// A number with a − and a + on either side, and dragging or typing on the
/// number itself. Kept within `range`, on multiples of `step`.
pub fn stepper(ui: &mut Ui, value: &mut f32, range: std::ops::RangeInclusive<f32>, step: f32, decimals: usize, suffix: &str) -> Response {
    use crate::icons::{icon_button, Icon};
    let (lo, hi) = (*range.start(), *range.end());
    let side = 28.0;
    let size = egui::vec2(side * 2.0 + 64.0, side);
    let (rect, mut response) = ui.allocate_exact_size(size, Sense::hover());
    let edge = ui.visuals().widgets.noninteractive.bg_stroke.color;
    let fill = ui.visuals().text_edit_bg_color();
    ui.painter().rect(rect, 6.0, fill, Stroke::new(1.0, edge), StrokeKind::Inside);
    let mut row = ui.new_child(egui::UiBuilder::new().max_rect(rect).layout(egui::Layout::left_to_right(egui::Align::Center)));
    row.spacing_mut().item_spacing.x = 0.0;
    row.spacing_mut().button_padding = egui::vec2(4.0, 2.0);
    let w = &mut row.visuals_mut().widgets;
    w.inactive.weak_bg_fill = Color32::TRANSPARENT;
    w.inactive.bg_stroke = Stroke::NONE;
    w.hovered.bg_stroke = Stroke::NONE;
    let before = *value;
    if row.add_enabled(*value > lo, icon_button(Icon::Minus).min_size(egui::vec2(side, side))).clicked() {
        *value -= step;
    }
    let drag = egui::DragValue::new(value).range(range.clone()).speed(step * 0.25).fixed_decimals(decimals).suffix(suffix);
    row.add_sized(egui::vec2(size.x - side * 2.0, side), drag);
    if row.add_enabled(*value < hi, icon_button(Icon::Plus).min_size(egui::vec2(side, side))).clicked() {
        *value += step;
    }
    *value = ((*value / step).round() * step).clamp(lo, hi);
    if *value != before {
        response.mark_changed();
    }
    response
}
