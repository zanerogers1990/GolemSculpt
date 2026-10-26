//! The Settings tab's Keyboard Shortcuts card: every command a key can be
//! put on, with the key it is on now, changed by clicking the key and
//! pressing another (keymap.rs, app/keys.rs); and, below, what the pen,
//! mouse and the keys kept for the app do, which is not for changing.
//!
//! A test holds every key the app ships with, and every fixed one, to the
//! README's Controls table, so a key added to one and not the other fails the
//! build's tests instead of going stale unseen.

use egui::{Color32, Id};

use super::theme::{self, ACCENT, ButtonStyle};
use crate::doc::Doc;
use crate::keymap::{self, Chord, Keymap, Listed};

/// What the pen, mouse and keys held do, and the keys kept for the app, a
/// heading to each group: what you press, and what it does. A key's words
/// are the README's own (its Controls table), so the test below can find
/// them there.
pub const FIXED: &[(&str, &[(&str, &str)])] = &[
    ("Sculpting", &[
        ("Left drag on the model", "Sculpt"),
        ("Ctrl + drag, or right drag", "Invert the brush"),
        ("Shift + drag", "Smooth"),
        ("Ctrl + Shift + click", "Isolate the face group"),
    ]),
    ("Brush", &[
        ("[ / ]", "Brush smaller / bigger"),
        ("Shift + [ / ]", "Strength down / up"),
        ("Scroll over the model", "Brush size"),
        ("Space (hold)", "Brush wheel"),
    ]),
    ("Tools", &[("Enter", "Finish: fuse, accept, build"), ("Esc", "Cancel, or put the tool away")]),
    ("View", &[
        ("Left drag on the background", "Orbit"),
        ("Alt + drag", "Orbit"),
        ("Middle drag", "Pan"),
        ("Alt + Shift + drag", "Pan"),
        ("Ctrl + Alt + drag", "Zoom"),
        ("Scroll off the model", "Zoom"),
        ("Double-click background", "Frame the selection"),
        ("O", "Spin the view (turntable)"),
        ("Tab", "Clean view: hide the panels"),
        ("Ctrl + = / Ctrl + -", "Interface size"),
    ]),
    ("Objects", &[("Double-click another object", "Select it"), ("Ctrl + click a row", "Add to the selection")]),
    ("File", &[("Ctrl + Q", "Quit (asks first)")]),
];

/// The colour of the card's title.
pub const COLOR: Color32 = Color32::from_rgb(230, 204, 102);

/// How long the second press of Reset All has before the button forgets the first.
const RESET_ALL_SECONDS: f64 = 4.0;

/// The keys a command has, for its button: "Ctrl + Y, Ctrl + Shift + Z".
fn keys_text(keys: &[Chord]) -> String {
    if keys.is_empty() { "\u{2014}".to_owned() } else { keys.iter().map(Chord::text).collect::<Vec<_>>().join(", ") }
}

/// Whether command `c` is listed while `find` is typed: its name or its key
/// has it. With nothing typed every command is, but of the brushes only those
/// that have a key: the library is long, and a brush is found by name.
fn listed(c: &Listed, keys: &Keymap, find: &str) -> bool {
    let find = find.trim().to_lowercase();
    if find.is_empty() {
        return c.group != "Brushes" || !keys.keys(&c.id).is_empty();
    }
    c.name.to_lowercase().contains(&find) || keys_text(&keys.keys(&c.id)).to_lowercase().contains(&find)
}

/// The card. `capture` is the command waiting for its new key (UiState).
pub fn card(ui: &mut egui::Ui, doc: &mut Doc, capture: &mut Option<String>) {
    crate::app::keys::refresh(doc);
    let tips = doc.opts.tooltips;
    let changed = doc.opts.keys.changes().len();
    let summary = match changed {
        0 => String::new(),
        1 => "1 changed".to_owned(),
        n => format!("{n} changed"),
    };
    theme::category(ui, "Keyboard Shortcuts", COLOR, &summary, false, |ui| {
        theme::hint(
            ui,
            "Click a key to change it, then press the new one, with Ctrl, Shift or Alt if you like. Esc leaves it as \
             it was; Backspace takes the key away. A key another command had is taken from it.",
        );
        let find_id = Id::new("shortcut_find");
        let mut find = ui.data(|d| d.get_temp::<String>(find_id)).unwrap_or_default();
        let field = egui::TextEdit::singleline(&mut find)
            .hint_text("Find a command or a brush")
            .font(theme::font(12.0))
            .desired_width(ui.available_width());
        let field = ui.add(field);
        if field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            find.clear();
        }
        ui.data_mut(|d| d.insert_temp(find_id, find.clone()));

        let commands: Vec<Listed> = doc.opts.keys.commands().to_vec();
        for group in keymap::GROUPS {
            let rows: Vec<&Listed> =
                commands.iter().filter(|c| c.group == *group && listed(c, &doc.opts.keys, &find)).collect();
            if rows.is_empty() && !(*group == "Brushes" && find.trim().is_empty()) {
                continue;
            }
            theme::heading(ui, group);
            if *group == "Brushes" && find.trim().is_empty() {
                theme::hint(ui, "Type a brush's name above to give it a key.");
            }
            for c in rows {
                row(ui, &mut doc.opts.keys, c, capture, tips);
            }
        }
        if changed > 0 {
            ui.add_space(6.0);
            reset_all(ui, &mut doc.opts.keys, tips);
        }
        ui.add_space(8.0);
        theme::heading(ui, "Not for changing");
        theme::hint(ui, "What the pen and mouse do, and the keys kept for the app.");
        fixed(ui);
    });
}

