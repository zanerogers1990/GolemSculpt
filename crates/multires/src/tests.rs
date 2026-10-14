//! Tests of the crate as a whole, and the small meshes they share.
//!
//! * `shapes`: what one subdivision makes of sound, open, broken and tiny meshes.
//! * `rules`: where the rules put things, against known answers.
//! * `stack`: the level stack's bookkeeping, deleting levels, taking steps back.
//! * `edits`: a coarse level edited and the detail above carried along.
//! * `dirty`: that edit carried from what it changed, against every vertex looked at.
//! * `carry`: the mask and the helpers for other per-vertex and per-face data.
//! * `parts`: a stack taken apart and put back together.
//! * `riders`: the app's own vectors on the surface, through a split and up from an edit.
//! * `symmetry`: mirrored meshes and mirrored edits stay mirrored.
//! * `timing`: speed and memory at the app's sizes (`--ignored`).

mod carry;
mod dirty;
mod edits;
mod fixtures;
mod parts;
mod riders;
mod rules;
mod shapes;
mod stack;
mod symmetry;
mod timing;
