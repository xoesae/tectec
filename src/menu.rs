//! Context menus with the look of GPUI Component's `PopupMenu`
//! (https://gpui-kit.com/component/menu/), drawn with egui: a surface 4px
//! inside a hairline ring and a short shadow, rows 26px tall with rounded
//! corners that fill in a neutral tone under the pointer, an icon column on
//! the left when any row has an icon, and a shortcut or an arrow on the right.
//!
//! Only the look is copied; opening, closing and submenus are egui's own.
//!
//! ```ignore
//! menu::context(&response, |ui| {
//!     if ui.add(menu::item(t("Copy")).icon(Icon::Copy).shortcut("Ctrl+C")).clicked() { … }
//!     menu::separator(ui);
//!     if ui.add(menu::item(t("Delete")).danger()).clicked() { … }
//! });
//! ```

use egui::{Align2, Color32, CornerRadius, FontSelection, Id, Margin, Rect, Response, Sense, Shadow, Stroke, TextStyle, Ui, Widget, WidgetInfo, WidgetText, WidgetType};

use crate::icons::Icon;

/// Height of a row (GPUI: 1.625rem).
const ROW: f32 = 26.0;
/// Room left and right of a row's contents (GPUI: 0.5rem).
const PAD_X: f32 = 8.0;
/// Corners of the surface and of a row's highlight.
const RADIUS: u8 = 6;
/// Width of the icon column.
const ICON_SLOT: f32 = 16.0;
/// Side of an icon in the icon column.
const ICON: f32 = 14.0;
/// Side of the submenu arrow (GPUI: xsmall).
const ARROW: f32 = 12.0;
/// Between the icon column and the label (GPUI: gap_x_1).
const ICON_GAP: f32 = 4.0;
/// Between the label and what sits on the right (GPUI: gap_3).
const RIGHT_GAP: f32 = 12.0;
/// The narrowest a menu gets (GPUI: 8rem).
const MIN_WIDTH: f32 = 128.0;

/// The menu's look, applied to the popup and inherited by its submenus.
fn look(style: &mut egui::Style) {
    egui::containers::menu::menu_style(style);
    let dark = style.visuals.dark_mode;
    let text = style.visuals.widgets.noninteractive.fg_stroke.color;
    // No border: a ring of the text colour at a tenth, then a short shadow.
    style.visuals.window_stroke = Stroke::new(1.0, Color32::from_rgba_unmultiplied(text.r(), text.g(), text.b(), 26));
    style.visuals.popup_shadow = Shadow { offset: [0, 3], blur: 6, spread: 0, color: Color32::from_black_alpha(if dark { 70 } else { 26 }) };
    style.visuals.menu_corner_radius = CornerRadius::same(RADIUS);
    style.spacing.menu_margin = Margin::same(4);
    // Rows 2px apart (GPUI: gap_y_0p5).
    style.spacing.item_spacing.y = 2.0;
}

/// Shows `add` in a context menu when `response` is right-clicked.
pub fn context(response: &Response, add: impl FnOnce(&mut Ui)) {
    egui::Popup::context_menu(response).style(look).show(|ui| {
        ui.set_min_width(MIN_WIDTH);
        add(ui);
    });
}

/// A menu of the menu bar: `header` opens `add` below it.
pub fn bar<R>(ui: &mut Ui, header: &str, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let config = egui::containers::menu::MenuConfig::new().style(look);
    egui::containers::menu::MenuButton::new(header)
        .config(config)
        .ui(ui, |ui| {
            ui.set_min_width(MIN_WIDTH);
            add(ui)
        })
        .1
        .map(|r| r.inner)
}

