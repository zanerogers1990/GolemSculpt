//! Signed distance fields, surface extraction, voxel remesh and booleans.
//!
//! Everything here turns a solid into samples on a lattice and back into a
//! closed, evenly tessellated triangle mesh:
//!
//!   - [`remesh()`] rebuilds one model's surface; [`boolean()`] joins, cuts or
//!     intersects two. Both scan-convert their meshes ([`scan`]) rather than
//!     sample a distance field, so cost follows surface area and creases
//!     survive.
//!   - [`sdf`] is for solids that are a formula rather than a mesh: fill a
//!     lattice with signed distances and extract its surface, or sample a
//!     mesh's distance field to blend with one.
//!   - [`mod@extract`], [`qef`] and [`topology`] are the extraction itself and its
//!     parts, for a tool that meshes a field of its own.
//!   - [`bvh`] answers nearest-point and inside/outside questions about a
//!     fixed mesh, from any number of threads at once.
//!
//! All of it shares one lattice layout ([`grid`]), chosen so that a model can
//! be rebuilt exactly symmetric. Wherever a mirror axis is asked for, the
//! result is exactly symmetric across that coordinate plane whatever went in:
//! it is built from the half on the positive side of the plane and that
//! half's reflection.
//!
//! Triangles are counter-clockwise seen from outside, in and out.

pub mod boolean;
pub mod bvh;
pub mod extract;
pub mod grid;
mod mirror;
mod parallel;
pub mod qef;
pub mod remesh;
pub mod scan;
pub mod sdf;
pub mod topology;

#[cfg(test)]
mod testing;
#[cfg(test)]
mod timing;

pub use boolean::{BooleanOp, BooleanSettings, boolean, boolean_with};
pub use bvh::{Closest, TriangleBvh};
pub use extract::{CellSource, ExtractOptions, Hermite, Stats, Surface, extract};
pub use grid::Grid;
pub use remesh::{RemeshSettings, Remeshed, remesh, remesh_with};

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::Aabb;

/// Checks that a triangle list is something the lattice code can work on, and
/// returns its bounds.
pub(crate) fn check_mesh(positions: &[Vec3], indices: &[u32]) -> Result<Aabb, String> {
    if positions.is_empty() || indices.len() < 3 {
        return Err("There are no triangles to work on.".into());
    }
    let highest = indices.par_iter().copied().max().unwrap_or(0);
    if indices.len() % 3 != 0 || highest as usize >= positions.len() {
        return Err("The triangle list is damaged: it names vertices that do not exist.".into());
    }
    let bounds = positions
        .par_iter()
        .fold(|| Aabb::EMPTY, |mut b, &p| {
            b.grow(p);
            b
        })
        .reduce(|| Aabb::EMPTY, Aabb::union);
    // A coordinate that is not a number fails every comparison, so it never
    // shows up in the bounds: look for it directly.
    if !bounds.min.is_finite() || !bounds.max.is_finite() || positions.par_iter().any(|p| !p.is_finite()) {
        return Err("A vertex has a position that is not a number.".into());
    }
    Ok(bounds)
}
