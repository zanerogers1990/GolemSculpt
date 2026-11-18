//! The welcome card: what a first-time visitor needs to start, over the
//! middle of the page on the first visit, and again whenever the Help button
//! (bottom bar) or F1 asks for it. Closing it with "Don't show this again"
//! ticked is remembered in the browser (`golem.pref`, key "welcome"), so a
//! returning sculptor is not stopped by it; the Settings tab's How to Use
//! card keeps the full list of keys.

use eframe::egui::{self, Color32, Id, Key, Modifiers, RichText};

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
            "File tab: Save Scene downloads everything to carry on elsewhere; OBJ, STL and FBX are for other programs.",
        ],
    ),
];

/// The same card on a phone, in fingers rather than keys and buttons (mobile.rs).
const PHONE_PARTS: [(&str, &[&str]); 4] = [
    (
        "Sculpt",
        &[
            "Draw on the model with one finger to sculpt.",
            "Pick a brush on the strip along the bottom; slide it for more.",
            "Carve makes the brush dig in instead of building up. Smooth softens. Each stays on until tapped again.",
            "Size and Strength are the two sliders down the left: drag up for more.",
        ],
    ),
    (
        "Look around",
        &[
            "One finger off the model turns the view, as does a drag on the axis ball.",
            "Two fingers: pinch to zoom, drag to move the view.",
            "Fit View (top right) brings the model back into view.",
        ],
    ),
    (
        "Undo",
        &["Tap with two fingers to undo, with three to redo, or use the buttons at the top."],
    ),
    (
        "Everything else",
        &[
            "Menu (top left) holds the rest: saving, finer detail, new shapes, the tools and every setting.",
            "Your work is kept in this browser between visits.",
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
    fn welcome_first_look(&mut self, ctx: &egui::Context) {
        let w = &mut self.welcome;
        if !w.decided {
            w.decided = true;
            w.open = crate::browser::pref(PREF) != SEEN;
            // A phone gets the tour that points at things instead (mobile.rs).
            if w.open && super::mobile::compact(ctx) {
                w.open = false;
                self.touch.tour = Some(0);
            }
        }
    }

    /// The first visit is over: the welcome does not open by itself again.
    pub(super) fn welcome_seen(&mut self) {
        crate::browser::set_pref(PREF, SEEN);
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
        self.welcome_first_look(ctx);
        if ctx.input_mut(|i| i.consume_key(Modifiers::NONE, Key::F1)) {
            if self.welcome.open { self.close_welcome() } else { self.open_welcome() }
        }
        if !self.welcome.open {
            return;
        }
        let screen = ctx.content_rect();
        let width = (screen.width() - 32.0).clamp(240.0, 460.0);
        // egui's own modal: it dims the page, keeps clicks off what is under
        // it, and lets go of the pointer cleanly when it closes (a backdrop
        // of our own left the next click landing on nothing). A click on the
        // dimmed page or Escape closes it too.
        let frame = egui::Frame::new()
            .fill(theme::PANEL())
            .stroke(egui::Stroke::new(1.0, theme::CONTROL()))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::same(18));
        let modal = egui::Modal::new(Id::new("welcome_card"))
            .backdrop_color(Color32::from_black_alpha(110))
            .frame(frame)
            .show(ctx, |ui| {
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
                    let parts: &[(&str, &[&str])] = if super::mobile::compact(ctx) { &PHONE_PARTS } else { &PARTS };
                    for &(heading, lines) in parts {
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
                let start = theme::button(ui, ui.available_width(), "Start Sculpting", style).clicked();
                ui.add_space(2.0);
                let again = if super::mobile::compact(ctx) {
                    "Help, at the foot of the Menu, brings this back."
                } else {
                    "F1 or Help (bottom of the page) brings this back. Settings has every key."
                };
                ui.label(
                    RichText::new(again)
                        .font(theme::font(11.0))
                        .color(theme::HINT()),
                );
                start
            });
        let close = modal.inner || modal.should_close();
        if close {
            self.close_welcome();
        }
    }
}
