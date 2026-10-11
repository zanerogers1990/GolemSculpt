//! The questions the app asks before something that could cost work: delete
//! this object, close with unsaved changes, open a file over the scene,
//! export for printing a model that may not print, and at start-up, whether
//! to bring back the copy a crash left behind.
//!
//! One dialog serves them all (the Unity app's is `UIFactory.ShowModal`): the
//! window dims, the question is put in plain words, and each answer is a
//! button. The first answer is the one Enter gives, the last is Cancel, which
//! Esc gives too. `scene_graph::State` holds the question that is up; the app
//! sends it Enter and Esc and carries out the answer.
//!
//! Closing and opening can also cost work that is in no file at all: SSpheres,
//! unmerged tubes, planned keys, a lathe outline (`lost`, from
//! app/unsaved.rs). A question that names any is asked even when the scene
//! itself is saved, says that saving does not keep them, and gives Cancel to
//! Enter as well as to Esc: no key pressed in passing gives them up.

use std::path::PathBuf;

use egui::{Id, vec2};

use super::theme::{self, ButtonStyle};
use crate::doc::{Action, Doc, OpenHow, Recover};

#[derive(Clone, Debug, PartialEq)]
pub enum Question {
    /// The Delete key: delete this object?
    Delete(u64),
    /// The app was asked to close. `unsaved` says the scene has changes that
    /// have not been saved; `lost` names the work that is in no file and
    /// would go with the app, saved or not. At least one of them holds.
    Exit { unsaved: bool, lost: Vec<String> },
    /// A `.sclp` file was chosen: open it in place of the scene, or add its
    /// objects? `unsaved` says the scene has changes that are not saved;
    /// `lost` names the work in no file that replacing the scene would cost.
    Open { path: PathBuf, unsaved: bool, lost: Vec<String> },
    /// At start-up: the app did not close properly last time and a copy of
    /// the work was kept. `when` is when ("today at 14:35"), `what` what it
    /// holds; `stopped` says it was made as the app stopped on a fault, and
    /// `earlier` is when the last regular copy before it was made, if there
    /// is one to offer instead. `others` is how many other times the app did
    /// not close properly and left copies: this question is about the latest
    /// only, and they are asked about at later starts. (See app/recovery.rs.)
    Recover { when: Option<String>, what: String, stopped: bool, earlier: Option<String>, others: usize },
    /// Export STL for Printing was pressed on an object that is not a closed
    /// surface. `text` is the question, with what is wrong (`printable.rs`).
    ExportNotClosed { object: u64, text: String },
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

    /// Cancel where it is the answer Enter gives: lit as the safe one.
    fn safe_cancel() -> Choice {
        Choice { label: "Cancel", tint: Some(theme::ACCENT), action: None }
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

/// The things a question says would be lost, as they read in a sentence:
/// "3 unmerged tubes (Merge or Keep Separate first) and an SSphere rig of 12
/// spheres (Make Skin first)".
fn listed(lost: &[String]) -> String {
    match lost {
        [] => String::new(),
        [one] => one.clone(),
        [most @ .., last] => format!("{} and {last}", most.join(", ")),
    }
}

/// The question about closing (`Question::Exit`). `file` is the scene's file,
/// if it has one.
fn exit_words(file: Option<&std::path::Path>, unsaved: bool, lost: &[String]) -> String {
    let changes = match file {
        Some(path) => format!("\"{}\" has changes that have not been saved.", file_name(path)),
        None => "This scene has not been saved.".to_owned(),
    };
    match (unsaved, lost.is_empty()) {
        (_, true) => format!("Close GolemSculpt? {changes}"),
        (true, false) => format!(
            "Close GolemSculpt? {changes}\n\nSaving does not keep these, which will be lost either way: {}.",
            listed(lost)
        ),
        (false, false) => {
            format!("Close GolemSculpt? These are not kept in a file and will be lost: {}.", listed(lost))
        }
    }
}

/// The question about opening a file (`Question::Open`).
fn open_words(path: &std::path::Path, unsaved: bool, lost: &[String]) -> String {
    let name = file_name(path);
    let aside = "The scene you have has changes that are not saved. Opening puts it aside: \
                 Undo (Z) brings its objects back, but only until the app is closed.";
    match (unsaved, lost.is_empty()) {
        (true, true) => format!("Open \"{name}\"? {aside}"),
        (false, true) => format!("Open \"{name}\" in place of the scene you have, or add its objects to it?"),
        (true, false) => format!(
            "Open \"{name}\"? {aside}\n\nSaving does not keep these, and opening loses them either way: {}. \
             Adding the file's objects to this scene keeps them.",
            listed(lost)
        ),
        (false, false) => format!(
            "Open \"{name}\"? Opening it in place of the scene you have loses these, \
             which are not kept in a file: {}. Adding its objects to this scene keeps them.",
            listed(lost)
        ),
    }
}

/// The question about a copy a crash left behind (`Question::Recover`).
/// With copies from more than one such time, it says which this is about and
/// that the answer leaves the rest alone: each is asked about by itself.
fn recover_words(when: Option<&str>, what: &str, stopped: bool, earlier: Option<&str>, others: usize) -> String {
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
    let ask = "Restore it? Not Now keeps the copy and asks again next time.";
    let rest = match others {
        0 => return format!("GolemSculpt did not close properly last time. {kept} {ask}"),
        1 => "The copy from the other time is kept, and GolemSculpt will ask about it at a later start, \
              once this one has been saved or discarded."
            .to_owned(),
        n => format!(
            "The copies from the other {n} times are kept, and GolemSculpt will ask about each at a later \
             start, once this one has been saved or discarded."
        ),
    };
    format!(
        "GolemSculpt did not close properly {} times, and kept a copy of your work each time. \
         This question is about the latest. {kept} {ask}\n\nYour answer is about this copy only. {rest}",
        others + 1
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
            Question::Exit { unsaved, lost } => exit_words(doc.path.as_deref(), *unsaved, lost),
            Question::Open { path, unsaved, lost } => open_words(path, *unsaved, lost),
            Question::Recover { when, what, stopped, earlier, others } => {
                recover_words(when.as_deref(), what, *stopped, earlier.as_deref(), *others)
            }
            // It was asked of the selected object, and is about no other.
            Question::ExportNotClosed { object, text } => {
                (doc.scene.active_id() == *object).then(|| text.clone())?
            }
        })
    }

