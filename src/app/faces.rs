//! Hidden faces, from the app's side: hiding the triangles a region tool
//! names, Show All and Invert Visible, each one undo step on the active
//! object. What hiding means for a mesh is in `mesh/visibility.rs`.

use sculpt_cut::select;

use super::Running;
use crate::doc::Action;

/// A count with its thousands set apart: 100,444.
fn grouped(count: usize) -> String {
    let digits = count.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, digit) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(digit);
    }
    out
}

impl Running {
    /// Hides these triangles of the active object, or with `hide` false shows
    /// them again. One undo step; says why and returns false when nothing
    /// changed. A hide that would leave nothing of the object on screen is
    /// refused: an object that has vanished gives no sign that it is only
    /// hidden.
    pub(crate) fn hide_faces(&mut self, triangles: &[u32], hide: bool) -> bool {
        self.redraw = true;
        let doc = &mut self.doc;
        let Some(delta) = doc.mesh.set_triangles_hidden(triangles, hide) else {
            self.status(if hide { "Nothing new to hide there." } else { "Nothing hidden there." }, false);
            return false;
        };
        if doc.mesh.visible_triangle_count() == 0 {
            doc.mesh.apply_visibility(&delta, true);
            self.status("That would hide the whole object - skipped.", false);
            return false;
        }
        doc.history.push_visibility(delta);
        let hidden = doc.mesh.hidden_triangle_count();
        self.toast(if hide { "Hide" } else { "Show" });
        self.status(
            if hidden == 0 {
                "Everything is showing again.".to_owned()
            } else {
                format!("{} faces hidden. Show All brings them back.", grouped(hidden))
            },
            true,
        );
        true
    }

    /// Shows every hidden face of the active object again, as one undo step.
    pub(crate) fn show_all_faces(&mut self) -> bool {
        self.redraw = true;
        let doc = &mut self.doc;
        let Some(delta) = doc.mesh.show_all() else {
            self.status("Nothing is hidden.", false);
            return false;
        };
        doc.history.push_visibility(delta);
        self.toast("Show All");
        self.status("Everything is showing again.", true);
        true
    }

    /// Swaps what is hidden and what shows on the active object, as one undo
    /// step. Does nothing while nothing is hidden.
    pub(crate) fn invert_visible_faces(&mut self) -> bool {
        self.redraw = true;
        let doc = &mut self.doc;
        let Some(delta) = doc.mesh.invert_visible() else {
            self.status("Nothing is hidden, so there is nothing to swap.", false);
            return false;
        };
        doc.history.push_visibility(delta);
        self.toast("Invert Visible");
        self.status("Swapped the hidden and the showing parts.", true);
        true
    }

    /// Hides the half of the active object that faces the camera's right, cut
    /// on a slant so the inside of the far wall shows; used with
    /// `--screenshot` to check hidden faces by eye.
    pub(super) fn demo_hide(&mut self) {
        let mesh = &self.doc.mesh;
        let centre = mesh.bounds().center();
        let inside: Vec<bool> = mesh.positions.iter().map(|&p| (p.x - centre.x) + (p.z - centre.z) > 0.0).collect();
        let triangles = select::triangles_inside(&mesh.indices, &inside);
        self.act(Action::HideFaces { triangles, hide: true });
    }
}

#[cfg(test)]
mod tests {
    use super::grouped;

    #[test]
    fn counts_are_grouped_in_thousands() {
        let cases = [(0, "0"), (999, "999"), (1000, "1,000"), (100_444, "100,444"), (1_234_567, "1,234,567")];
        for (count, want) in cases {
            assert_eq!(grouped(count), want);
        }
    }
}
