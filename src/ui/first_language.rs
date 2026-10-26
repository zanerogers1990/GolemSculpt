//! The first time the app opens on a machine, before anything else: which
//! language the tooltips should be in. Windows' own language is picked to
//! start with, and the words under the choices are shown in whichever is
//! picked, so a person can tell they have found theirs. The interface itself
//! stays in English. Later the Interface card's Tooltip Language changes it.
//!
//! Asked only of a run a person started (not a scripted one), and only while
//! there is no settings file yet: once one is written with the choice in it,
//! the app never asks again.

use egui::{Id, vec2};
use sculpt_toolkit::translate::{self, LANGUAGES};

use super::theme::{self, ACCENT, ButtonStyle};
use crate::doc::Options;

fn id() -> Id {
    Id::new("first_language")
}

/// Has the question shown, from the next frame until it is answered.
pub fn ask(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(id(), true));
}

/// Whether the question is up.
pub fn asking(ctx: &egui::Context) -> bool {
    ctx.data(|d| d.get_temp(id()).unwrap_or(false))
}

/// "Language" in each of the languages (by the language's code), as the
/// question's heading.
const HEADING: [(&str, &str); 11] = [
    ("Language", "en"),
    ("Idioma", "es"),
    ("Langue", "fr"),
    ("Sprache", "de"),
    ("Lingua", "it"),
    ("J\u{119}zyk", "pl"),
    ("Dil", "tr"),
    ("\u{42f}\u{437}\u{44b}\u{43a}", "ru"),
    ("\u{8a00}\u{8a9e}", "ja"),
    ("\u{8bed}\u{8a00}", "zh"),
    ("\u{c5b8}\u{c5b4}", "ko"),
];

/// The heading, each word in its own language's font.
fn heading() -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let format = |font| egui::TextFormat { font_id: font, color: theme::TEXT(), ..Default::default() };
    for (i, (word, code)) in HEADING.iter().enumerate() {
        if i > 0 {
            job.append(" \u{b7} ", 0.0, format(theme::font(13.0)));
        }
        let language = translate::find(code);
        let font = if language.name_font(13.0) == theme::font(13.0) { theme::bold(13.0) } else { language.name_font(13.0) };
        job.append(word, 0.0, format(font));
    }
    job
}

/// Draws the question while it is up; a language picked is put in `o` at
/// once, so what is written under the choices turns to it.
pub fn show(ctx: &egui::Context, o: &mut Options) {
    if !asking(ctx) {
        return;
    }
    translate::load_every_script(ctx);
    let frame = egui::Frame::new()
        .fill(theme::CARD())
        .corner_radius(10.0)
        .stroke(theme::hairline())
        .inner_margin(egui::Margin::same(16));
    let mut done = ctx.input(|i| i.key_pressed(egui::Key::Enter));
    egui::Modal::new(id()).frame(frame).show(ctx, |ui| {
        ui.set_width(360.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 10.0);
        let mut job = heading();
        job.wrap.max_width = ui.available_width();
        ui.add(egui::Label::new(job).wrap());
        ui.spacing_mut().item_spacing.y = 6.0;
        for pair in LANGUAGES.chunks(2) {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let width = theme::row_width(ui, 2, 6.0);
                for language in pair {
                    // The button bare, and the name over it in its own letters.
                    let on = o.tooltip_language == language.code;
                    let style = ButtonStyle { active: on.then_some(ACCENT()), ..Default::default() };
                    let button = theme::button(ui, width, "", style);
                    let ink = if on { egui::Color32::WHITE } else { theme::TEXT() };
                    let at = button.rect.center();
                    ui.painter().text(at, egui::Align2::CENTER_CENTER, language.name, language.name_font(12.0), ink);
                    if button.clicked() {
                        o.tooltip_language = language.code;
                    }
                }
            });
        }
        ui.add_space(4.0);
        let say = |ui: &mut egui::Ui, english: &str, size: f32, color| {
            let text = egui::RichText::new(translate::tooltip(english)).font(theme::font(size)).color(color);
            ui.add(egui::Label::new(text).wrap());
        };
        say(
            ui,
            "Pick the language you would like the tooltips in: they say what a button does when the pointer rests on it. \
             The buttons and cards keep their English names, so a tooltip names them as they are on screen.",
            13.0,
            theme::TEXT(),
        );
        say(ui, "This can be changed later on the Settings tab, in the Interface card.", 11.0, theme::HINT());
        ui.add_space(2.0);
        let style = ButtonStyle { badge: "Enter", active: Some(ACCENT()), ..Default::default() };
        if theme::button(ui, ui.available_width(), "OK", style).clicked() {
            done = true;
        }
    });
    if done {
        ctx.data_mut(|d| d.remove::<bool>(id()));
    }
}
