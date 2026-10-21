//! Every key and pointer shortcut, in one table. The Settings tab's Keyboard
//! Shortcuts card is drawn from it, and a test holds each key it names to the
//! README's Controls table, so a key added to one and not the other fails the
//! build's tests instead of going stale unseen.

use egui::Color32;

use super::theme;

/// The shortcuts, a heading to each group: what you press, and what it does.
/// A key's words are the README's own (its Controls table), so the test below
/// can find them there.
pub const GROUPS: &[(&str, &[(&str, &str)])] = &[
    ("Sculpting", &[
        ("Left drag on the model", "Sculpt"),
        ("Ctrl + drag, or right drag", "Invert the brush"),
        ("Shift + drag", "Smooth"),
        ("1 2 3 4 5 6 7 8 9 0", "Base brushes, Move to Snake Hook"),
        ("P", "Paint brush; again, the one before"),
        ("I", "Eyedropper: pick up paint"),
        ("M", "Mask painting"),
        ("X", "Mirror X"),
        ("Ctrl + Shift + click", "Isolate the face group"),
    ]),
    ("Brush", &[
        ("S / F / D + drag", "Size / strength / focal shift"),
        ("[ / ]", "Brush smaller / bigger"),
        ("Shift + [ / ]", "Strength down / up"),
        ("Scroll over the model", "Brush size"),
        ("Space (hold)", "Brush wheel"),
    ]),
    ("Tools", &[
        ("G", "Transform gizmo"),
        ("L", "Measure"),
        ("Y", "Array"),
        ("T", "Tube"),
        ("B", "Muscle"),
        ("N", "Mask tool: box, lasso, off"),
        ("K", "Split tool: line, lasso, box, off"),
        ("J", "Keys (joints for printing)"),
        ("Enter", "Finish: fuse, accept, build"),
        ("Esc", "Cancel, or put the tool away"),
    ]),
    ("Mesh", &[
        ("R", "Voxel remesh"),
        ("Ctrl + D", "Subdivide"),
        ("Page Up / Page Down", "Finer / coarser level"),
    ]),
    ("View", &[
        ("Left drag on the background", "Orbit"),
        ("Alt + drag", "Orbit"),
        ("Middle drag", "Pan"),
        ("Alt + Shift + drag", "Pan"),
        ("Ctrl + Alt + drag", "Zoom"),
        ("Scroll off the model", "Zoom"),
        ("Double-click background", "Frame the selection"),
        ("F F", "Frame the selection"),
        ("Numpad 5", "Orthographic / perspective"),
        ("O", "Spin the view (turntable)"),
        ("Tab", "Clean view: hide the panels"),
        ("F11", "Fullscreen"),
        ("Ctrl + = / Ctrl + -", "Interface size"),
    ]),
    ("Objects", &[
        ("Double-click another object", "Select it"),
        ("Ctrl + click a row", "Add to the selection"),
        ("H", "Hide the selection"),
        ("Shift + H", "Hide everything else"),
        ("Alt + H", "Show everything"),
        ("/", "Local View: solo the selection"),
        ("Delete", "Delete the object (asks)"),
    ]),
    ("File and undo", &[
        ("Z, Shift + Z", "Undo, redo"),
        ("Ctrl + Z, Ctrl + Y", "Undo, redo"),
        ("Ctrl + S", "Save"),
        ("Ctrl + Shift + S", "Save as"),
        ("Ctrl + O", "Open"),
        ("Ctrl + N", "New scene (asks first)"),
        ("Ctrl + Q", "Quit (asks first)"),
    ]),
];

/// The Keyboard Shortcuts card's body. The keys are the README's words, and
/// some are long ("Left drag on the background"), so each column wraps
/// rather than run into the other, which the toolkit's fixed-width
/// `theme::shortcuts` would let them do.
pub fn list(ui: &mut egui::Ui) {
    let width = ui.available_width();
    // The keys' column as wide as the widest key needs, but never more than
    // about half the card: a longer key wraps onto a second line.
    let widest = GROUPS
        .iter()
        .flat_map(|(_, rows)| rows.iter())
        .map(|(key, _)| ui.painter().layout_no_wrap((*key).to_owned(), theme::font(11.0), theme::KEY).size().x)
        .fold(0.0f32, f32::max);
    let keys = (widest + 10.0).min(width * 0.46);
    for (heading, rows) in GROUPS {
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

/// The colour of the card's title.
pub const COLOR: Color32 = Color32::from_rgb(230, 204, 102);

#[cfg(test)]
mod tests {
    use super::GROUPS;

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
    fn every_shortcut_in_the_settings_card_is_in_the_readme_controls_table() {
        let table = controls();
        let missing: Vec<&str> =
            GROUPS.iter().flat_map(|(_, rows)| rows.iter()).map(|(key, _)| *key).filter(|key| !table.contains(key)).collect();
        assert!(missing.is_empty(), "not in the README's Controls table: {missing:?}");
    }

    #[test]
    fn no_shortcut_is_listed_twice() {
        let mut seen = std::collections::HashSet::new();
        for (key, what) in GROUPS.iter().flat_map(|(_, rows)| rows.iter()) {
            assert!(seen.insert((key, what)), "{key} is listed twice");
        }
    }
}
