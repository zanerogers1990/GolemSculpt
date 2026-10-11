//! Dynamic topology's part of the Remesh card: on or off, how fine, and a
//! line that says when it has stopped at its triangle limit. The wording is
//! the Unity app's.

use egui::Color32;

use super::theme::{self, ACCENT};
use super::tip;
use crate::sculpt::dyntopo::{DynTopo, MAX_DETAIL, MAX_TRIANGLES, MIN_DETAIL};

/// The colour of a warning that is not an error.
pub const WARNING: Color32 = Color32::from_rgb(242, 190, 77);

/// What to say while dynamic topology has stopped adding triangles at its
/// limit; nothing while it has not. The Remesh card says it, and so does the
/// plate over the view while the Sculpt panel is folded away (`status`):
/// a brush that has quietly stopped adding detail looks broken otherwise.
pub fn limit_note(dyntopo: &DynTopo) -> Option<String> {
    (dyntopo.enabled && dyntopo.at_limit).then(|| {
        format!("Triangle limit reached ({}M). Remesh at a lower resolution to keep refining.", MAX_TRIANGLES / 1_000_000)
    })
}

pub fn section(ui: &mut egui::Ui, dyntopo: &mut DynTopo, tips: bool) {
    ui.add_space(6.0);
    theme::heading(ui, "Dynamic Topology");
    tip(
        theme::checkbox(ui, &mut dyntopo.enabled, "Dynamic Topology", ACCENT),
        tips,
        "Adds triangles under the brush as you sculpt, so detail appears where you work. Standard, Clay, Crease, \
         Inflate, Flatten and Layer refine; Move, Pose, Snake Hook and Smooth don't. Remesh evens the whole model \
         out again.",
    );
    let mut percent = dyntopo.detail * 100.0;
    let range = MIN_DETAIL * 100.0..=MAX_DETAIL * 100.0;
    let slider = theme::slider(ui, "Detail (Fine - Coarse)", &mut percent, range, |v| format!("{v:.0}%"));
    if tip(
        slider,
        tips,
        "Longest edge left under the brush, as a percentage of the brush radius: lower is finer. A smaller brush \
         adds finer detail.",
    )
    .changed()
    {
        dyntopo.detail = (percent / 100.0).clamp(MIN_DETAIL, MAX_DETAIL);
    }
    if let Some(text) = limit_note(dyntopo) {
        ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(11.0)).italics().color(WARNING)).wrap());
    }
}