    /// Whether the question names work that is in no file. Every answer but
    /// Cancel (and adding a file's objects) then gives that work up, so Enter
    /// is Cancel and no answer is lit as the safe one but Cancel.
    fn names_lost_work(&self) -> bool {
        match self {
            Question::Exit { lost, .. } | Question::Open { lost, .. } => !lost.is_empty(),
            Question::Delete(_) | Question::Recover { .. } | Question::ExportNotClosed { .. } => false,
        }
    }

    /// The answers. Cancel is last. Enter gives the first, unless the
    /// question names work that would be lost: then it gives Cancel.
    fn choices(&self) -> Vec<Choice> {
        let (safe, lossy) = (Some(theme::ACCENT), Some(theme::SUB));
        // Saving first is the safe answer only while it saves everything.
        let saving = safe.filter(|_| !self.names_lost_work());
        let cancel = if self.names_lost_work() { Choice::safe_cancel() } else { Choice::cancel() };
        match self {
            Question::Delete(id) => vec![Choice::new("Delete", lossy, Action::DeleteObject(*id)), cancel],
            Question::Exit { unsaved: false, .. } => {
                vec![Choice::new("Exit and Lose Them", lossy, Action::Quit { save: false }), cancel]
            }
            Question::Exit { unsaved: true, lost } => {
                let save = if lost.is_empty() { "Save and Exit" } else { "Save the Scene and Exit" };
                vec![
                    Choice::new(save, saving, Action::Quit { save: true }),
                    Choice::new("Exit Without Saving", lossy, Action::Quit { save: false }),
                    cancel,
                ]
            }
            Question::Open { path, unsaved, lost } => {
                let open = |how| Action::OpenAs(path.clone(), how);
                let mut choices = match (*unsaved, lost.is_empty()) {
                    (true, kept) => vec![
                        Choice::new(
                            if kept { "Save, Then Open" } else { "Save the Scene, Then Open" },
                            saving,
                            open(OpenHow::SaveThenReplace),
                        ),
                        Choice::new("Open Without Saving", lossy, open(OpenHow::Replace)),
                    ],
                    (false, true) => vec![Choice::new("Open", safe, open(OpenHow::Replace))],
                    (false, false) => vec![Choice::new("Open and Lose Them", lossy, open(OpenHow::Replace))],
                };
                choices.push(Choice::new("Add Its Objects to This Scene", None, open(OpenHow::Add)));
                choices.push(cancel);
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
            Question::ExportNotClosed { .. } => {
                vec![Choice::new("Export Anyway", lossy, Action::ExportStlAnyway), cancel]
            }
        }
    }

    /// Which answer Enter gives: the first, or Cancel (the last) where the
    /// first would give up work that is in no file.
    fn enter(&self, choices: usize) -> usize {
        if self.names_lost_work() { choices.saturating_sub(1) } else { 0 }
    }

    /// What Enter does: `None` where it is Cancel.
    pub fn default_action(&self) -> Option<Action> {
        let choices = self.choices();
        let enter = self.enter(choices.len());
        choices.into_iter().nth(enter).and_then(|choice| choice.action)
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
            let (last, enter) = (choices.len() - 1, self.enter(choices.len()));
            let button = |ui: &mut egui::Ui, width: f32, index: usize| {
                let badge = match (index == enter, index == last) {
                    (true, true) => "Enter / Esc",
                    (true, false) => "Enter",
                    (false, true) => "Esc",
                    (false, false) => "",
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
            others: 0,
        }
    }

    fn exit(unsaved: bool, lost: &[&str]) -> Question {
        Question::Exit { unsaved, lost: lost.iter().map(|&what| what.to_owned()).collect() }
    }

    fn open(unsaved: bool, lost: &[&str]) -> Question {
        let lost = lost.iter().map(|&what| what.to_owned()).collect();
        Question::Open { path: PathBuf::from("folder/dragon head.sclp"), unsaved, lost }
    }

    const TUBES: &str = "3 unmerged tubes (Merge or Keep Separate first)";
    const RIG: &str = "an SSphere rig of 12 spheres (Make Skin first)";
    const KEYS: &str = "4 planned keys (Build Keys first)";

    /// Whether Enter, pressed on this question, changes anything.
    fn enter_is_cancel(question: &Question) -> bool {
        question.default_action().is_none()
    }

    #[test]
    fn closing_over_unsaved_changes_alone_asks_as_it_always_did_under_the_apps_name() {
        let question = exit(true, &[]);
        assert_eq!(labels(&question), ["Save and Exit", "Exit Without Saving", "Cancel"]);
        assert_eq!(question.default_action(), Some(Action::Quit { save: true }), "Enter saves and exits");
        let file = PathBuf::from("folder/dragon head.sclp");
        let named = "Close GolemSculpt? \"dragon head.sclp\" has changes that have not been saved.";
        assert_eq!(exit_words(Some(&file), true, &[]), named);
        assert_eq!(exit_words(None, true, &[]), "Close GolemSculpt? This scene has not been saved.");
    }

    #[test]
    fn closing_over_work_that_is_in_no_file_names_it_and_enter_does_not_give_it_up() {
        // The scene is saved: there is nothing to save, only something to lose.
        let question = exit(false, &[RIG, TUBES]);
        assert_eq!(labels(&question), ["Exit and Lose Them", "Cancel"]);
        assert!(enter_is_cancel(&question), "Enter is Cancel");
        let text = exit_words(None, false, &[RIG.to_owned(), TUBES.to_owned()]);
        assert_eq!(
            text,
            "Close GolemSculpt? These are not kept in a file and will be lost: an SSphere rig of 12 spheres \
             (Make Skin first) and 3 unmerged tubes (Merge or Keep Separate first)."
        );
        assert!(!text.contains("not been saved"), "the scene is saved, and the question does not say otherwise");
    }

    #[test]
    fn with_both_the_question_says_saving_does_not_keep_the_scaffolds() {
        let question = exit(true, &[TUBES]);
        assert_eq!(labels(&question), ["Save the Scene and Exit", "Exit Without Saving", "Cancel"]);
        assert!(enter_is_cancel(&question), "Save and Exit would still lose the tubes: Enter is Cancel");
        let saves = question.choices().into_iter().next().unwrap();
        assert!(saves.tint.is_none(), "saving is not lit as the safe answer when it does not save everything");
        assert_eq!(question.choices().pop().unwrap().tint, Some(theme::ACCENT), "Cancel is");
        let file = PathBuf::from("dragon head.sclp");
        let text = exit_words(Some(&file), true, &[TUBES.to_owned()]);
        assert!(text.starts_with("Close GolemSculpt? \"dragon head.sclp\" has changes that have not been saved."));
        assert!(text.ends_with(
            "Saving does not keep these, which will be lost either way: \
             3 unmerged tubes (Merge or Keep Separate first)."
        ));
    }

    #[test]
    fn opening_over_work_that_is_in_no_file_names_it_and_says_adding_keeps_it() {
        let question = open(false, &[TUBES, KEYS]);
        assert_eq!(labels(&question), ["Open and Lose Them", "Add Its Objects to This Scene", "Cancel"]);
        assert!(enter_is_cancel(&question));
        let path = PathBuf::from("folder/dragon head.sclp");
        let text = open_words(&path, false, &[TUBES.to_owned(), KEYS.to_owned()]);
        assert!(text.starts_with("Open \"dragon head.sclp\"? Opening it in place of the scene you have loses these"));
        assert!(text.contains("3 unmerged tubes (Merge or Keep Separate first) and 4 planned keys (Build Keys first)."));
        assert!(text.ends_with("Adding its objects to this scene keeps them."));

        let both = open(true, &[TUBES]);
        let answers = ["Save the Scene, Then Open", "Open Without Saving", "Add Its Objects to This Scene", "Cancel"];
        assert_eq!(labels(&both), answers);
        assert!(enter_is_cancel(&both));
        let text = open_words(&path, true, &[TUBES.to_owned()]);
        assert!(text.contains("changes that are not saved") && text.contains("Saving does not keep these"));
    }

    #[test]
    fn opening_with_nothing_of_that_kind_asks_as_it_always_did() {
        let clean = open(false, &[]);
        assert_eq!(labels(&clean), ["Open", "Add Its Objects to This Scene", "Cancel"]);
        assert!(matches!(clean.default_action(), Some(Action::OpenAs(_, OpenHow::Replace))));
        let unsaved = open(true, &[]);
        let answers = ["Save, Then Open", "Open Without Saving", "Add Its Objects to This Scene", "Cancel"];
        assert_eq!(labels(&unsaved), answers);
        assert!(matches!(unsaved.default_action(), Some(Action::OpenAs(_, OpenHow::SaveThenReplace))));
    }

    #[test]
    fn a_list_reads_as_a_sentence_does() {
        let of = |items: &[&str]| listed(&items.iter().map(|&item| item.to_owned()).collect::<Vec<_>>());
        assert_eq!(of(&[]), "");
        assert_eq!(of(&["one"]), "one");
        assert_eq!(of(&["one", "two"]), "one and two");
        assert_eq!(of(&["one", "two", "three"]), "one, two and three");
    }

    #[test]
    fn the_question_about_a_copy_restores_on_enter_and_gives_nothing_up_on_esc() {
        let question = recover(false, None);
        assert_eq!(labels(&question), ["Restore It", "Discard It", "Not Now"]);
        assert_eq!(question.default_action(), Some(Action::Recover(Recover::Newest)), "Enter restores");
        let last = question.choices().pop().unwrap();
        assert!(last.action.is_none(), "Esc is Not Now: it neither restores nor discards");
        let text = recover_words(Some(WHEN), WHAT, false, None, 0);
        assert!(text.contains("did not close properly") && text.contains("today at 14:35"));
        assert!(text.contains("dragon head.sclp") && !text.contains("incomplete"));
    }

    #[test]
    fn a_copy_made_as_the_app_stopped_says_so_and_offers_the_one_before_it() {
        let question = recover(true, Some("today at 14:32"));
        assert_eq!(labels(&question), ["Restore It", "Restore the Earlier Copy Instead", "Discard It", "Not Now"]);
        let text = recover_words(Some(WHEN), WHAT, true, Some("today at 14:32"), 0);
        assert!(text.contains("as it stopped") && text.contains("may be incomplete"));
        assert!(text.contains("earlier copy, from today at 14:32"));
        // With no regular copy before it there is nothing else to offer.
        let alone = recover(true, None);
        assert_eq!(labels(&alone), ["Restore It", "Discard It", "Not Now"]);
        assert!(!recover_words(None, WHAT, true, None, 0).contains("earlier copy"));
    }

    /// Review point 3: with copies from two runs the question was about the
    /// newest copy of them all, and said nothing of the rest.
    #[test]
    fn with_copies_from_more_than_one_time_the_question_says_which_it_is_about_and_that_the_rest_stay() {
        let one = recover_words(Some(WHEN), WHAT, false, None, 0);
        assert!(one.starts_with("GolemSculpt did not close properly last time. It kept a copy of your work, today"));
        assert!(!one.contains("other time") && !one.contains("this copy only"));

        let two = recover_words(Some(WHEN), WHAT, false, None, 1);
        assert!(two.starts_with("GolemSculpt did not close properly 2 times, and kept a copy of your work each time."));
        assert!(two.contains("This question is about the latest. It kept a copy of your work, today at 14:35"));
        assert!(two.contains("Your answer is about this copy only. The copy from the other time is kept"));
        assert!(two.ends_with("ask about it at a later start, once this one has been saved or discarded."));

        let four = recover_words(Some(WHEN), WHAT, true, Some("today at 14:32"), 3);
        assert!(four.contains("did not close properly 4 times") && four.contains("The copies from the other 3 times"));
        assert!(four.contains("may be incomplete") && four.contains("earlier copy, from today at 14:32"));
        // The answers are the same: they are about the one copy.
        let question = Question::Recover { when: None, what: WHAT.into(), stopped: false, earlier: None, others: 2 };
        assert_eq!(labels(&question), ["Restore It", "Discard It", "Not Now"]);
    }
}
