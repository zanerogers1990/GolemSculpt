//! The Reconstruct Subdiv button of the Subdivision card, shown for a quad
//! model without levels: one brought in from ZBrush or Blender at its finest
//! level gets the levels below it back (`app/levels/reconstruct.rs`).
//!
//! Whether the quads really were made by subdividing is only found out when
//! it is pressed, because finding out reads every face; the status line says
//! so in words when they were not.

use super::super::theme::{self, ButtonStyle};
use super::super::tip;
use crate::doc::Action;
use crate::mesh::Mesh;

/// The button and a line on what it is for, when the object is quads and has
/// no levels yet.
pub(super) fn button(ui: &mut egui::Ui, mesh: &Mesh, tips: bool, actions: &mut Vec<Action>) {
    if mesh.levels.is_some() || mesh.polys.is_none() {
        return;
    }
    ui.add_space(4.0);
    let help = "For a model subdivided in ZBrush or Blender and brought in at its finest level: finds the lower \
                levels it was made from, so you can step down (Page Down), change the big forms and come back up \
                with the detail kept. The model does not change. Undo (Ctrl+Z) takes the levels away again.";
    let button = theme::button(ui, ui.available_width(), "Reconstruct Subdiv", ButtonStyle::default());
    if tip(button, tips, help).clicked() {
        actions.push(Action::ReconstructSubdiv);
    }
    theme::hint(ui, "This model is quads. If it was subdivided elsewhere, Reconstruct finds its lower levels.");
}
