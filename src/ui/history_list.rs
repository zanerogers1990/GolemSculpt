//! The History list: the … button at the end of the Sculpt panel's title line,
//! over Undo and Redo, opens it. Every step
//! Undo would take back is there by name, the latest at the top, and a tap on
//! one goes back to just before it, however many presses of Undo that is;
//! below them, what Redo would bring back, the next first. The names are the
//! history's own (app/undo_names.rs).
//!
//! While SSpheres or a hosted tool is up, Ctrl+Z steps through that tool's own
//! history first, so the list, which is the scene's, is not offered then.

use egui::Ui;

use super::theme::{self, ButtonStyle};
use crate::doc::{Action, Doc, Tool};

/// How many steps each way the list shows. Undo keeps more; they are a
/// scroll of Z away.
const SHOWN: usize = 20;

/// How wide the button is.
pub(super) const WIDTH: f32 = 30.0;

/// The button and the list it opens.
pub(super) fn button(ui: &mut Ui, doc: &Doc, tips: bool, actions: &mut Vec<Action>) {
    let (back, forward) = doc.history.names();
    let own_history = matches!(doc.tool, Tool::SSpheres | Tool::Plugin);
    let usable = !own_history && !(back.is_empty() && forward.is_empty());
    let style = ButtonStyle { height: 24.0, ..Default::default() };
    let r = ui.add_enabled_ui(usable, |ui| theme::button(ui, WIDTH, "\u{2026}", style)).inner;
    let help = if own_history {
        "While this tool is up, Ctrl+Z steps through its own history first; the scene's history list is here again once it is put away."
    } else {
        "History: every step Undo would take back, by name. Tap one to go back to just before it."
    };
    let r = if tips { sculpt_toolkit::translate::hover(r, help) } else { r };
    egui::Popup::menu(&r).show(|ui| {
        ui.set_min_width(220.0);
        if !back.is_empty() {
            theme::heading(ui, "Undo back to before");
        }
        for (i, name) in back.iter().take(SHOWN).enumerate() {
            if ui.button(name).clicked() {
                actions.push(Action::UndoSteps(i + 1));
                ui.close();
            }
        }
        if back.len() > SHOWN {
            theme::hint(ui, &format!("and {} more", back.len() - SHOWN));
        }
        if !forward.is_empty() {
            ui.separator();
            theme::heading(ui, "Redo up to");
        }
        for (i, name) in forward.iter().take(SHOWN).enumerate() {
            if ui.button(name).clicked() {
                actions.push(Action::RedoSteps(i + 1));
                ui.close();
            }
        }
    });
}
