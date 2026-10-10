//! The Keys tool: joints that register the pieces of a split model.
//!
//! A sculpt printed in pieces needs the pieces to find each other again. A
//! key is a peg standing out of one half of a cut and a matching socket sunk
//! into the other, the socket being the peg's exact outline grown by a
//! printing clearance, so the halves go back together in exactly one
//! position.
//!
//! The tool works on two objects that touch face to face: the two selected
//! ones, or the selected one and whichever other object its cut face mates
//! with (`mating` finds that from the geometry, so the halves can come from
//! the Split tool, a saved scene or another program). With the tool up the
//! joints are a scaffold of previews:
//!
//! | Input | Action |
//! | --- | --- |
//! | Click the cut face | Add a joint there (keep the button down to place it) |
//! | Drag a joint | Move it; it turns red where it would not fit |
//! | Right-click a joint, or Delete over it | Remove it |
//! | Wheel over a joint | Turn it 15 degrees (Shift: 5); R turns it 90 |
//! | V | Show the half with the pegs, the half with the sockets, or both |
//! | Enter | Build the joints into both pieces: one undo step |
//! | Z / Shift + Z | Undo / redo a joint edit |
//! | Esc | Let go of a joint, else stop a build, else put the tool away |
//!
//! J brings the tool up and puts it away. A drag off the cut face orbits.
//!
//! Two things differ from the Unity tool, both because of where a scaffold
//! lives here. Putting the tool away (Esc, J, another tool) keeps the joints
//! as planned instead of building them or throwing them away: only Enter
//! builds. And the joints can be changed after they are built by undoing the
//! build, which hands them back, rather than by reopening the keyed pieces.
//!
//! The geometry, each part usable on its own:
//!
//! - `profile`: a key's cross-section, as a rounded polygon that offsets
//!   exactly.
//! - `joint`, `mating`: the two halves of a cut, the faces along which they
//!   touch, and the direction each joint pulls apart in.
//! - `joint_map`: a joint's face on a grid, with the material behind it.
//! - `planner`: where keys can go (automatically, or checked as the user
//!   places them) and building a plan into both halves.
//! - `builder`: a peg or a socket sewn into a cut face, triangle by triangle.
//! - `cutting`: a mesh cut in two along a plane, for the demo and the tests.

pub mod builder;
pub mod cutting;
mod input;
pub mod joint;
pub mod joint_map;
pub mod mating;
mod pair;
mod panel;
pub mod planner;
mod preview;
pub mod profile;
mod tool;
pub mod types;

#[cfg(test)]
mod geometry_tests;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod tool_tests;

pub use input::ViewSide;
pub use joint::{KeyJoint, KeyPart};
pub use joint_map::JointMap;
pub use planner::{BuildResult, JointPlan, KeyPlan, KeySettings, KeySide, PlannedKey};
pub use profile::{KeyProfile, KeyShape};
pub use tool::{Keys, tool};
pub use types::{KeyDimensions, KeyFeature, KeyFrame, KeyRole};
