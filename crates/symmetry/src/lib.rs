//! Symmetry maps, mirror and weld, symmetrize, mirror repeaters.
//!
//! Everything the app does to make or keep a mesh symmetric, as pure geometry
//! over slices:
//!
//! * `ops`: what each symmetry command computes (Status, Make Symmetric, Cut &
//!   Mirror, Cleanup), with the status line for each outcome. Start here.
//! * `map`: which vertex on one side of the plane IS which on the other.
//! * `repair`: fixes that only move vertices, through that pairing.
//! * `project`: laying one side onto the other's mirrored surface where the
//!   vertices do not pair up.
//! * `weld`: fixes that rebuild the mesh: mirror one half over the other,
//!   merge a doubled seam.
//! * `mirror`: an object's symmetry settings, and live mirror copies of whole
//!   objects.
//! * `grid`: which vertices are near a point.
//!
//! Mirrored brush strokes do not come through here. They reflect the brush and
//! re-run it, which never goes stale; the repairs are for the two things that
//! cannot do: telling whether a mesh IS symmetric, and putting it right when it
//! is not.
//!
//! A vertex-only repair returns the vertices it moved (`repair::Moved`), to be
//! recorded as an ordinary sparse undo step. One that changes topology returns
//! a new mesh and, for every new vertex, the old vertex it came from, so a mask
//! survives it.

use glam::Vec3;

mod cells;
pub mod grid;
pub mod map;
pub mod mirror;
pub mod ops;
mod par;
pub mod project;
pub mod repair;
pub mod weld;

#[cfg(test)]
mod fixtures;

pub use grid::VertexGrid;
pub use map::SymmetryMap;
pub use mirror::{MirrorCopy, MirrorRepeater, Placement, SymmetrySettings};
pub use repair::Moved;

/// Which local axis a mirror plane is perpendicular to: `X` is the YZ plane
/// through the origin, and so on. The same plane mirrored sculpting reflects
/// strokes across.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Axis {
    X,
    Y,
    Z,
}

impl Axis {
    pub const ALL: [Axis; 3] = [Axis::X, Axis::Y, Axis::Z];

    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    pub fn from_index(index: usize) -> Option<Axis> {
        Axis::ALL.get(index).copied()
    }

    pub fn name(self) -> &'static str {
        ["X", "Y", "Z"][self.index()]
    }

    /// The component of `p` measured across the plane: its signed distance from
    /// it.
    #[inline]
    pub fn coord(self, p: Vec3) -> f32 {
        p[self.index()]
    }

    /// `p` reflected through the plane.
    #[inline]
    pub fn reflect(self, mut p: Vec3) -> Vec3 {
        p[self.index()] = -p[self.index()];
        p
    }

    /// `p` moved straight onto the plane.
    #[inline]
    pub fn pin(self, mut p: Vec3) -> Vec3 {
        p[self.index()] = 0.0;
        p
    }

    /// -1 on this axis and +1 on the others: scaling by it is `reflect`.
    #[inline]
    pub fn signs(self) -> Vec3 {
        self.reflect(Vec3::ONE)
    }
}
