//! Colours, type and the hand-painted widgets.
//!
//! They live in `sculpt_toolkit::theme` (crates/toolkit/src/theme.rs), so the
//! tools written against the toolkit paint their panels with exactly what
//! the app's own panels use. This module is that one under its old name:
//! add or change a widget there, and both sides get it.

pub use sculpt_toolkit::theme::*;

/// The toolkit's card, with the one thing only the app can add: whether the
/// card is open is remembered between runs (see `crate::prefs::cards`). The
/// app's own panels get this one, since it has the same name; cards a hosted
/// tool draws straight from the toolkit are not remembered.
pub fn category(
    ui: &mut egui::Ui,
    title: &str,
    accent: egui::Color32,
    summary: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    let id = crate::prefs::cards::before(ui, title);
    sculpt_toolkit::theme::category(ui, title, accent, summary, default_open, body);
    crate::prefs::cards::after(ui, id, title, default_open);
}
