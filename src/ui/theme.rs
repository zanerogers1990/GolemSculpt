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
/// tool draws straight from the toolkit are not remembered. (Which cards
/// float is remembered for both: that is the toolkit's own doing.)
pub fn category(
    ui: &mut egui::Ui,
    title: &str,
    accent: egui::Color32,
    summary: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    category_as(ui, title, title, accent, summary, default_open, body);
}

/// A [`category`] whose title changes with what it shows: `key` is the one
/// name it floats under (see `sculpt_toolkit::theme::category_as`).
pub fn category_as(
    ui: &mut egui::Ui,
    key: &str,
    title: &str,
    accent: egui::Color32,
    summary: &str,
    default_open: bool,
    body: impl FnOnce(&mut egui::Ui),
) {
    // A folded panel run for its floating cards alone is not the panel the
    // open and closed states are kept under: leave those for the real one.
    if sculpt_toolkit::floating::hidden(ui) {
        return sculpt_toolkit::theme::category_as(ui, key, title, accent, summary, default_open, body);
    }
    let id = crate::prefs::cards::before(ui, title);
    sculpt_toolkit::theme::category_as(ui, key, title, accent, summary, default_open, body);
    crate::prefs::cards::after(ui, id, title, default_open);
}

/// The toolkit's fold heading, remembered between runs as a card is.
pub fn fold(ui: &mut egui::Ui, title: &str, default_open: bool, open_anyway: bool) -> bool {
    if sculpt_toolkit::floating::hidden(ui) {
        return sculpt_toolkit::theme::fold(ui, title, default_open, open_anyway);
    }
    let id = crate::prefs::cards::before(ui, title);
    let shown = sculpt_toolkit::theme::fold(ui, title, default_open, open_anyway);
    crate::prefs::cards::after(ui, id, title, default_open);
    shown
}
