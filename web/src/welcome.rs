//! The welcome card: what a first-time visitor needs to start, over the
//! middle of the page on the first visit, and again whenever the Help button
//! (bottom bar) or F1 asks for it. Closing it with "Don't show this again"
//! ticked is remembered in the browser (`golem.pref`, key "welcome"), so a
//! returning sculptor is not stopped by it; the Settings tab's How to Use
//! card keeps the full list of keys.

use eframe::egui::{self, Align2, Color32, Id, Key, Modifiers, Order, RichText};

use super::GolemApp;
use super::ui::theme::{self, ACCENT, ButtonStyle};

/// The key the browser keeps "don't show again" under, and its value.
const PREF: &str = "welcome";
const SEEN: &str = "seen";

/// What the card shows, a heading and its lines.
const PARTS: [(&str, &[&str]); 4] = [
    (
        "Sculpt",
        &[
            "Draw on the model to sculpt. A pen's pressure is read.",
            "Ctrl, or the right mouse button, does the brush's opposite. Shift smooths.",
            "Pick brushes on the left panel, or hold Space for the brush wheel.",
        ],
    ),
    (
        "Look around",
        &[
            "Drag off the model to turn the view. Middle drag moves it, the wheel zooms.",
            "On a touch screen: one finger off the model turns, two fingers move and zoom.",
            "Tap F twice to frame the model.",
        ],
    ),
    (
        "Build up",
        &[
            "Mesh tab: Subdivide for finer detail, Remesh to even it out, Layers to keep passes apart.",
            "Tools tab: Tubes, Muscles, Straps, Lathe, Hands and more. Objects tab: several objects.",
        ],
    ),
    (
        "Keep it",
        &[
            "Your work is kept in this browser between visits. Ctrl+Z undoes.",
            "File tab: Save Scene downloads everything to carry on elsewhere; OBJ and STL are for other programs.",
        ],
    ),
];

#[derive(Default)]
pub struct Welcome {
    pub open: bool,
    /// "Don't show this again", as the tick stands.
    pub dont_show: bool,
    /// The browser said, at the start, that it was seen before.
    decided: bool,
}

impl GolemApp {
    /// Opens the card on a first visit; afterwards only F1 or Help opens it.
    fn welcome_first_look(&mut self) {
        let w = &mut self.welcome;
        if !w.decided {
            w.decided = true;
            w.open = crate::browser::pref(PREF) != SEEN;
        }
    }

    pub(super) fn open_welcome(&mut self) {
        self.welcome.open = true;
        self.welcome.dont_show = crate::browser::pref(PREF) == SEEN;
    }

    fn close_welcome(&mut self) {
        self.welcome.open = false;
        crate::browser::set_pref(PREF, if self.welcome.dont_show { SEEN } else { "" });
    }

    /// Once a frame, after the panels: the card, if it is up.
    pub(super) fn welcome(&mut self, ctx: &egui::Context) {
        self.welcome_first_look();
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            if self.welcome.open { self.close_welcome() } else { self.open_welcome() }
        }
        if !self.welcome.open {
            return;
        }
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::Escape)) {
            self.close_welcome();
            return;
        }
        let screen = ctx.content_rect();
        // A dimmed page behind the card; a click on it closes the card.
        let shade = egui::Area::new(Id::new("welcome_shade"))
            .order(Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                let (rect, response) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                ui.painter().rect_filled(rect, 0.0, Color32::from_black_alpha(110));
                response
            });
        let width = (screen.width() - 32.0).clamp(240.0, 460.0);
        let mut close = shade.inner.clicked();
        // Over the shade, in the same layer as it: the tooltip layer would be
        // on top too, but takes no clicks.
        let card = egui::Area::new(Id::new("welcome_card"))
            .order(Order::Foreground)
            .pivot(Align2::CENTER_CENTER)
            .fixed_pos(screen.center())
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(theme::PANEL())
                    .stroke(egui::Stroke::new(1.0, theme::CONTROL()))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::same(18))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.spacing_mut().item_spacing.y = 4.0;
                        ui.label(RichText::new("Welcome to GolemSculpt").font(theme::font(20.0)).color(theme::TEXT()));
                        ui.label(
                            RichText::new("Digital clay in your browser. Nothing to install, and nothing you make leaves this computer.")
                                .font(theme::font(12.0))
                                .color(theme::HINT()),
                        );
                        let room = (screen.height() - 220.0).max(120.0);
                        egui::ScrollArea::vertical().max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                            for (heading, lines) in PARTS {
                                ui.add_space(8.0);
                                ui.label(RichText::new(heading).font(theme::font(14.0)).color(ACCENT()));
                                for line in lines {
                                    ui.add(egui::Label::new(RichText::new(*line).font(theme::font(12.0)).color(theme::TEXT())).wrap());
                                }
                            }
                        });
                        ui.add_space(12.0);
                        ui.horizontal(|ui| {
                            theme::checkbox(ui, &mut self.welcome.dont_show, "Don't show this again", ACCENT());
                        });
                        ui.add_space(6.0);
                        let style = ButtonStyle { active: Some(ACCENT()), ..Default::default() };
                        if theme::button(ui, ui.available_width(), "Start Sculpting", style).clicked() {
                            close = true;
                        }
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("F1 or Help (bottom of the page) brings this back. Settings has every key.")
                                .font(theme::font(11.0))
                                .color(theme::HINT()),
                        );
                    });
            });
        // A click on the shade would raise it over the card: keep the card on top.
        ctx.move_to_top(card.response.layer_id);
        if close {
            self.close_welcome();
        }
    }
}
