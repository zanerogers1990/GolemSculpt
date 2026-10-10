//! What a region drag is pointed at: the mask, the hidden faces, or the
//! geometry itself.
//!
//! Mask, Hide, Trim and Split are one gesture with four endings, and each
//! has its own colour so the marquee says which is about to happen. Trim's is
//! the warning one: hiding and masking are put right with a click, and
//! trimming deletes geometry.

use egui::Color32;

use crate::gesture::Shape;

/// The marquee while the drag would take away (unmask, show): the red the
/// brushes use for their erasing side.
pub const REMOVE: Color32 = Color32::from_rgb(255, 77, 77);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    /// Mask what the shape covers.
    Mask,
    /// Hide the faces the shape wholly covers.
    Hide,
    /// Cut away what the shape covers and close the hole.
    Trim,
    /// Cut along the shape and keep both sides: the covered one becomes a new
    /// object.
    Split,
}

impl Action {
    pub fn name(self) -> &'static str {
        match self {
            Action::Mask => "Mask",
            Action::Hide => "Hide",
            Action::Trim => "Trim",
            Action::Split => "Split",
        }
    }

    pub fn tint(self) -> Color32 {
        match self {
            Action::Mask => Color32::from_rgb(242, 166, 38),
            Action::Hide => Color32::from_rgb(77, 191, 204),
            Action::Trim => Color32::from_rgb(242, 89, 77),
            Action::Split => Color32::from_rgb(89, 217, 191),
        }
    }

    /// The shapes this action is drawn with, in the order its key steps
    /// through them. Split starts with the straight line: a flat cut is what
    /// keys and printing both want most.
    pub fn shapes(self) -> &'static [Shape] {
        match self {
            Action::Mask | Action::Hide => &[Shape::Box, Shape::Lasso],
            Action::Trim => &[Shape::Box, Shape::Lasso, Shape::Line],
            Action::Split => &[Shape::Line, Shape::Lasso, Shape::Box],
        }
    }

    /// Whether the action changes the geometry (and so runs as a cut).
    pub fn cuts(self) -> bool {
        matches!(self, Action::Trim | Action::Split)
    }

    /// An icon in `assets/icons` for this action drawn with `shape`.
    pub fn icon(self, shape: Shape) -> &'static str {
        match (self, shape) {
            (Action::Mask, Shape::Lasso) => "region_lasso_mask",
            (Action::Mask, _) => "region_box_mask",
            (Action::Hide, Shape::Lasso) => "region_lasso_hide",
            (Action::Hide, _) => "region_box_hide",
            (Action::Trim, Shape::Box) => "region_box_trim",
            (Action::Trim, Shape::Lasso) => "region_lasso_trim",
            (Action::Trim, Shape::Line) => "region_line_trim",
            (Action::Split, Shape::Box) => "region_box_split",
            (Action::Split, Shape::Lasso) => "region_lasso_split",
            (Action::Split, Shape::Line) => "region_line_split",
        }
    }

    /// What a drag does, in a sentence, for the panel and the status line.
    pub fn prompt(self, shape: Shape) -> &'static str {
        match (self, shape) {
            (Action::Mask, Shape::Lasso) => "Draw round part of the model to mask it, front and back.",
            (Action::Mask, _) => "Drag a box over the model to mask what it covers, front and back.",
            (Action::Hide, Shape::Lasso) => "Draw round part of the model to hide it, front and back.",
            (Action::Hide, _) => "Drag a box over the model to hide what it covers, front and back.",
            (Action::Trim, Shape::Box) => "Drag a box over the model: what it covers is cut away and the hole closed.",
            (Action::Trim, Shape::Lasso) => "Draw round part of the model: it is cut away and the hole closed.",
            (Action::Trim, Shape::Line) => "Drag a line across the model: the shaded side is cut away, leaving a flat face.",
            (Action::Split, Shape::Box) => "Drag a box over the model: what it covers is cut off as a new object.",
            (Action::Split, Shape::Lasso) => "Draw round part of the model: it is cut off as a new object.",
            (Action::Split, Shape::Line) => "Drag a line across the model: the shaded side becomes a new object.",
        }
    }

    /// What a click without a drag says when it has nothing to reset.
    pub fn click_hint(self, shape: Shape) -> &'static str {
        match (self, shape) {
            (_, Shape::Line) => "Drag a line across the model.",
            (Action::Split, _) => "Drag a shape out to split.",
            _ => "Drag a shape out to trim.",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_action_starts_on_its_first_shape_and_names_an_icon_for_each() {
        for action in [Action::Mask, Action::Hide, Action::Trim, Action::Split] {
            assert!(!action.shapes().is_empty());
            for &shape in action.shapes() {
                assert!(action.icon(shape).starts_with("region_"));
                assert!(!action.prompt(shape).is_empty());
            }
        }
        assert_eq!(Action::Split.shapes()[0], Shape::Line);
        assert!(!Action::Mask.shapes().contains(&Shape::Line));
    }
}