/// One command: its name, its keys (click to change), and a way back to how
/// it ships if it was changed.
fn row(ui: &mut egui::Ui, keys: &mut Keymap, c: &Listed, capture: &mut Option<String>, tips: bool) {
    let waiting = capture.as_deref() == Some(c.id.as_str());
    let shipped = keys.is_shipped(&c.id);
    ui.horizontal(|ui| {
        let reset_w = if shipped { 0.0 } else { 24.0 };
        let key_w = 118.0f32.min(ui.available_width() * 0.5);
        let name_w = (ui.available_width() - key_w - reset_w - 8.0).max(40.0);
        // The name to the left, wrapping onto a second line if it is long.
        let name = egui::Label::new(egui::RichText::new(&c.name).font(theme::font(11.0)).color(theme::TEXT())).wrap();
        let layout = egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true);
        ui.allocate_ui_with_layout(egui::vec2(name_w, 22.0), layout, |ui| {
            ui.set_min_size(egui::vec2(name_w, 22.0));
            ui.add(name);
        });
        let text = if waiting { "Press a key\u{2026}".to_owned() } else { keys_text(&keys.keys(&c.id)) };
        let style = ButtonStyle { height: 22.0, active: waiting.then_some(ACCENT()), ..Default::default() };
        let button = theme::button(ui, key_w, &text, style);
        let help = format!("{}: click, then press the key to put it on. Esc leaves it as it is.", c.name);
        if theme::tip(button, tips, &help).clicked() {
            *capture = if waiting { None } else { Some(c.id.clone()) };
        }
        if !shipped {
            let back = format!("Back to {}, as the app ships it.", keys_text(&c.shipped));
            let reset = theme::button(ui, 22.0, "\u{21ba}", ButtonStyle { height: 22.0, ..Default::default() });
            if theme::tip(reset, tips, &back).clicked() {
                keys.reset(&c.id);
            }
        }
    });
}

/// Reset All, which asks to be pressed a second time: the changes are not an
/// undo step, and a lot of them may go at once.
fn reset_all(ui: &mut egui::Ui, keys: &mut Keymap, tips: bool) {
    let id = Id::new("shortcuts_reset_all");
    let now = ui.input(|i| i.time);
    let armed = ui.data(|d| d.get_temp::<f64>(id)).is_some_and(|at| now - at < RESET_ALL_SECONDS);
    let text = if armed { "Click Again to Reset Every Key" } else { "Reset All Shortcuts" };
    let style = ButtonStyle { active: armed.then_some(ACCENT()), ..Default::default() };
    let button = theme::button(ui, ui.available_width(), text, style);
    let help = "Puts every command back on the key the app ships with. Press twice: it cannot be undone.";
    if theme::tip(button, tips, help).clicked() {
        if armed {
            keys.reset_all();
            ui.data_mut(|d| d.remove::<f64>(id));
        } else {
            ui.data_mut(|d| d.insert_temp(id, now));
        }
    }
    if armed {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(250));
    }
}

/// The fixed shortcuts. The keys are the README's words, and some are long
/// ("Left drag on the background"), so each column wraps rather than run into
/// the other.
fn fixed(ui: &mut egui::Ui) {
    let width = ui.available_width();
    // The keys' column as wide as the widest key needs, but never more than
    // about half the card: a longer key wraps onto a second line.
    let widest = FIXED
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(key, _)| ui.painter().layout_no_wrap((*key).to_owned(), theme::font(11.0), theme::KEY).size().x)
        .fold(0.0f32, f32::max);
    let keys = (widest + 10.0).min(width * 0.46);
    for (heading, rows) in FIXED {
        theme::heading(ui, heading);
        for (key, what) in rows.iter() {
            let p = ui.painter();
            let key = p.layout((*key).to_owned(), theme::font(11.0), theme::KEY, keys - 8.0);
            let what = p.layout((*what).to_owned(), theme::font(11.0), theme::TEXT(), (width - keys).max(40.0));
            let height = key.size().y.max(what.size().y) + 3.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
            ui.painter().galley(rect.left_top(), key, theme::KEY);
            ui.painter().galley(rect.left_top() + egui::vec2(keys, 0.0), what, theme::TEXT());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::FIXED;
    use crate::keymap::{COMMANDS, Chord};

    /// The README's Controls table: the lines from its heading to the next.
    fn controls() -> String {
        let readme = include_str!("../../README.md");
        let start = readme.find("## Controls").expect("the README has a Controls section");
        let rest = &readme[start + 2..];
        let end = rest.find("\n## ").unwrap_or(rest.len());
        // Only the table's first column: what is pressed, not what it does.
        rest[..end]
            .lines()
            .filter_map(|line| line.strip_prefix("| "))
            .filter_map(|row| row.split(" | ").next())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn every_fixed_shortcut_is_in_the_readme_controls_table() {
        let table = controls();
        let missing: Vec<&str> =
            FIXED.iter().flat_map(|(_, rows)| rows.iter()).map(|(key, _)| *key).filter(|key| !table.contains(key)).collect();
        assert!(missing.is_empty(), "not in the README's Controls table: {missing:?}");
    }

    #[test]
    fn every_key_the_app_ships_a_command_on_is_in_the_readme_controls_table() {
        let table = controls();
        let missing: Vec<String> = COMMANDS
            .iter()
            .flat_map(|c| c.keys.iter())
            .map(|k| Chord::parse(k).expect("reads").text())
            .filter(|key| !table.contains(key.as_str()))
            .collect();
        assert!(missing.is_empty(), "not in the README's Controls table: {missing:?}");
    }

    #[test]
    fn no_fixed_shortcut_is_listed_twice() {
        let mut seen = std::collections::HashSet::new();
        for (key, what) in FIXED.iter().flat_map(|(_, rows)| rows.iter()) {
            assert!(seen.insert((key, what)), "{key} is listed twice");
        }
    }
}
