//! Screen regions, trimming, splitting and cap triangulation.
//!
//! The geometry behind the region tools: a box, lasso or straight line drawn on
//! screen, used to select, hide, mask, trim (cut away and close) or split a
//! mesh (cut into separate closed pieces), plus pulling a masked area out as
//! its own solid.
//!
//! - `region`: the drawn shape as a screen-space stencil, and the projection
//!   that puts a mesh's points under it.
//! - `select`: which vertices and triangles a shape covers, and what a hide or
//!   mask gesture should change.
//! - `trim`: cutting a mesh along a shape and closing the opening.
//! - `split`: cutting a mesh into the part a shape covers and the part it
//!   does not, with the faces where the two mate.
//! - `extract`: a masked area pulled out as a closed shell.
//! - `cap`, `planar`, `polygon`: filling a closed loop, or a flat polygon with
//!   holes, with well-shaped triangles; and the 2D predicates under them.
//!
//! Screen positions are pixels with (0, 0) at the top-left corner. Meshes are
//! wound counter-clockwise seen from outside, and come back that way.

pub mod cap;
pub mod extract;
mod patch;
pub mod planar;
pub mod polygon;
pub mod region;
pub mod select;
pub mod split;
pub mod trim;

#[cfg(test)]
mod split_tests;
#[cfg(test)]
mod testing;
#[cfg(test)]
mod trim_tests;

pub use extract::{ExtractSettings, extract};
pub use region::{CutView, ScreenProjection, ScreenRegion};
pub use split::{Piece, Split, SplitPass, split};
pub use trim::{TrimSummary, Trimmed, trim, trim_passes};
