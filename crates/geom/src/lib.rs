//! Plain-data geometry every tool crate shares: triangle meshes, bounds,
//! symmetry, units.
//!
//! Nothing here knows about the app, the GPU or the sculpting mesh. Functions
//! take slices and return `Vec`s, so they can be tested and timed on their own
//! and called from any thread.

pub mod adjacency;
pub mod decimate;
pub mod edge_census;
pub mod edge_key;
pub mod join;
pub mod mesh;
pub mod mirror;
pub mod symmetry;
pub mod tri;
pub mod units;
pub mod vector;

#[cfg(test)]
mod testing;

pub use adjacency::{MeshAdjacency, MeshEdges};
pub use decimate::{Decimated, decimate};
pub use edge_census::{EdgeCensus, edge_census};
pub use mesh::{Aabb, TriMesh};
pub use symmetry::{SymmetryGroup, SymmetryOp};
pub use units::LengthUnit;
