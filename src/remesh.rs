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

use sculpt_volume::remesh::{RemeshSettings, Remeshed, budgeted_resolution, remesh_with, surface_area};

use crate::mesh::Mesh;

/// Most cells along the model's longest side.
const MAX_RESOLUTION: u32 = 2048;
/// Most triangles a remesh may make. Past this it is refused, not thinned.
const MAX_TRIANGLES: usize = 80_000_000;

const NO_VOLUME: &str = "Remesh found no solid volume. Is the model closed?";
const TOO_MANY: &str = "That resolution would make more than 80 million triangles. Try a lower one.";

/// What a remesh or a boolean did, or could not do, that changes what the
/// user has in front of them without showing: holes closed for them, a guess
/// made where the surface was torn, faces the rebuild could not finish. The
/// kernel has always counted these; this is the app passing them on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Notes {
    /// Meshes that went in with holes, each capped flat across its rim.
    pub capped: usize,
    /// Meshes torn, doubled or wound so that no cap fits, read by a vote of the axes.
    pub voted: usize,
    /// Faces left out because the lattice had no vertex for a corner: holes.
    pub skipped_quads: usize,
    /// Places where two sheets in one lattice cell could not be kept apart:
    /// edges shared by four triangles.
    pub residual_collapses: usize,
}

impl Notes {
    pub fn of(built: &Remeshed) -> Notes {
        Notes {
            capped: built.sources.capped,
            voted: built.sources.voted,
            skipped_quads: built.stats.skipped_quads,
            residual_collapses: built.stats.residual_collapses,
        }
    }

    /// Sentences for the status line after a remesh of one model, each with
    /// a space before it; empty when there is nothing to say.
    pub fn after_remesh(&self) -> String {
        let mut text = String::new();
        if self.capped > 0 {
            text += " The model had holes, which were closed over flat.";
        }
        if self.voted > 0 {
            text += " Part of its surface was torn, doubled or inside out, so what is solid there is a best \
                     guess: look that area over.";
        }
        text + &self.unfinished()
    }

    /// The same after a boolean, which reads several objects.
    pub fn after_boolean(&self) -> String {
        let of_the_objects = |count: usize| match count {
            1 => "One of the objects".to_owned(),
            _ => format!("{count} of the objects"),
        };
        let mut text = String::new();
        if self.capped > 0 {
            text += &format!(" {} had holes, which were closed over flat first.", of_the_objects(self.capped));
        }
        if self.voted > 0 {
            text += &format!(
                " {} was torn, doubled or inside out, so what is solid there is a best guess: look that area over.",
                of_the_objects(self.voted)
            );
        }
        text + &self.unfinished()
    }

    /// What the rebuild itself could not finish. Neither should happen; both
    /// leave the result not quite closed, which the check that follows counts.
    fn unfinished(&self) -> String {
        let mut text = String::new();
        if self.skipped_quads > 0 {
            text += &format!(" {} faces could not be built, which leaves holes.", self.skipped_quads);
        }
        if self.residual_collapses > 0 {
            text += &format!(
                " {} places where surfaces nearly touch could not be kept apart.",
                self.residual_collapses
            );
        }
        text
    }
}

pub fn voxel_remesh(mesh: &Mesh, resolution: u32, symmetry: [bool; 3]) -> Result<Mesh, String> {
    voxel_remesh_noted(mesh, resolution, symmetry).map(|(rebuilt, _)| rebuilt)
}

/// [`voxel_remesh`], with what it did about a model that was not closed.
pub fn voxel_remesh_noted(mesh: &Mesh, resolution: u32, symmetry: [bool; 3]) -> Result<(Mesh, Notes), String> {
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
    let notes = Notes::of(&rebuilt);
    Ok((Mesh::new(rebuilt.mesh.positions, rebuilt.mesh.indices, None), notes))
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

    /// A sphere with the triangles near `+X` taken out: a hole with a rim.
    fn holed_sphere() -> Mesh {
        let sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let kept = sphere.indices.chunks_exact(3).filter(|t| t.iter().any(|&v| sphere.positions[v as usize].x < 0.45));
        let indices: Vec<u32> = kept.flatten().copied().collect();
        assert!(indices.len() < sphere.indices.len());
        Mesh::new(sphere.positions.clone(), indices, None)
    }

    #[test]
    fn a_remesh_says_when_it_closed_holes_for_the_user() {
        let sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let (_, notes) = voxel_remesh_noted(&sphere, 48, [false; 3]).unwrap();
        assert_eq!(notes, Notes::default());
        assert_eq!(notes.after_remesh(), "");

        let (rebuilt, notes) = voxel_remesh_noted(&holed_sphere(), 48, [false; 3]).unwrap();
        assert_eq!(notes, Notes { capped: 1, ..Notes::default() });
        assert_eq!(notes.after_remesh(), " The model had holes, which were closed over flat.");
        // And closed they are.
        assert!(sculpt_geom::edge_census(rebuilt.vertex_count(), &rebuilt.indices).is_closed());
    }

    #[test]
    fn every_note_has_plain_words() {
        let all = Notes { capped: 2, voted: 1, skipped_quads: 3, residual_collapses: 4 };
        assert_eq!(
            all.after_remesh(),
            " The model had holes, which were closed over flat. Part of its surface was torn, doubled or inside \
             out, so what is solid there is a best guess: look that area over. 3 faces could not be built, which \
             leaves holes. 4 places where surfaces nearly touch could not be kept apart."
        );
        assert_eq!(
            all.after_boolean(),
            " 2 of the objects had holes, which were closed over flat first. One of the objects was torn, doubled \
             or inside out, so what is solid there is a best guess: look that area over. 3 faces could not be \
             built, which leaves holes. 4 places where surfaces nearly touch could not be kept apart."
        );
        assert_eq!(Notes::default().after_boolean(), "");
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
