//! What an undo step is called: Undo's toast says what it took back ("Undo
//! Remesh"), and the History list (the … beside Undo and Redo) names every
//! step. The history gives each step the name the app has given it while it
//! was recorded (`History::name_next`), and a command names the steps it
//! records by what it is: [`of`] makes that name from the command itself,
//! so a command added to `Action` is named without a word here. A stroke is
//! named by its brush, a gizmo drag by its handle, a hide key by what it hid.
//!
//! Steps recorded later, by work that finishes in the background or inside
//! a tool, are called by what kind of change they are ("Rebuild",
//! "Objects").

use crate::doc::Action;

/// "QuadRemesh" as "Quad Remesh": the words of a name written in one.
fn spaced(joined: &str) -> String {
    let mut out = String::new();
    for (i, c) in joined.chars().enumerate() {
        if i > 0 && c.is_ascii_uppercase() {
            out.push(' ');
        }
        out.push(c);
    }
    out
}

/// The name a command gives the steps it records, or `None` for one that
/// records none of its own (Undo and Redo themselves).
pub(super) fn of(action: &Action) -> Option<String> {
    let said = match action {
        Action::Undo | Action::Redo | Action::UndoSteps(_) | Action::RedoSteps(_) => return None,
        Action::Boolean { op, .. } => format!("{op:?}"),
        Action::Mask(op) => format!("Mask {}", spaced(&format!("{op:?}"))),
        Action::New(kind) => format!("Add {}", spaced(&format!("{kind:?}"))),
        Action::Join { remesh: true } => "Join and Remesh".to_owned(),
        // The rest by the command's own name: `Remesh`, `QuadRemesh(..)`, `CutAndMirror { .. }`.
        other => {
            let debug = format!("{other:?}");
            let name: String = debug.chars().take_while(char::is_ascii_alphanumeric).collect();
            spaced(&name)
        }
    };
    Some(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_names_its_steps_in_words() {
        assert_eq!(of(&Action::Remesh).as_deref(), Some("Remesh"));
        assert_eq!(of(&Action::QuadRemesh).as_deref(), Some("Quad Remesh"));
        assert_eq!(of(&Action::Join { remesh: false }).as_deref(), Some("Join"));
        assert_eq!(of(&Action::Join { remesh: true }).as_deref(), Some("Join and Remesh"));
        assert_eq!(of(&Action::Undo), None, "Undo records nothing of its own");
        assert_eq!(spaced("CutAndMirror"), "Cut And Mirror");
    }
}
