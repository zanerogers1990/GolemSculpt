//! The questions the app asks before something that could cost work: delete
//! this object, close with unsaved changes, open a file over the scene.
//!
//! One dialog serves them all (the Unity app's is `UIFactory.ShowModal`): the
//! window dims, the question is put in plain words, and each answer is a
//! button. The first answer is the one Enter gives, the last is Cancel, which
//! Esc gives too. `scene_graph::State` holds the question that is up; the app
//! sends it Enter and Esc and carries out the answer.

use std::path::PathBuf;

use egui::{Id, vec2};

use super::theme::{self, ButtonStyle};
use crate::doc::{Action, Doc, OpenHow};

#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// The Delete key: delete this object?
    Delete(u64),
    /// The app was asked to close with changes that have not been saved.
    Exit,
    /// A `.sclp` file was chosen: open it in place of the scene, or add its
    /// objects? `unsaved` says the scene has changes that are not saved.
    Open { path: PathBuf, unsaved: bool },
}

/// One answer: its button, and what it sets in motion. `None` only takes the
/// question down.
struct Choice {
    label: &'static str,
    /// A colour for the button that matters: blue for the safe answer, red
    /// for the one that gives something up.
    tint: Option<egui::Color32>,
    action: Option<Action>,
}

impl Choice {
    fn new(label: &'static str, tint: Option<egui::Color32>, action: Action) -> Choice {
        Choice { label, tint, action: Some(action) }
    }

    fn cancel() -> Choice {
        Choice { label: "Cancel", tint: None, action: None }
    }
}

/// A file's name without its folders, for a sentence.
fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

impl Question {
    /// The question in words, or `None` if it no longer applies (the object
    /// it asks about has gone).
    fn text(&self, doc: &Doc) -> Option<String> {
        Some(match self {
            Question::Delete(id) => {
                let object = doc.scene.get(*id)?;
                if doc.scene.users(object.mesh) > 1 {
                    format!("Delete \"{}\"? The mesh and its other copies stay. Undo (Z) restores it.", object.name)
                } else {
                    format!("Delete \"{}\"? Undo (Z) restores it.", object.name)
                }
            }
            Question::Exit => match &doc.path {
                Some(path) => format!("Close Sculpt? \"{}\" has changes that have not been saved.", file_name(path)),
                None => "Close Sculpt? This scene has not been saved.".to_owned(),
            },
            Question::Open { path, unsaved: true } => format!(
                "Open \"{}\"? The scene you have has changes that are not saved. Opening puts it aside: Undo (Z) brings its objects back, but only until the app is closed.",
                file_name(path)
            ),
            Question::Open { path, unsaved: false } => {
                format!("Open \"{}\" in place of the scene you have, or add its objects to it?", file_name(path))
            }
        })
    }

    /// The answers, the one Enter gives first and Cancel last.
    fn choices(&self) -> Vec<Choice> {
        let (safe, lossy) = (Some(theme::ACCENT), Some(theme::SUB));
        match self {
            Question::Delete(id) => vec![Choice::new("Delete", lossy, Action::DeleteObject(*id)), Choice::cancel()],
            Question::Exit => vec![
                Choice::new("Save and Exit", safe, Action::Quit { save: true }),
                Choice::new("Exit Without Saving", lossy, Action::Quit { save: false }),
                Choice::cancel(),
            ],
            Question::Open { path, unsaved } => {
                let open = |how| Action::OpenAs(path.clone(), how);
                let mut choices = if *unsaved {
                    vec![
                        Choice::new("Save, Then Open", safe, open(OpenHow::SaveThenReplace)),
                        Choice::new("Open Without Saving", lossy, open(OpenHow::Replace)),
                    ]
                } else {
                    vec![Choice::new("Open", safe, open(OpenHow::Replace))]
                };
                choices.push(Choice::new("Add Its Objects to This Scene", None, open(OpenHow::Add)));
                choices.push(Choice::cancel());
                choices
            }
        }
    }

    /// What Enter does: the first answer.
    pub fn default_action(&self) -> Option<Action> {
        self.choices().into_iter().next().and_then(|choice| choice.action)
    }

    /// Draws the question over everything else. Returns `None` while it
    /// waits; once it is answered (or no longer applies), what to carry out.
    pub fn show(&self, ctx: &egui::Context, doc: &Doc) -> Option<Option<Action>> {
        let Some(text) = self.text(doc) else { return Some(None) };
        let choices = self.choices();
        let frame = egui::Frame::new()
            .fill(theme::CARD)
            .corner_radius(10.0)
            .stroke(theme::hairline())
            .inner_margin(egui::Margin::same(16));
        let mut picked = None;
        egui::Modal::new(Id::new("question")).frame(frame).show(ctx, |ui| {
            ui.set_width(320.0);
            ui.spacing_mut().item_spacing = vec2(8.0, 12.0);
            ui.add(egui::Label::new(egui::RichText::new(text).font(theme::font(13.0)).color(theme::TEXT)).wrap());
            let last = choices.len() - 1;
            let button = |ui: &mut egui::Ui, width: f32, index: usize| {
                let badge = if index == 0 {
                    "Enter"
                } else if index == last {
                    "Esc"
                } else {
                    ""
                };
                let style = ButtonStyle { badge, active: choices[index].tint, ..Default::default() };
                theme::button(ui, width, choices[index].label, style).clicked()
            };
            if choices.len() == 2 {
                // Two answers sit side by side, Cancel on the left where the
                // hand is not already resting.
                ui.horizontal(|ui| {
                    let width = theme::row_width(ui, 2, 8.0);
                    for index in [1, 0] {
                        if button(ui, width, index) {
                            picked = Some(index);
                        }
                    }
                });
            } else {
                ui.spacing_mut().item_spacing.y = 6.0;
                for index in 0..choices.len() {
                    if button(ui, ui.available_width(), index) {
                        picked = Some(index);
                    }
                }
            }
        });
        picked.map(|index| choices.into_iter().nth(index).and_then(|choice| choice.action))
    }
}
