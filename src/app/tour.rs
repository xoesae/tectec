//! The first-run tour: once the first project is open, four stops dim the
//! window around one part of it (sidebar, editor, preview, the palette)
//! and say in a sentence what it is for. Seen once; "Show the tour" in the
//! Help menu brings it back. See docs/tour.md.

use crate::i18n::{n, t};
use egui::{Color32, Rect, Stroke};

/// Where the parts of the window were drawn this frame, for the spotlight.
#[derive(Default, Clone, Copy)]
pub struct Spots {
    pub sidebar: Option<Rect>,
    pub editor: Option<Rect>,
    pub preview: Option<Rect>,
    pub menu: Option<Rect>,
}

/// One stop: what it lights, a title and a sentence.
struct Stop {
    spot: fn(&Spots) -> Option<Rect>,
    title: &'static str,
    body: &'static str,
}

const STOPS: [Stop; 4] = [
    Stop {
        spot: |s| s.sidebar,
        title: n("The project"),
        body: n("Files, sections, references and images. The bar on the left switches between them."),
    },
    Stop {
        spot: |s| s.editor,
        title: n("The editor"),
        body: n("Write here. The toolbar above inserts sections, lists, tables, images and formulas."),
    },
    Stop {
        spot: |s| s.preview,
        title: n("The preview"),
        body: n("The PDF compiles by itself when you pause typing. Errors show up below the editor."),
    },
    Stop {
        spot: |s| s.menu,
        title: n("Every command"),
        body: n("Ctrl+Shift+P opens the command palette: type what you want to do."),
    },
];

const CALLOUT_WIDTH: f32 = 300.0;

#[derive(Default)]
pub struct Tour {
    step: usize,
}

impl Tour {
    /// Draws the current stop over everything. False once it is over.
    pub fn show(&mut self, ctx: &egui::Context, spots: &Spots) -> bool {
        // A stop whose part is hidden (a closed sidebar…) is skipped.
        while self.step < STOPS.len() && (STOPS[self.step].spot)(spots).is_none() {
            self.step += 1;
        }
        let Some(stop) = STOPS.get(self.step) else { return false };
        let Some(spot) = (stop.spot)(spots) else { return false };
        let last = !STOPS[self.step + 1..].iter().any(|s| (s.spot)(spots).is_some());
        let mut next = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter));
        let mut end = ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape));

        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("tour"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                // Swallows clicks and scrolls meant for what is under the veil.
                ui.allocate_rect(screen, egui::Sense::click_and_drag());
                let painter = ui.painter();
                let veil = Color32::from_black_alpha(150);
                let hole = spot.shrink(1.0);
                for r in [
                    Rect::from_min_max(screen.min, egui::pos2(screen.max.x, hole.min.y)),
                    Rect::from_min_max(egui::pos2(screen.min.x, hole.max.y), screen.max),
                    Rect::from_min_max(egui::pos2(screen.min.x, hole.min.y), egui::pos2(hole.min.x, hole.max.y)),
                    Rect::from_min_max(egui::pos2(hole.max.x, hole.min.y), egui::pos2(screen.max.x, hole.max.y)),
                ] {
                    painter.rect_filled(r, 0.0, veil);
                }
                painter.rect_stroke(hole, 6.0, Stroke::new(2.0, crate::theme::accent()), egui::StrokeKind::Inside);

                // The callout beside the spot: right, else left, else below,
                // else inside it.
                let size = egui::vec2(CALLOUT_WIDTH, 132.0);
                let gap = 14.0;
                let min = if spot.max.x + gap + size.x < screen.max.x {
                    egui::pos2(spot.max.x + gap, spot.min.y + 40.0)
                } else if spot.min.x - gap - size.x > screen.min.x {
                    egui::pos2(spot.min.x - gap - size.x, spot.min.y + 40.0)
                } else if spot.max.y + gap + size.y < screen.max.y {
                    egui::pos2(spot.center().x - size.x / 2.0, spot.max.y + gap)
                } else {
                    egui::pos2(spot.center().x - size.x / 2.0, spot.max.y - size.y - 40.0)
                };
                let min = egui::pos2(min.x, min.y.min(screen.max.y - size.y - gap).max(screen.min.y + gap));
                let callout = Rect::from_min_size(min, egui::vec2(size.x, 0.0));
                let mut card = ui.new_child(egui::UiBuilder::new().max_rect(callout).layout(egui::Layout::top_down(egui::Align::Min)));
                egui::Frame::popup(card.style()).inner_margin(16).show(&mut card, |ui| {
                    ui.set_width(CALLOUT_WIDTH - 32.0);
                    ui.label(egui::RichText::new(t(stop.title)).strong().size(15.0));
                    ui.add_space(4.0);
                    ui.label(t(stop.body));
                    ui.add_space(12.0);
                    ui.horizontal(|ui| {
                        crate::theme::framed(ui);
                        ui.weak(format!("{} / {}", self.step + 1, STOPS.len()));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.add(crate::widgets::primary(if last { t("Done") } else { t("Next") })).clicked() {
                                next = true;
                            }
                            if !last && ui.add(egui::Button::new(t("Skip")).frame(false)).clicked() {
                                end = true;
                            }
                        });
                    });
                });
            });
        if end || (next && last) {
            return false;
        }
        if next {
            self.step += 1;
        }
        true
    }
}
