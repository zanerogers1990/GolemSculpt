//! Voxel remesh: rebuilds the surface as an even mesh at a chosen resolution,
//! fusing self-intersections on the way.
//!
//! The work is done by `sculpt_volume`: the mesh is scan-converted along each
//! axis onto a lattice of sample points, which gives an inside/outside bit
//! per sample and the exact point and face normal where the surface crosses
//! every lattice edge, and a surface is extracted from that with one vertex
//! for each piece of surface in each lattice cell. See that crate for how and
//! why. This file is the app's side of it: a sculpt mesh in, a sculpt mesh
//! out, and the app's wording for what can go wrong.
//!
//! What callers rely on:
//!
//!   - Sample `i` on an axis sits at `(i + 0.5) * h`, so the coordinate
//!     planes run through cell centres and the lattice is its own mirror
//!     image across each of them.
//!   - With a symmetry axis on, the result is exactly mirror-symmetric across
//!     that plane, bit for bit, whatever the model. It is rebuilt from the
//!     half on the positive side of the plane and that half's reflection, so
//!     where the two halves of the model differ, the positive one wins.
//!   - Inside is a winding number: shells that overlap fuse into their union,
//!     and a shell wound backwards is still solid.
//!   - Holes are capped, each across its own rim, before the mesh is read.

use sculpt_volume::remesh::{RemeshSettings, budgeted_resolution, remesh_with, surface_area};

use crate::mesh::Mesh;

/// Most cells along the model's longest side.
const MAX_RESOLUTION: u32 = 2048;
/// Most triangles a remesh may make. Past this it is refused, not thinned.
const MAX_TRIANGLES: usize = 80_000_000;

const NO_VOLUME: &str = "Remesh found no solid volume. Is the model closed?";
const TOO_MANY: &str = "That resolution would make more than 80 million triangles. Try a lower one.";

pub fn voxel_remesh(mesh: &Mesh, resolution: u32, symmetry: [bool; 3]) -> Result<Mesh, String> {
    let bounds = mesh.bounds();
    if mesh.triangle_count() == 0 || bounds.is_empty() {
        return Err("Nothing to remesh.".into());
    }
    let longest = bounds.size().max_element();
    if !(longest > 0.0 && longest.is_finite()) {
        return Err("The model has no size.".into());
    }
    let resolution = resolution.clamp(4, MAX_RESOLUTION);
    // The kernel would lower the resolution to fit the budget. Here the
    // resolution is the user's own number, so say no instead.
    let area = surface_area(&mesh.positions, &mesh.indices);
    if budgeted_resolution(area, longest as f64, resolution, MAX_TRIANGLES).0 < resolution {
        return Err(TOO_MANY.into());
    }
    let settings = RemeshSettings { resolution, mirror: symmetry, max_triangles: MAX_TRIANGLES };
    let rebuilt = remesh_with(&mesh.positions, &mesh.indices, &settings).map_err(reworded)?;
    Ok(Mesh::new(rebuilt.mesh.positions, rebuilt.mesh.indices, None))
}

/// The kernel's account of a failure, in the words this app has always used
/// for it. Anything it has no words for is passed on as it came.
fn reworded(error: String) -> String {
    let says = |part: &str| error.contains(part);
    if says("no solid volume") || says("not a closed surface") {
        NO_VOLUME.into()
    } else if says("No surface came out") {
        "Remesh produced no surface.".into()
    } else if says("too many triangles") {
        TOO_MANY.into()
    } else if says("too fine") {
        "That resolution needs too much memory for this model. Try a lower one.".into()
    } else {
        error
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    #[test]
    fn nothing_and_nothing_solid_are_refused_in_the_usual_words() {
        let empty = Mesh::new(Vec::new(), Vec::new(), None);
        assert_eq!(voxel_remesh(&empty, 64, [false; 3]).err().unwrap(), "Nothing to remesh.");

        // A flat sheet: something to remesh, with no inside.
        let corners = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::X + Vec3::Y];
        let sheet = Mesh::new(corners, vec![0, 1, 2, 2, 1, 3], None);
        assert_eq!(voxel_remesh(&sheet, 64, [false; 3]).err().unwrap(), NO_VOLUME);
    }

    #[test]
    fn the_lowest_resolutions_are_brought_into_range() {
        let sphere = primitives::build(Primitive::Sphere, 5_000, 0.5);
        let coarsest = voxel_remesh(&sphere, 4, [false; 3]).unwrap().triangle_count();
        for resolution in [0, 1, 3] {
            assert_eq!(voxel_remesh(&sphere, resolution, [false; 3]).unwrap().triangle_count(), coarsest);
        }
        assert!(coarsest > 20);
    }

    #[test]
    fn the_mirror_plane_runs_through_cell_centres() {
        // Vertices sit exactly on the plane, which they could not if a row of
        // samples did: a cell astride it puts its vertex there.
        let sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let rebuilt = voxel_remesh(&sphere, 40, [true, false, false]).unwrap();
        let on_plane = rebuilt.positions.iter().filter(|p| p.x == 0.0).count();
        assert!(on_plane > 100, "{on_plane} vertices on the mirror plane");
    }

    #[test]
    fn the_positive_half_is_the_one_that_is_kept() {
        // A sphere with a second, smaller one stuck on its positive side.
        let mut positions = Vec::new();
        let mut indices = Vec::new();
        for (radius, centre) in [(0.5, Vec3::ZERO), (0.2, Vec3::new(0.5, 0.0, 0.0))] {
            let part = primitives::build(Primitive::Sphere, 20_000, radius);
            let base = positions.len() as u32;
            positions.extend(part.positions.iter().map(|&p| p + centre));
            indices.extend(part.indices.iter().map(|&i| i + base));
        }
        let lopsided = Mesh::new(positions, indices, None);
        let rebuilt = voxel_remesh(&lopsided, 64, [true, false, false]).unwrap();
        let (low, high) = (rebuilt.bounds().min.x, rebuilt.bounds().max.x);
        assert!((high - 0.7).abs() < 0.02 && low == -high, "from {low} to {high}");
    }
}