/// A menu that drops from `button` when it is clicked: the button's
/// response, and what `add` returned while the menu was open.
pub fn dropdown<R>(ui: &mut Ui, button: egui::Button<'_>, add: impl FnOnce(&mut Ui) -> R) -> (Response, Option<R>) {
    let config = egui::containers::menu::MenuConfig::new().style(look);
    let (response, inner) = egui::containers::menu::MenuButton::from_button(button).config(config).ui(ui, |ui| {
        ui.set_min_width(MIN_WIDTH);
        add(ui)
    });
    (response, inner.map(|r| r.inner))
}

/// A row of a menu: `ui.add(menu::item(…))`, or `ui.add_enabled(…)` for one
/// that can be greyed out.
pub fn item(label: impl Into<WidgetText>) -> Item {
    Item { label: label.into(), icon: None, checked: false, selected: false, indent: 0.0, shortcut: None, danger: false, arrow: false }
}

pub struct Item {
    label: WidgetText,
    icon: Option<Icon>,
    checked: bool,
    selected: bool,
    indent: f32,
    shortcut: Option<String>,
    danger: bool,
    arrow: bool,
}

impl Item {
    /// An icon in the column on the left.
    pub fn icon(mut self, icon: Icon) -> Self {
        self.icon = Some(icon);
        self
    }

    /// A check in the column on the left: a choice that is on.
    pub fn checked(mut self, on: bool) -> Self {
        self.checked = on;
        self
    }

    /// The current one among the rows (the open tab, the section at the
    /// cursor): lit as if under the pointer.
    pub fn selected(mut self, on: bool) -> Self {
        self.selected = on;
        self
    }

    /// Room before the label, for rows nested under others.
    pub fn indent(mut self, px: f32) -> Self {
        self.indent = px;
        self
    }

    /// The keys that do the same, faint on the right.
    pub fn shortcut(mut self, keys: impl Into<String>) -> Self {
        self.shortcut = Some(keys.into());
        self
    }

    /// A destructive action: its text in the error colour.
    pub fn danger(mut self) -> Self {
        self.danger = true;
        self
    }

    fn arrow(mut self) -> Self {
        self.arrow = true;
        self
    }
}

/// Whether this menu reserves the icon column: some row asked for it on this
/// pass or the last one.
fn icon_column(ui: &Ui, wants: bool) -> bool {
    let id = egui::containers::menu::find_menu_root(ui).id.with("menu-icon-column");
    let pass = ui.ctx().cumulative_pass_nr();
    if wants {
        ui.data_mut(|d| d.insert_temp(id, pass));
        return true;
    }
    ui.data(|d| d.get_temp::<u64>(id)).is_some_and(|at| at + 1 >= pass)
}

