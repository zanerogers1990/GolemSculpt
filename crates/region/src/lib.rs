//! The region tools: box, lasso and straight-line mask, hide, trim and split,
//! and mask extract.
//!
//! The user drags a box, draws a lasso or lays a straight line across the
//! viewport, and what it covers (front and back, straight through the model)
//! is masked, hidden, cut away and capped, or split off as a separate closed
//! piece.
//! A masked area can then be pulled out as its own solid with thickness. The
//! geometry is all `sculpt_cut`'s; this crate is the gestures, the marquee,
//! the panels and the undo steps on top of it.
//!
//! Five tools, each a button:
//!
//! | Tool | Key | A drag |
//! | --- | --- | --- |
//! | **Mask** | N | masks what a box or lasso covers |
//! | **Hide** | | hides the faces a box or lasso wholly covers |
//! | **Trim** | | cuts away what a box, lasso or line covers and closes the hole |
//! | **Split** | K | cuts along a line, lasso or box and keeps both pieces |
//! | **Extract** | | (no drag) shows the masked area as a shell; Accept makes it an object |
//!
//! One tool per action rather than one tool with a mode row, so the lit
//! button says what the next drag will do: a trim deletes geometry and should
//! not look like a mask. The shape is the row inside each.
//!
//! A shape acts through every place the selected object is drawn, as in the
//! Unity app: the object, its live mirror copies, its instances and the copies
//! another tool keeps of it (an array's). The tools learn of the places from
//! the scene (`ObjectInfo`), not from whoever made them; see `screen.rs`.
//!
//! While Mask, Hide, Trim or Split is up it has the pointer:
//!
//! | Input | Action |
//! | --- | --- |
//! | Drag (anywhere on the viewport) | Draw the shape; it acts on release |
//! | Ctrl, or drag with the right button | Reverse: unmask; show; trim keeps the covered part |
//! | Shift | Box, lasso: act on everything outside. Line: snap (or, with Snap ticked, do not) |
//! | Click | Mask: clear the mask. Hide: show everything |
//! | Esc | Cancel the drag in progress, else put the tool away |
//! | N / K | Next shape, and after the last put the tool away |
//! | Space (hold) | Shape wheel |
//! | 1 - 9 | Back to sculpting |
//!
//! A snapping line (the panel's Snap tick, or Shift) is for a cut that has to
//! be exact: it turns in steps of 15 degrees and, where it passes close to
//! the middle of the selected object or to its origin, is moved sideways to
//! go exactly through it. Both points are marked while Snap is on, and the
//! line's angle is read off beside it.
//!
//! The one place these differ from the rest of the app: with a region tool
//! up, a drag on empty background draws a region instead of orbiting,
//! because a box or a cut line has to be able to start beside the model.
//! Alt-drag and the middle button still move the camera. This is how the
//! Unity app's region modes behave once armed. Extract takes no drags, so
//! there a drag orbits as usual.

mod action;
mod cut;
mod demo;
mod extract;
mod gesture;
mod hide;
mod mask;
mod panel;
mod screen;
mod tool;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;

pub use action::Action;
pub use extract::Extract;
pub use gesture::Shape;
pub use tool::Region;

use sculpt_toolkit::Tool;

/// The Mask tool (box and lasso mask), ready to register with the app.
pub fn mask() -> Box<dyn Tool> {
    Box::new(Region::mask())
}

/// The Hide tool (box and lasso hide of part of a mesh), ready to register.
pub fn hide() -> Box<dyn Tool> {
    Box::new(Region::hide())
}

/// The Trim tool, ready to register.
pub fn trim() -> Box<dyn Tool> {
    Box::new(Region::trim())
}

/// The Split tool, ready to register.
pub fn split() -> Box<dyn Tool> {
    Box::new(Region::split())
}

/// The Extract tool, ready to register.
pub fn extract() -> Box<dyn Tool> {
    extract::tool()
}

/// All five, in the order of their buttons: Mask, Hide, Trim, Split,
/// Extract.
pub fn tools() -> Vec<Box<dyn Tool>> {
    vec![mask(), hide(), trim(), split(), extract()]
}
