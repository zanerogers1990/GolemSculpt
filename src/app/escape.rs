//! Esc in the middle of something takes it back. A brush stroke is put back
//! as if it had never been drawn, and leaves no undo step; a drag of the
//! Transform gizmo puts the objects, or the reshaped surface, back where they
//! were at the press. The button or pen stays down until it is lifted, and
//! does nothing more until then: lifting it does not start anything new.
//!
//! With nothing under way, Esc puts the tool that is up away, as the hosted
//! tools (Measure, Tube and the rest) and SSpheres do: the Transform tool goes
//! back to sculpting. The brush wheel and the clean view are closed first.
//!
//! The SSpheres and the hosted tools see Esc before this does and cancel
//! their own drags (app/spheres.rs, app/tools.rs); so does a deformer of the
//! Deform card while it is up (app/deform.rs).

use super::{Drag, Running};
use crate::doc::Tool;

impl Running {
    /// Esc during a drag the app itself holds: a brush stroke, or a drag of
    /// the Transform gizmo's handles. True if it took one back.
    pub(super) fn escape_drag(&mut self) -> bool {
        match self.input.drag {
            Drag::Sculpt => {
                self.cancel_stroke();
                true
            }
            Drag::Gizmo => self.transform_escape(),
            _ => false,
        }
    }

    /// Esc with nothing under way, after everything that might want it first:
    /// the Transform tool goes back to sculpting. True if it did.
    pub(super) fn escape_tool(&mut self) -> bool {
        if self.input.drag != Drag::None || !self.transform_active() {
            return false;
        }
        self.set_tool(Tool::Sculpt);
        true
    }

    /// Back to sculpting from whatever tool is up: a hosted tool is put away
    /// the way its own Esc puts it away, the others by `set_tool`. What
    /// picking a brush does, from the panel, the wheel or a number key.
    pub(super) fn back_to_sculpting(&mut self) {
        match self.doc.tool {
            Tool::Sculpt => {}
            Tool::Plugin => self.tools_leave(),
            Tool::Transform | Tool::SSpheres => self.set_tool(Tool::Sculpt),
        }
    }

    /// What a stroke's undo step is called: its brush, as the Brushes card
    /// names it ("Clay Strips"), or "Smooth" for a Shift + drag with another
    /// brush in hand, or "Mask" for mask painting.
    pub(super) fn stroke_name(&self) -> String {
        let sculptor = &self.doc.sculptor;
        let s = &sculptor.settings;
        if s.mask_mode {
            return "Mask".to_owned();
        }
        let kernel = sculptor.effective_brush(self.input.mods.shift_key());
        if kernel != s.brush {
            return kernel.name().to_owned();
        }
        sculptor.library.entries()[sculptor.library.selected(s)].name.clone()
    }

    /// Puts the surface back as it was when the stroke began. The stroke is
    /// ended the ordinary way, so everything it touched (layers, paint, what
    /// dynamic topology added) is in the record it makes, and that record is
    /// undone at once instead of going on the history.
    fn cancel_stroke(&mut self) {
        // What the pen did since the last frame goes with the rest.
        self.input.samples.clear();
        let doc = &mut self.doc;
        if let Some(delta) = doc.sculptor.end(&mut doc.mesh, &self.local) {
            doc.mesh.apply_delta(&delta, true);
        }
        doc.sculptor.last_hit = None;
        // The button is still down: the rest of the drag moves nothing.
        self.input.drag = Drag::Ui;
        self.toast("Stroke Cancelled");
        self.redraw = true;
    }
}
