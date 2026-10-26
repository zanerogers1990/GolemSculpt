//! The Older Versions window (File tab): the earlier saves of the scene's
//! file, kept beside it (`crate::backups`), and the recovery copies kept for
//! a week (`crate::recovery::kept`). Each row adds its objects to the scene,
//! which is how one lost object is brought back into today's work, or opens
//! the whole of it in place of the scene. The app makes the list when the
//! window opens (`app/older.rs`); the window only shows it.

use std::path::PathBuf;

use egui::{Id, RichText, vec2};

use super::UiState;
use super::theme::{self, ButtonStyle};
use crate::doc::{Action, Doc};

/// One version in the list.
#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub file: PathBuf,
    /// "Saved today at 23:11".
    pub when: String,
    /// "154.9 MB", or what a recovery copy holds.
    pub what: String,
}

/// What the window shows.
#[derive(Clone, Debug, PartialEq)]
pub struct Listing {
    /// The scene's file, if it has one.
    pub document: Option<PathBuf>,
    /// Its earlier saves, newest first.
    pub saves: Vec<Row>,
    pub saves_folder: Option<PathBuf>,
    /// The recovery copies kept for a week, newest first.
    pub copies: Vec<Row>,
    pub copies_folder: Option<PathBuf>,
    /// Whether a version may take the scene's place: nothing unsaved, and
    /// nothing that is in no file.
    pub can_replace: bool,
}

#[derive(Default)]
pub struct State {
    /// The window is up, showing this.
    pub open: Option<Listing>,
}

/// The rows of one list, each with its two buttons.
fn rows(ui: &mut egui::Ui, rows: &[Row], can_replace: bool, tips: bool, actions: &mut Vec<Action>) {
    for row in rows {
        ui.horizontal(|ui| {
            let buttons = 2.0 * 92.0 + 6.0;
            ui.allocate_ui(vec2((ui.available_width() - buttons).max(80.0), 34.0), |ui| {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    ui.label(RichText::new(&row.when).font(theme::font(12.0)).color(theme::TEXT()));
                    ui.add(egui::Label::new(RichText::new(&row.what).font(theme::font(10.5)).color(theme::HINT())).truncate());
                });
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                let help = if can_replace {
                    "Open this version in place of the scene. It has no file of its own: Save asks where to keep it. Undo (Ctrl+Z) brings the scene back."
                } else {
                    "Save the scene first: it has changes that are not saved. Add Objects works now, and keeps the scene as it is."
                };
                let open = ui.add_enabled_ui(can_replace, |ui| theme::button(ui, 92.0, "Open", ButtonStyle::default())).inner;
                if super::tip(open, tips, help).clicked() {
                    actions.push(Action::OpenOlder { file: row.file.clone(), add: false });
                }
                let style = ButtonStyle { active: Some(theme::ACCENT()), ..Default::default() };
                let help = "Add this version's objects to the scene, beside what is there: the way to get back an object that was deleted. Nothing of the scene changes, and Undo (Ctrl+Z) takes them out again.";
                if super::tip(theme::button(ui, 92.0, "Add Objects", style), tips, help).clicked() {
                    actions.push(Action::OpenOlder { file: row.file.clone(), add: true });
                }
            });
        });
    }
}

/// A heading, with Show Folder beside it where there is a folder to show.
fn heading(ui: &mut egui::Ui, text: &str, folder: Option<&PathBuf>, actions: &mut Vec<Action>) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(text).font(theme::font(13.0)).strong().color(theme::TEXT()));
        if let Some(folder) = folder {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.small_button("Show Folder").on_hover_text(folder.display().to_string()).clicked() {
                    actions.push(Action::ShowFolder(folder.clone()));
                }
            });
        }
    });
}

/// Draws the window while it is up. Close, Esc or a click outside it takes
/// it down; so does choosing a version.
pub fn show(ctx: &egui::Context, doc: &Doc, state: &mut UiState, actions: &mut Vec<Action>) {
    let Some(listing) = &state.older.open else { return };
    let tips = doc.opts.tooltips;
    let frame = egui::Frame::new()
        .fill(theme::CARD())
        .corner_radius(10.0)
        .stroke(theme::hairline())
        .inner_margin(egui::Margin::same(16));
    let before = actions.len();
    let mut close = false;
    let modal = egui::Modal::new(Id::new("older_versions")).frame(frame).show(ctx, |ui| {
        ui.set_width(440.0);
        ui.spacing_mut().item_spacing = vec2(8.0, 8.0);
        ui.label(RichText::new("Older Versions").font(theme::font(15.0)).strong().color(theme::TEXT()));
        egui::ScrollArea::vertical().max_height(ctx.content_rect().height() * 0.6).show(ui, |ui| {
            let name = listing.document.as_ref().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned());
            match &name {
                Some(name) => heading(ui, &format!("Earlier saves of {name}"), listing.saves_folder.as_ref(), actions),
                None => heading(ui, "Earlier saves", None, actions),
            }
            if listing.saves.is_empty() {
                let text = match name {
                    Some(_) => "None yet. From now on each save keeps the one before it, in a folder called GolemSculpt backups beside the file: the last 3 saves, and a few older ones going back a week and more.",
                    None => "This scene has no file yet. Once it is saved, each save keeps the one before it.",
                };
                theme::hint(ui, text);
            } else {
                rows(ui, &listing.saves, listing.can_replace, tips, actions);
            }
            ui.add_space(6.0);
            heading(ui, "Recovery copies from the past week", listing.copies_folder.as_ref(), actions);
            if listing.copies.is_empty() {
                theme::hint(ui, "None yet. Each time GolemSculpt closes, or saves, its newest recovery copy is kept here for a week, even after Exit Without Saving.");
            } else {
                rows(ui, &listing.copies, listing.can_replace, tips, actions);
            }
        });
        if !listing.can_replace {
            theme::hint(ui, "The scene has changes that are not saved, so only Add Objects is offered: it leaves the scene as it is.");
        }
        let style = ButtonStyle { badge: "Esc", ..Default::default() };
        close = theme::button(ui, ui.available_width(), "Close", style).clicked();
    });
    let chosen = actions[before..].iter().any(|action| matches!(action, Action::OpenOlder { .. }));
    if close || chosen || modal.should_close() {
        state.older.open = None;
    }
}
