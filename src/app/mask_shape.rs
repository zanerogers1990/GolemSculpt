//! Mask Recesses and Mask Peaks, the app's side: the command run on the
//! active object as one undo step, and what the status line says of it.
//! What is masked, and why, is `sculpt/cavity.rs`.

use super::Running;
use crate::sculpt::cavity::{self, Shape};

impl Running {
    /// Masks the active object's recesses or its peaks, in place of the
    /// mask it had, and says how much of the surface that came to.
    pub(super) fn mask_shape(&mut self, shape: Shape, amount: f32) {
        let delta = cavity::mask(&mut self.doc.mesh, shape, amount);
        let changed = delta.is_some();
        if let Some(delta) = delta {
            self.doc.history.push_stroke(delta);
        }
        let share = cavity::share_masked(&self.doc.mesh);
        self.status(said(shape, share, changed), true);
    }
}

/// What the status line says: how much was masked, and what to do next
/// with it or instead.
fn said(shape: Shape, share: f32, changed: bool) -> String {
    let what = shape.name();
    if share < 0.0005 {
        let more = if shape == Shape::Recesses { "shallower hollows" } else { "gentler rises" };
        return format!("Nothing was masked: the surface has no {what} sharp enough at this Amount. Raise Amount to take in {more}.");
    }
    if !changed {
        return format!("The {what} are masked already at this Amount.");
    }
    // A part too small to round to one in a hundred is still said to be there.
    let percent = (share * 100.0).round().max(1.0);
    format!("Masked the {what}: about {percent:.0}% of the surface. Invert the mask to work only on them.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_status_line_says_how_much_and_what_next() {
        let masked = said(Shape::Recesses, 0.073, true);
        assert!(masked.contains("recesses") && masked.contains("about 7%") && masked.contains("Invert"), "{masked}");
        assert!(said(Shape::Peaks, 0.002, true).contains("about 1%"), "a sliver is not nothing");
        let none = said(Shape::Peaks, 0.0, false);
        assert!(none.contains("Nothing was masked") && none.contains("Raise Amount") && none.contains("gentler rises"), "{none}");
        assert!(said(Shape::Recesses, 0.0, false).contains("shallower hollows"));
        assert!(said(Shape::Recesses, 0.2, false).contains("already"));
    }
}