impl Widget for Item {
    fn ui(self, ui: &mut Ui) -> Response {
        let reserve = icon_column(ui, self.icon.is_some() || self.checked);
        let left = if reserve { ICON_SLOT + ICON_GAP } else { 0.0 } + self.indent;
        let galley = self.label.into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, FontSelection::Style(TextStyle::Body));
        let keys = self.shortcut.map(|k| WidgetText::from(k).into_galley(ui, Some(egui::TextWrapMode::Extend), f32::INFINITY, TextStyle::Small));
        let right = match (&keys, self.arrow) {
            (Some(k), _) => RIGHT_GAP + k.size().x,
            (None, true) => RIGHT_GAP + ARROW,
            _ => 0.0,
        };
        let width = PAD_X + left + galley.size().x + right + PAD_X;
        // The layout is justified: the row stretches to the menu's width.
        let response = ui.allocate_response(egui::vec2(width, ROW), Sense::click());
        let rect = response.rect;
        response.widget_info(|| WidgetInfo::labeled(WidgetType::Button, ui.is_enabled(), galley.text()));
        if !ui.is_rect_visible(rect) {
            return response;
        }
        let visuals = ui.visuals();
        let enabled = ui.is_enabled();
        let weak = visuals.weak_text_color();
        let lit = enabled && (self.selected || response.hovered() || response.is_pointer_button_down_on() || response.has_focus() || open_submenu(ui, &response));
        if lit {
            ui.painter().rect_filled(rect, RADIUS, crate::theme::menu_hover(visuals.dark_mode));
        }
        let color = if !enabled {
            weak
        } else if self.danger {
            visuals.error_fg_color
        } else {
            visuals.widgets.noninteractive.fg_stroke.color
        };
        let inner = rect.shrink2(egui::vec2(PAD_X, 0.0));
        if reserve {
            let slot = Rect::from_center_size(egui::pos2(inner.left() + self.indent + ICON_SLOT / 2.0, inner.center().y), egui::vec2(ICON, ICON));
            let icon = if self.checked { Some(Icon::Check) } else { self.icon };
            if let Some(icon) = icon {
                icon.image(color, ICON).paint_at(ui, slot);
            }
        }
        let text_pos = egui::pos2(inner.left() + left, inner.center().y - galley.size().y / 2.0);
        ui.painter().galley(text_pos, galley, color);
        if let Some(keys) = keys {
            let pos = Align2::RIGHT_CENTER.anchor_size(egui::pos2(inner.right(), inner.center().y), keys.size()).min;
            ui.painter().galley(pos, keys, weak);
        } else if self.arrow {
            let slot = Rect::from_center_size(egui::pos2(inner.right() - ARROW / 2.0, inner.center().y), egui::vec2(ARROW, ARROW));
            Icon::ChevronRight.image(weak, ARROW).paint_at(ui, slot);
        }
        response
    }
}

/// Whether the submenu hanging off `response` is open (set by [`submenu`]).
fn open_submenu(ui: &Ui, response: &Response) -> bool {
    ui.data(|d| d.get_temp::<bool>(response.id.with("menu-sub-open"))).unwrap_or(false)
}

/// A row that opens `add` in a menu beside it while the pointer is on it.
pub fn submenu<R>(ui: &mut Ui, label: impl Into<WidgetText>, icon: Option<Icon>, add: impl FnOnce(&mut Ui) -> R) -> Option<R> {
    let mut row = item(label).arrow();
    if let Some(icon) = icon {
        row = row.icon(icon);
    }
    let response = ui.add(row);
    let shown = egui::containers::menu::SubMenu::new().show(ui, &response, |ui| {
        ui.set_min_width(MIN_WIDTH);
        add(ui)
    });
    let key: Id = response.id.with("menu-sub-open");
    let open = shown.is_some();
    if ui.data(|d| d.get_temp::<bool>(key)) != Some(open) {
        ui.data_mut(|d| d.insert_temp(key, open));
        ui.ctx().request_repaint();
    }
    shown.map(|r| r.inner)
}

/// A line between groups of rows, edge to edge, 4px clear of each (GPUI:
/// the rows' 2px gap plus my_0p5).
pub fn separator(ui: &mut Ui) {
    let margin = ui.spacing().menu_margin.left as f32;
    // No width of its own: the justified layout stretches it to the rows'.
    let rect = ui.allocate_response(egui::vec2(0.0, 5.0), Sense::hover()).rect;
    let color = ui.visuals().widgets.noninteractive.bg_stroke.color;
    ui.painter().hline((rect.left() - margin)..=(rect.right() + margin), rect.center().y, Stroke::new(1.0, color));
}

/// A heading or a note in a menu: faint, small, not clickable.
pub fn label(ui: &mut Ui, text: impl Into<String>) {
    let text = egui::RichText::new(text).small().color(ui.visuals().weak_text_color());
    // Level with the rows' labels, past the icon column when there is one.
    let left = PAD_X + if icon_column(ui, false) { ICON_SLOT + ICON_GAP } else { 0.0 };
    let margin = Margin { left: left as i8, right: PAD_X as i8, top: 3, bottom: 3 };
    egui::Frame::new().inner_margin(margin).show(ui, |ui| {
        ui.add(egui::Label::new(text).wrap());
    });
}
