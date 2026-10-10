//! The questions the app asks before something that could cost work: delete
//! this object, close with unsaved changes, open a file over the scene, and
//! at start-up, whether to bring back the copy a crash left behind.
//!
//! One dialog serves them all (the Unity app's is `UIFactory.ShowModal`): the
//! window dims, the question is put in plain words, and each answer is a
//! button. The first answer is the one Enter gives, the last is Cancel, which
//! Esc gives too. `scene_graph::State` holds the question that is up; the app
//! sends it Enter and Esc and carries out the answer.

use std::path::PathBuf;

use egui::{Id, vec2};

use super::theme::{self, ButtonStyle};
use crate::doc::{Action, Doc, OpenHow, Recover};

#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// The Delete key: delete this object?
    Delete(u64),
    /// The app was asked to close with changes that have not been saved.
    Exit,
    /// A `.sclp` file was chosen: open it in place of the scene, or add its
    /// objects? `unsaved` says the scene has changes that are not saved.
    Open { path: PathBuf, unsaved: bool },
    /// At start-up: the app did not close properly last time and a copy of
    /// the work was kept. `when` is when ("today at 14:35"), `what` what it
    /// holds; `stopped` says it was made as the app stopped on a fault, and
    /// `earlier` is when the last regular copy before it was made, if there
    /// is one to offer instead. (See app/recovery.rs.)
    Recover { when: Option<String>, what: String, stopped: bool, earlier: Option<String> },
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

    /// Cancel for a question that will be asked again: nothing is given up.
    fn not_now() -> Choice {
        Choice { label: "Not Now", tint: None, action: None }
    }
}

/// A file's name without its folders, for a sentence.
fn file_name(path: &std::path::Path) -> String {
    path.file_name().map_or_else(|| path.display().to_string(), |n| n.to_string_lossy().into_owned())
}

/// The question about a copy a crash left behind (`Question::Recover`).
fn recover_words(when: Option<&str>, what: &str, stopped: bool, earlier: Option<&str>) -> String {
    let when = when.map_or(String::new(), |when| format!(" {when}"));
    let kept = if stopped {
        let earlier = earlier.map_or(String::new(), |at| format!(" There is also an earlier copy, from {at}."));
        format!(
            "It kept a copy of your work as it stopped,{when} ({what}). \
             The very last change may be incomplete in it.{earlier}"
        )
    } else {
        format!("It kept a copy of your work,{when} ({what}).")
    };
    format!(
        "GolemSculpt did not close properly last time. {kept} Restore it? \
         Not Now keeps the copy and asks again next time."
    )
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
            Question::Recover { when, what, stopped, earlier } => {
                recover_words(when.as_deref(), what, *stopped, earlier.as_deref())
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
            Question::Recover { stopped, earlier, .. } => {
                let mut choices = vec![Choice::new("Restore It", safe, Action::Recover(Recover::Newest))];
                if *stopped && earlier.is_some() {
                    let earlier = Action::Recover(Recover::Earlier);
                    choices.push(Choice::new("Restore the Earlier Copy Instead", None, earlier));
                }
                choices.push(Choice::new("Discard It", lossy, Action::Recover(Recover::Discard)));
                choices.push(Choice::not_now());
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

#[cfg(test)]
mod tests {
    use super::*;

    fn labels(question: &Question) -> Vec<&'static str> {
        question.choices().iter().map(|choice| choice.label).collect()
    }

    const WHEN: &str = "today at 14:35";
    const WHAT: &str = "\"dragon head.sclp\", 3 objects, 2.1 million triangles";

    fn recover(stopped: bool, earlier: Option<&str>) -> Question {
        Question::Recover {
            when: Some(WHEN.to_owned()),
            what: WHAT.to_owned(),
            stopped,
            earlier: earlier.map(str::to_owned),
        }
    }

    #[test]
    fn the_question_about_a_copy_restores_on_enter_and_gives_nothing_up_on_esc() {
        let question = recover(false, None);
        assert_eq!(labels(&question), ["Restore It", "Discard It", "Not Now"]);
        assert_eq!(question.default_action(), Some(Action::Recover(Recover::Newest)), "Enter restores");
        let last = question.choices().pop().unwrap();
        assert!(last.action.is_none(), "Esc is Not Now: it neither restores nor discards");
        let text = recover_words(Some(WHEN), WHAT, false, None);
        assert!(text.contains("did not close properly") && text.contains("today at 14:35"));
        assert!(text.contains("dragon head.sclp") && !text.contains("incomplete"));
    }

    #[test]
    fn a_copy_made_as_the_app_stopped_says_so_and_offers_the_one_before_it() {
        let question = recover(true, Some("today at 14:32"));
        assert_eq!(labels(&question), ["Restore It", "Restore the Earlier Copy Instead", "Discard It", "Not Now"]);
        let text = recover_words(Some(WHEN), WHAT, true, Some("today at 14:32"));
        assert!(text.contains("as it stopped") && text.contains("may be incomplete"));
        assert!(text.contains("earlier copy, from today at 14:32"));
        // With no regular copy before it there is nothing else to offer.
        let alone = recover(true, None);
        assert_eq!(labels(&alone), ["Restore It", "Discard It", "Not Now"]);
        assert!(!recover_words(None, WHAT, true, None).contains("earlier copy"));
    }
}
