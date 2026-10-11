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
//!
//! How fine a remesh may be, how many triangles it may make and how much
//! memory it may take are the kernel's own limits (`sculpt_volume::budget`),
//! the same ones the booleans and a tube merge answer to. The app used to
//! keep a second set here, which did not agree with the first.

use sculpt_volume::budget::{self, MAX_RESOLUTION, MAX_TRIANGLES, MIN_RESOLUTION};
use sculpt_volume::remesh::{RemeshSettings, Remeshed, admits, budgeted_resolution, check_mirror_side, remesh_with};
use sculpt_volume::remesh::surface_area;

use crate::mesh::Mesh;

const NO_VOLUME: &str = "Remesh found no solid volume. Is the model closed?";

/// What is said of a resolution that would make more triangles than a
/// remesh may ([`MAX_TRIANGLES`]): past that it is refused, not thinned.
/// `fits` is the highest resolution that would not, when that is known.
fn too_many(fits: Option<u32>) -> String {
    let instead = fits.map_or("a lower one".to_owned(), |fits| format!("{fits} or lower"));
    format!("That resolution would make more than {} million triangles. Try {instead}.", MAX_TRIANGLES / 1_000_000)
}

/// A refusal from the kernel with the object's own name where the kernel,
/// which has no names, says "The model" or "The first object": the status
/// line then says which object is too far from its mirror plane, has no
/// inside, and so on.
pub fn named(error: String, name: &str) -> String {
    let stand_ins = [budget::MODEL.named, budget::FIRST_OBJECT.named];
    match stand_ins.iter().find_map(|stand_in| error.strip_prefix(stand_in)) {
        Some(rest) => format!("\"{name}\"{rest}"),
        None => error,
    }
}

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
    let resolution = resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION);
    // Asked before anything about size is: a model behind its mirror plane
    // has no resolution that works, and must not be sent to try a lower one.
    check_mirror_side(bounds.max, symmetry)?;
    let settings = RemeshSettings { resolution, mirror: symmetry, max_triangles: MAX_TRIANGLES };
    // The kernel would lower the resolution to fit the budget. Here the
    // resolution is the user's own number, so say no instead, and say what
    // number would do: one the kernel will not then refuse for memory.
    let area = surface_area(&mesh.positions, &mesh.indices);
    let fits = budgeted_resolution(area, longest as f64, resolution, MAX_TRIANGLES).0;
    if fits < resolution {
        let at = |resolution: u32| RemeshSettings { resolution, ..settings };
        let admitted = |lower: u32| admits(&mesh.positions, &mesh.indices, &at(lower)).is_ok();
        return Err(too_many(budget::highest_passing(fits, admitted)));
    }
    let rebuilt = remesh_with(&mesh.positions, &mesh.indices, &settings).map_err(reworded)?;
    let notes = Notes::of(&rebuilt);
    Ok((Mesh::new(rebuilt.mesh.positions, rebuilt.mesh.indices, None), notes))
}

