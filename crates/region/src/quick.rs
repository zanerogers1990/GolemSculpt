//! The box mask the mask brush borrows. While mask painting, Shift + drag
//! draws a box instead of a stroke, and what it covers is masked as the Mask
//! tool's box would mask it (front and back, through the mirror copies);
//! with the right button it unmasks. A box that covers nothing of the selected
//! object, dragged or tapped on empty space, clears the mask.
//!
//! The app keeps the drag and calls in here with a context; no tool is
//! brought up, so the brush is still in hand when the box lands.

use glam::Vec2;
use sculpt_cut::region::CLICK_SLOP_PIXELS;
use sculpt_toolkit::{Ctx, Draw, View};

use crate::action::{Action, REMOVE};
use crate::gesture::{Drag, LineSnap, Marquee, Shape};
use crate::mask;

/// A box being dragged out with Shift held while mask painting.
pub struct QuickBox {
    drag: Drag,
}

impl QuickBox {
    /// Starts at `at`, in viewport pixels. `unmask` (the right button)
    /// takes the mask off what the box covers instead.
    pub fn begin(at: Vec2, unmask: bool) -> QuickBox {
        QuickBox { drag: Drag::begin(Shape::Box, at, unmask) }
    }

    pub fn drag_to(&mut self, at: Vec2) {
        self.drag.move_to(at, 0.0);
    }

    /// Draws the box: the Mask tool's orange, or its red while it unmasks.
    pub fn draw(&self, view: &View, draw: &mut Draw) {
        let tint = if self.drag.inverted { REMOVE } else { Action::Mask.tint() };
        let look = Marquee {
            tint,
            viewport: view.size,
            pixels_per_point: view.pixels_per_point,
            snap: LineSnap::OFF,
            other_side: false,
        };
        self.drag.draw(&mut draw.overlay, look);
    }

    /// The button came up at `at`: the box masks what it covers, or clears
    /// the mask when it covers none of the selected object. A tap on the
    /// model does nothing, so a slip of the pen there loses no mask.
    pub fn finish(mut self, cx: &mut Ctx, at: Vec2) -> Result<String, String> {
        self.drag.move_to(at, 0.0);
        let view = cx.view;
        if self.drag.is_click(CLICK_SLOP_PIXELS * view.pixels_per_point) {
            if cx.scene().raycast(&view.ray(at)).is_some() {
                return Ok("Shift + drag a box to mask what it covers; on empty space it clears the mask.".to_owned());
            }
            return mask::clear(cx);
        }
        let region = self.drag.region(LineSnap::OFF)?;
        let value = if self.drag.inverted { 0.0 } else { 1.0 };
        let mut covered = Vec::new();
        let result = mask::apply(cx, &view, &region, value, false, &mut covered);
        if !covered.contains(&true) {
            return mask::clear(cx);
        }
        result
    }
}
