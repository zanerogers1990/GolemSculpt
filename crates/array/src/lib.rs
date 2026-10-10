//! Array layouts: copies along lines, circles and curves, and the tool that
//! edits them.
//!
//! An array repeats one object a number of times along a straight line, round
//! an axis, or along a drawn curve, with a size that can taper or swell along
//! the run. The geometry is where each copy goes (`layout::compute`), the
//! curve it follows (`path`), the state that is saved and undone
//! (`ArrayState`), the numbers a new array starts with (`make`) and baking the
//! copies into one mesh (`bake`). None of it knows about the app.
//!
//! The interactive tool, `tool()`, is written against `sculpt-toolkit` and
//! sits on top of that: the live copies (the selected object's own mesh drawn
//! again, so they follow every stroke), the handles and the pen's Draw Curve,
//! the panel, the tool's own undo history, and making the copies real as one
//! commit. See `tool.rs` for what it does and `finish.rs` for the commit.
//!
//! Everything is relative to the object the array repeats. Handle points are
//! offsets from its position in world axes, and a copy's placement is an offset,
//! a turn and a size from the object, so moving, turning or resizing the object
//! carries the whole array and never needs a new layout.
//!
//! Handedness. The Unity app was left-handed, this one is right-handed, and the
//! layout maths is the same numbers in both: an angle about an axis, and a path's
//! frame carried along by cross products, give the same coordinates either way;
//! only how a turn looks on screen differs. Angles here are glam's, so a positive
//! turn about an axis is counter-clockwise looking at the origin from the axis's
//! positive end. The circle in `Design/Array/array_circle.png` runs
//! counter-clockwise from the object on screen, with the camera on the axis's
//! positive side, which is this app's front view looking down -Z at a circle
//! about Z; the layout reproduces that without any sign change.

pub mod bake;
pub mod layout;
pub mod make;
pub mod path;
mod state;
mod types;

mod arrays;
mod copies;
mod demo;
mod edit;
mod finish;
mod gestures;
mod panel;
mod picture;
mod pose;
mod simplify;
mod tool;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod tests;
#[cfg(test)]
mod tool_tests;
#[cfg(test)]
mod copy_tests;

pub use bake::{MAX_COMBINED_VERTICES, bake, bake_array, bake_normals, local_matrices};
pub use copies::{Place, drawn as drawn_copies, places as copy_places};
pub use finish::Apply;
pub use layout::{axis_vector, compute as compute_layout, radial_arc, scale_at};
pub use pose::CopyPick;
pub use state::ArrayState;
pub use tool::{ArrayTool, tool};
pub use types::{ArrayKind, ArrayPlacement, ArraySettings, Pose};