/// The kernel's account of a failure, in the words this app has always used
/// for it. Anything it has no words for is passed on as it came: what the
/// kernel says of a model too far from its mirror plane, or too much for
/// memory, already says what to do.
fn reworded(error: String) -> String {
    let says = |part: &str| error.contains(part);
    if says("no solid volume") || says("not a closed surface") {
        NO_VOLUME.into()
    } else if says("No surface came out") {
        "Remesh produced no surface.".into()
    } else if says("too many triangles") {
        too_many(None)
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

    /// Nine spheres in one place: nine times the surface in one sphere's
    /// box, which at the slider's highest resolution is 89 million triangles.
    fn nine_spheres() -> Mesh {
        let sphere = primitives::build(Primitive::Sphere, 5_000, 0.5);
        let (mut positions, mut indices) = (Vec::new(), Vec::new());
        for _ in 0..9 {
            let base = positions.len() as u32;
            positions.extend_from_slice(&sphere.positions);
            indices.extend(sphere.indices.iter().map(|&i| i + base));
        }
        Mesh::new(positions, indices, None)
    }

    #[test]
    fn too_many_triangles_is_refused_with_a_resolution_that_would_do() {
        let heavy = nine_spheres();
        let refused = voxel_remesh(&heavy, 1024, [false; 3]).err().unwrap();
        let fits: u32 = refused
            .strip_prefix("That resolution would make more than 80 million triangles. Try ")
            .and_then(|rest| rest.strip_suffix(" or lower."))
            .and_then(|number| number.parse().ok())
            .unwrap_or_else(|| panic!("{refused}"));
        assert!((900..1024).contains(&fits), "{fits}");
        // The number offered is one that is not refused in its turn, and the next one up is.
        let area = surface_area(&heavy.positions, &heavy.indices);
        assert_eq!(budgeted_resolution(area, 1.0, fits, MAX_TRIANGLES).0, fits);
        assert!(budgeted_resolution(area, 1.0, fits + 1, MAX_TRIANGLES).0 <= fits);
    }

    /// The kernel's refusal of a model far from its mirror planes reaches
    /// the status line as it was written, with the object's name put in.
    #[test]
    fn a_model_far_from_its_mirror_planes_is_refused_by_name_with_what_to_do() {
        let mut rivet = primitives::build(Primitive::Cube, 2_400, 0.5);
        rivet.positions.iter_mut().for_each(|p| *p += Vec3::new(40.0, 40.0, 0.0));
        let refused = voxel_remesh(&Mesh::new(rivet.positions, rivet.indices, None), 1024, [true, true, false]);
        let said = named(refused.err().unwrap(), "Rivet");
        let start = "\"Rivet\" is about 40 times its own size away from its X and Y mirror planes. Rebuilt mirrored, \
                     the grid has to reach from it to its mirror image, which is too big at this resolution. Turn \
                     the X and Y mirrors off for this, lower the resolution to ";
        let end = " or less, or move its centre onto it with Set Origin to Mesh.";
        assert!(said.starts_with(start) && said.ends_with(end), "{said}");
    }

    /// Twelve cubes in one place: surface enough to be over the triangle
    /// budget at the slider's highest setting, in a model one cube long.
    fn heavy_cube(middle: Vec3) -> Mesh {
        let cube = primitives::build(Primitive::Cube, 2_400, 0.5);
        let count = cube.positions.len() as u32;
        let positions = (0..12).flat_map(|_| cube.positions.iter().map(|&p| p + middle)).collect();
        let indices = (0..12).flat_map(|copy| cube.indices.iter().map(move |&i| i + copy * count)).collect();
        Mesh::new(positions, indices, None)
    }

    /// A model behind its mirror plane is told so whatever else is wrong
    /// with what was asked. Over the triangle budget as well, it used to be
    /// sent to a lower resolution first, and only there told that no
    /// resolution would do.
    #[test]
    fn a_model_behind_its_mirror_plane_is_told_so_before_it_is_told_to_lower_the_resolution() {
        let behind = heavy_cube(Vec3::new(-40.0, 0.0, 0.0));
        for resolution in [1024, 600, 64] {
            let refused = voxel_remesh(&behind, resolution, [true, false, false]).err().unwrap();
            let said = named(refused, "Rivet");
            assert_eq!(
                said,
                "\"Rivet\" lies wholly on the far side of its mirror plane, so its mirror image is empty. Turn the \
                 mirror off for this, move the model across the plane, or move its centre onto it with Set Origin \
                 to Mesh.",
                "at {resolution}"
            );
        }
        // Mirror off, it is only its triangles that are spoken of.
        assert!(voxel_remesh(&behind, 1024, [false; 3]).err().unwrap().contains("80 million triangles"));
    }

    /// The resolution offered for too many triangles is one the kernel then
    /// takes. Twenty-five lengths off two mirror planes the grid is too big
    /// for memory until well below what the triangles alone would allow.
    #[test]
    fn the_resolution_offered_for_too_many_triangles_is_not_then_refused_for_memory() {
        let far = heavy_cube(Vec3::new(25.0, 25.0, 0.0));
        let mirror = [true, true, false];
        let refused = voxel_remesh(&far, 1024, mirror).err().unwrap();
        assert!(refused.starts_with("That resolution would make more than 80 million triangles. Try "), "{refused}");
        let offered: u32 = refused.split(' ').nth(10).and_then(|word| word.parse().ok()).expect("a resolution to try");
        let settings = RemeshSettings { resolution: offered, mirror, max_triangles: MAX_TRIANGLES };
        assert_eq!(admits(&far.positions, &far.indices, &settings), Ok(()), "offered {offered}");
        // On its mirror planes the triangles alone decide, as they always did.
        let centred = heavy_cube(Vec3::ZERO);
        let area = surface_area(&centred.positions, &centred.indices);
        let by_triangles = budgeted_resolution(area, 1.0, 1024, MAX_TRIANGLES).0;
        assert!(offered < by_triangles, "{offered} against {by_triangles}");
        assert_eq!(voxel_remesh(&centred, 1024, mirror).err().unwrap(), too_many(Some(by_triangles)));
    }

    #[test]
    fn the_kernels_stand_in_for_an_object_gives_way_to_its_name() {
        assert_eq!(named("The model has no size.".into(), "Sphere"), "\"Sphere\" has no size.");
        assert_eq!(
            named("The first object is not a closed surface, so it has no inside.".into(), "Cube"),
            "\"Cube\" is not a closed surface, so it has no inside."
        );
        // Anything else is passed on untouched.
        assert_eq!(named(NO_VOLUME.into(), "Sphere"), NO_VOLUME);
        let elsewhere = "Try 200 or lower. The model is large.";
        assert_eq!(named(elsewhere.into(), "Sphere"), elsewhere);
        assert_eq!(too_many(None), "That resolution would make more than 80 million triangles. Try a lower one.");
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
