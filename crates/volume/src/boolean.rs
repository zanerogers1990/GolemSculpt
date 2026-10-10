//! Booleans: one solid joined to, cut out of, or intersected with another.
//!
//! Done on a lattice, like the remesh, and for the same reason DynaMesh works
//! that way: sculpted meshes pass through themselves, have near-degenerate
//! triangles and pinches, and an exact boolean has to fail on those, while a
//! lattice one simply resamples whatever solid the input describes. The
//! result is watertight and evenly tessellated, at the cost of being resampled
//! everywhere (the whole target is rebuilt, not just the cut) and of losing
//! features finer than one cell.
//!
//! Each solid is scan-converted onto one shared lattice ([`crate::scan`]). A
//! sample is inside the result according to which solids it is inside of, and
//! where the result's surface crosses a lattice edge it does so at an exact
//! crossing of one of the solids, with that solid's own face normal. So the
//! seam where two surfaces meet is rebuilt as the crease it is: the vertex
//! solve ([`crate::qef`]) sees two different planes in a seam cell and lands
//! on their intersection. Combining sampled distance fields with min and max
//! instead rounds every seam off by about a cell.
//!
//! A solid here is anything a winding number makes sense of. Several closed
//! shells listed in one mesh are their union, which is how to cut with many
//! cutters at once: append them into one mesh.

use glam::Vec3;
use sculpt_geom::{Aabb, TriMesh};

use crate::extract::{self, CellSource, ExtractOptions, Hermite, hermite_from_edges, values_from_edges};
use crate::grid::Grid;
use crate::remesh::{MAX_RESOLUTION, MAX_TRIANGLES, MIN_RESOLUTION, Remeshed, Sources, surface_area};
use crate::scan::{Scan, facing_out, unpack_direction};

/// Which way two solids are combined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BooleanOp {
    /// Everything either solid covers, welded into one surface with nothing
    /// left inside the overlap.
    Union,
    /// The first solid with every other cut out of it.
    Subtract,
    /// Only what all the solids share.
    Intersect,
}

#[derive(Clone, Copy, Debug)]
pub struct BooleanSettings {
    /// Cells along the first solid's longest side, as for a remesh. Also the
    /// feature-size floor: a cutter detail thinner than one cell rounds away,
    /// so cutting fine detail out of a big block wants a higher number than
    /// remeshing the block would.
    pub resolution: u32,
    /// The cell size to use instead of one worked out from `resolution`.
    pub cell_size: Option<f32>,
    /// Coordinate planes the result must be exactly symmetric across. It
    /// always is, whatever the solids: the result's half on the positive side
    /// of each plane is kept and reflected onto the other.
    pub mirror: [bool; 3],
    pub max_triangles: usize,
}

impl BooleanSettings {
    pub fn new(resolution: u32) -> BooleanSettings {
        BooleanSettings { resolution, cell_size: None, mirror: [false; 3], max_triangles: MAX_TRIANGLES }
    }
}

/// Most solids one operation takes. More are better appended into one mesh.
pub const MAX_SOLIDS: usize = 8;

/// Most crossings looked at on one lattice edge.
const MAX_EVENTS: usize = 16;

/// Several scan-converted solids read as one.
struct Combined {
    scans: Vec<Scan>,
    op: BooleanOp,
    grid: Grid,
}

impl Combined {
    /// Inside or outside the result, from each solid's winding number.
    #[inline]
    fn inside(&self, windings: &[i32]) -> bool {
        let mut rest = windings[1..].iter().map(|&w| w != 0);
        match self.op {
            BooleanOp::Union => windings[0] != 0 || rest.any(|inside| inside),
            BooleanOp::Subtract => windings[0] != 0 && !rest.any(|inside| inside),
            BooleanOp::Intersect => windings[0] != 0 && rest.all(|inside| inside),
        }
    }

    /// Where the result's surface crosses the lattice edge from `at` one step
    /// up `axis`: the crossings of every solid on that edge are walked in
    /// order, and the ones at which the result changes between inside and
    /// outside are its surface.
    fn edge_hit(&self, axis: usize, at: [usize; 3], lower_inside: bool) -> Option<(f32, Vec3)> {
        let mut events = [(0.0f32, 0i8, 0u8, 0u32); MAX_EVENTS];
        let mut count = 0;
        let mut windings = [0i32; MAX_SOLIDS];
        for (s, scan) in self.scans.iter().enumerate() {
            windings[s] = scan.winding_at(axis, at);
            for (t, winding, normal) in scan.edge_crossings(axis, at) {
                if count < MAX_EVENTS {
                    events[count] = (t, winding, s as u8, normal);
                    count += 1;
                }
            }
        }
        if count == 0 {
            return None;
        }
        let events = &mut events[..count];
        events.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then(b.1.cmp(&a.1)));

        let windings = &mut windings[..self.scans.len()];
        let mut inside = self.inside(windings);
        let (mut sum, mut hits, mut from) = (0.0f32, 0u32, None);
        for &(t, winding, s, normal) in events.iter() {
            windings[s as usize] += winding as i32;
            let now = self.inside(windings);
            if now != inside {
                inside = now;
                sum += t;
                hits += 1;
                from.get_or_insert(normal);
            }
        }
        // Crossings but no change: the passes broke a tie differently. Any
        // crossing on the edge is then within rounding of the right place.
        let normal = from.unwrap_or(events[0].3);
        let t = if hits > 0 { sum / hits as f32 } else { events[0].0 };
        let fraction = (t - self.grid.coord(axis, at[axis])) / self.grid.cell;
        // Facing out of the result, which for a cutter is into the cutter.
        Some((fraction.clamp(0.0, 1.0), facing_out(unpack_direction(normal), axis, lower_inside)))
    }
}

impl CellSource for Combined {
    fn row(&self, j: usize, k: usize, out: &mut [u64], scratch: &mut Vec<u64>) -> bool {
        let any = self.scans[0].row(j, k, out);
        if !any && self.op != BooleanOp::Union {
            return false;
        }
        scratch.resize(out.len(), 0);
        for other in &self.scans[1..] {
            other.row(j, k, scratch);
            for (a, &b) in out.iter_mut().zip(scratch.iter()) {
                *a = match self.op {
                    BooleanOp::Union => *a | b,
                    BooleanOp::Subtract => *a & !b,
                    BooleanOp::Intersect => *a & b,
                };
            }
        }
        out.iter().any(|&w| w != 0)
    }

    fn hermite(&self, cell: [usize; 3], mask: u8, out: &mut Hermite) {
        hermite_from_edges(cell, mask, out, |axis, at, lower_inside| self.edge_hit(axis, at, lower_inside));
    }

    fn corner_values(&self, cell: [usize; 3], mask: u8) -> [f32; 8] {
        values_from_edges(&self.grid, cell, mask, |axis, at| self.edge_hit(axis, at, true).map(|hit| hit.0))
    }
}

/// Combines two solids at `resolution` cells along the first one's longest
/// side.
pub fn boolean(a: &TriMesh, b: &TriMesh, op: BooleanOp, resolution: u32) -> Result<TriMesh, String> {
    boolean_with(a, &[b], op, &BooleanSettings::new(resolution)).map(|r| r.mesh)
}

/// Combines `target` with every one of `others`, with every setting and the
/// full result. All meshes must be in the same space.
pub fn boolean_with(
    target: &TriMesh,
    others: &[&TriMesh],
    op: BooleanOp,
    settings: &BooleanSettings,
) -> Result<Remeshed, String> {
    let target_bounds =
        crate::check_mesh(&target.positions, &target.indices).map_err(|e| format!("The first object: {e}"))?;
    if others.is_empty() {
        return Err("There is no second object to combine with.".into());
    }
    if others.len() + 1 > MAX_SOLIDS {
        return Err(format!("That is too many objects at once. Join some of them first; the limit is {MAX_SOLIDS}."));
    }
    let mut other_bounds = Vec::with_capacity(others.len());
    for other in others {
        let checked = crate::check_mesh(&other.positions, &other.indices);
        other_bounds.push(checked.map_err(|e| format!("The second object: {e}"))?);
    }

    // Subtract and Intersect can only remove material from the target, so the
    // lattice need cover nothing more: a cutter ten times the target's size
    // costs what remeshing the target costs. Union has to cover everything.
    let bounds = match op {
        BooleanOp::Union => other_bounds.iter().fold(target_bounds, |all, b| Aabb::union(all, *b)),
        BooleanOp::Subtract | BooleanOp::Intersect => target_bounds,
    };
    if op != BooleanOp::Union && !other_bounds.iter().any(|b| b.overlaps(&target_bounds)) {
        // Refusing beats "succeeding": with no overlap the operation would
        // only resample the target (Subtract) or erase it (Intersect), and
        // leave the user wondering which they got.
        return Err(match op {
            BooleanOp::Subtract => "The two objects do not overlap, so there is nothing to cut away.",
            _ => "The two objects do not overlap, so they have nothing in common.",
        }
        .into());
    }

    let extent = target_bounds.size().max_element();
    if !(extent > 0.0) {
        return Err("The first object has no size.".into());
    }
    let mut resolution = settings.resolution.clamp(MIN_RESOLUTION, MAX_RESOLUTION);
    let cell = settings.cell_size.unwrap_or(extent / resolution as f32);
    if !(cell > 0.0 && cell.is_finite()) {
        return Err("The cell size must be more than zero.".into());
    }
    if settings.cell_size.is_some() {
        // What the given cell size comes to, for the report.
        resolution = (extent / cell + 0.5) as u32;
    }
    // Every piece of a union's surface is a piece of one of the solids', so
    // its size can be known before any work is done, and a resolution that
    // would fit suggested. (The other two are bounded by the target's lattice
    // whatever the cutter's size, and are stopped during extraction instead.)
    if op == BooleanOp::Union {
        let solids = std::iter::once(target).chain(others.iter().copied());
        let area: f64 = solids.map(|m| surface_area(&m.positions, &m.indices)).sum();
        let expected = 3.0 * area / (cell as f64 * cell as f64);
        if expected > 2.0 * settings.max_triangles as f64 {
            let fits = (extent as f64 / cell as f64 * (settings.max_triangles as f64 / expected).sqrt()) as u32;
            return Err(format!(
                "That resolution would make too many triangles. Try {} or lower.",
                fits.clamp(MIN_RESOLUTION, MAX_RESOLUTION)
            ));
        }
    }

    let grid = Grid::covering(&bounds, cell)?.mirrored(settings.mirror);
    let mut scans = Vec::with_capacity(others.len() + 1);
    for (index, mesh) in std::iter::once(target).chain(others.iter().copied()).enumerate() {
        let scan = Scan::new(&mesh.positions, &mesh.indices, &grid)?;
        // Holes are capped as a remesh caps them. A sheet has no inside even
        // then, and would silently do nothing.
        if !scan.is_closed() && scan.encloses_nothing() {
            let which = if index == 0 { "first" } else { "second" };
            return Err(format!(
                "The {which} object is not a closed surface, so it has no inside. Close its holes first."
            ));
        }
        scans.push(scan);
    }
    let sources = Sources::of(&scans);
    let source = Combined { scans, op, grid };
    let options = ExtractOptions { mirror: settings.mirror, max_vertices: settings.max_triangles / 4 * 3 };
    let surface = extract::extract(&source, &grid, &options).map_err(|error| {
        if !error.contains("no solid volume") {
            return error;
        }
        match op {
            BooleanOp::Subtract => "The cutter removed the whole object. Nothing is left.".to_string(),
            BooleanOp::Intersect => "The objects have nothing in common. The result is empty.".to_string(),
            BooleanOp::Union => error,
        }
    })?;
    Ok(Remeshed::new(surface, resolution, cell, sources))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::remesh::{RemeshSettings, remesh_with};
    use crate::testing::{Defects, Rng, assert_closed, box_mesh, defects, icosphere, lens_volume, rotated, shells};
    use crate::testing::sphere_volume;
    use std::collections::HashSet;

    fn spheres() -> (TriMesh, TriMesh) {
        (icosphere(5, 1.0, Vec3::new(-0.5, 0.02, 0.01)), icosphere(5, 1.0, Vec3::new(0.5, 0.02, 0.01)))
    }

    fn combined(a: &TriMesh, b: &TriMesh, op: BooleanOp, resolution: u32) -> Remeshed {
        let out = boolean_with(a, &[b], op, &BooleanSettings::new(resolution)).unwrap();
        assert_eq!((out.stats.skipped_quads, out.stats.residual_collapses), (0, 0));
        assert_closed(&out.mesh, &format!("{op:?}"));
        out
    }

    /// A boolean says how many of its solids had holes capped for it.
    #[test]
    fn the_result_counts_the_solids_that_were_capped() {
        let (a, b) = spheres();
        let whole = boolean_with(&a, &[&b], BooleanOp::Subtract, &BooleanSettings::new(48)).unwrap();
        assert_eq!(whole.sources, Sources::default());
        let pierced = crate::testing::without_near(&b, Vec3::new(0.5, 1.02, 0.01), 0.1);
        assert!(pierced.triangle_count() < b.triangle_count());
        for (target, cutter, capped) in [(&a, &pierced, 1), (&pierced, &a, 1), (&pierced, &pierced, 2)] {
            let out = boolean_with(target, &[cutter], BooleanOp::Union, &BooleanSettings::new(48)).unwrap();
            assert_eq!(out.sources, Sources { capped, voted: 0 });
            assert_closed(&out.mesh, "a union of capped solids");
        }
    }

    /// Two unit spheres a unit apart, against the analytic volumes.
    #[test]
    fn overlapping_spheres_have_the_volumes_they_should() {
        let (a, b) = spheres();
        let (ball, lens) = (sphere_volume(1.0), lens_volume(1.0, 1.0));
        for (op, want, pieces) in [
            (BooleanOp::Union, 2.0 * ball - lens, 1),
            (BooleanOp::Subtract, ball - lens, 1),
            (BooleanOp::Intersect, lens, 1),
        ] {
            let out = combined(&a, &b, op, 64);
            assert_eq!(shells(&out.mesh), pieces, "{op:?}");
            let volume = out.mesh.signed_volume();
            assert!((volume / want - 1.0).abs() < 0.006, "{op:?}: {volume} vs {want}");
        }
    }

    /// The seam between the two surfaces is a crease, and comes back as one:
    /// its vertices lie on both spheres at once, to a small fraction of a
    /// cell. Combining sampled fields with min and max rounds it by about a
    /// cell.
    #[test]
    fn the_seam_is_sharp() {
        let (a, b) = spheres();
        let (ca, cb) = (Vec3::new(-0.5, 0.02, 0.01), Vec3::new(0.5, 0.02, 0.01));
        let out = combined(&a, &b, BooleanOp::Intersect, 64);
        let mut worst = 0.0f32;
        let mut on_seam = 0;
        for p in &out.mesh.positions {
            let (da, db) = ((p.distance(ca) - 1.0).abs(), (p.distance(cb) - 1.0).abs());
            // Every vertex is on one sphere or the other.
            assert!(da.min(db) < 0.1 * out.cell, "a vertex {} cells off both spheres", da.min(db) / out.cell);
            if (p.x).abs() < 0.3 * out.cell {
                worst = worst.max(da.max(db));
                on_seam += 1;
            }
        }
        assert!(on_seam > 100, "{on_seam} seam vertices");
        assert!(worst < 0.3 * out.cell, "seam {} cells off the true crease", worst / out.cell);
    }

    #[test]
    fn a_box_cut_from_a_box_leaves_flat_faces_and_the_right_volume() {
        let block = box_mesh(Vec3::splat(-0.5), Vec3::splat(0.5));
        let upright = box_mesh(Vec3::new(-0.2, -0.2, -1.0), Vec3::new(0.2, 0.2, 1.0));
        let cutter = rotated(&upright, Vec3::new(0.0, 0.0, 30.0));
        let out = combined(&block, &cutter, BooleanOp::Subtract, 60);
        // A square hole 0.4 across, right through.
        let want = 1.0 - 0.4 * 0.4 * 1.0;
        let volume = out.mesh.signed_volume();
        assert!((volume / want - 1.0).abs() < 0.004, "{volume} vs {want}");
        // One piece, with a hole through it (genus 1): V - E + F = 0.
        assert_eq!(shells(&out.mesh), 1);
        let (v, f) = (out.mesh.vertex_count() as i64, out.mesh.triangle_count() as i64);
        assert_eq!(v - f * 3 / 2 + f, 0);
    }

    #[test]
    fn a_cutter_larger_than_the_target_costs_no_more() {
        let target = icosphere(4, 0.5, Vec3::ZERO);
        let slab = box_mesh(Vec3::new(-50.0, 0.0, -50.0), Vec3::new(50.0, 50.0, 50.0));
        let out = combined(&target, &slab, BooleanOp::Subtract, 48);
        let volume = out.mesh.signed_volume() / (0.5 * sphere_volume(0.5));
        assert!((volume - 1.0).abs() < 0.01, "half a sphere, ratio {volume}");
        assert!(out.mesh.positions.iter().all(|p| p.y < 0.01 && p.length() < 0.51));
    }

    #[test]
    fn several_cutters_and_several_shells_in_one_cutter() {
        let block = box_mesh(Vec3::splat(-0.5), Vec3::splat(0.5));
        let left = icosphere(4, 0.25, Vec3::new(-0.5, 0.0, 0.0));
        let right = icosphere(4, 0.25, Vec3::new(0.5, 0.0, 0.0));
        let want = 1.0 - sphere_volume(0.25);
        let settings = BooleanSettings::new(64);
        let apart = boolean_with(&block, &[&left, &right], BooleanOp::Subtract, &settings).unwrap();
        let mut both = left.clone();
        both.append(&right);
        let joined = boolean_with(&block, &[&both], BooleanOp::Subtract, &settings).unwrap();
        for out in [&apart, &joined] {
            assert_closed(&out.mesh, "two bites");
            assert!((out.mesh.signed_volume() / want - 1.0).abs() < 0.004, "{}", out.mesh.signed_volume());
        }
        assert_eq!(apart.mesh.indices.len(), joined.mesh.indices.len());
    }

    #[test]
    fn a_symmetric_pair_gives_an_exactly_symmetric_result() {
        let body = icosphere(5, 0.6, Vec3::ZERO);
        let mut ears = icosphere(4, 0.3, Vec3::new(0.45, 0.4, 0.0));
        let mut other = icosphere(4, 0.3, Vec3::new(0.45, 0.4, 0.0));
        other.transform(&glam::Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        ears.append(&other);
        for op in [BooleanOp::Union, BooleanOp::Subtract] {
            let settings = BooleanSettings { mirror: [true, false, false], ..BooleanSettings::new(72) };
            let out = boolean_with(&body, &[&ears], op, &settings).unwrap();
            assert_closed(&out.mesh, "mirrored boolean");
            assert_eq!(out.stats.mirror_skipped, 0, "{op:?}");
            let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
            let all: HashSet<_> = out.mesh.positions.iter().map(|&p| bits(p)).collect();
            let twinned = |p: &&Vec3| all.contains(&bits(Vec3::new(-p.x, p.y, p.z)));
            let unmatched = out.mesh.positions.iter().filter(|p| !twinned(p)).count();
            assert_eq!(unmatched, 0, "{op:?}");
        }
    }

    /// A cutter on one side only, with the mirror on: the cut is made on
    /// both sides if it is on the positive one and on neither if it is not.
    #[test]
    fn a_lopsided_boolean_is_made_symmetric_from_its_positive_half() {
        let body = icosphere(5, 0.6, Vec3::ZERO);
        let settings = BooleanSettings { mirror: [true, false, false], ..BooleanSettings::new(72) };
        let bits = |p: Vec3| p.to_array().map(|v| (v + 0.0).to_bits());
        for (side, bites) in [(1.0, 2.0), (-1.0, 0.0)] {
            let cutter = icosphere(4, 0.3, Vec3::new(0.55 * side, 0.3, 0.0));
            let out = boolean_with(&body, &[&cutter], BooleanOp::Subtract, &settings).unwrap();
            assert_closed(&out.mesh, "mirrored cut");
            assert_eq!((out.stats.mirror_skipped, shells(&out.mesh)), (0, 1));
            let all: HashSet<_> = out.mesh.positions.iter().map(|&p| bits(p)).collect();
            assert!(out.mesh.positions.iter().all(|p| all.contains(&bits(Vec3::new(-p.x, p.y, p.z)))));
            // What one cut takes, from a cut without the mirror.
            let plain = boolean_with(&body, &[&cutter], BooleanOp::Subtract, &BooleanSettings::new(72)).unwrap();
            let whole = sphere_volume(0.6);
            let bite = whole - plain.mesh.signed_volume();
            assert!(bite > 0.02, "the cutter bites {bite}");
            let want = whole - bites * bite;
            let volume = out.mesh.signed_volume();
            assert!((volume / want - 1.0).abs() < 0.006, "cutter at {side}: volume {volume}, expected {want}");
        }
    }

    #[test]
    fn the_cell_size_can_be_given_outright() {
        let (a, b) = spheres();
        let settings = BooleanSettings { cell_size: Some(0.05), ..BooleanSettings::new(4) };
        let out = boolean_with(&a, &[&b], BooleanOp::Union, &settings).unwrap();
        assert_eq!((out.cell, out.resolution), (0.05, 40));
        let want = 2.0 * sphere_volume(1.0) - lens_volume(1.0, 1.0);
        assert!((out.mesh.signed_volume() / want - 1.0).abs() < 0.01);
        // The lowest resolution there is, and below it, still give a solid.
        for resolution in [0, 1, MIN_RESOLUTION] {
            let coarse = boolean(&a, &b, BooleanOp::Union, resolution).unwrap();
            assert_closed(&coarse, "coarsest union");
        }
    }

    #[test]
    fn operations_that_make_no_sense_say_why() {
        let (a, b) = spheres();
        let far = icosphere(3, 1.0, Vec3::new(9.0, 0.0, 0.0));
        let empty = TriMesh::default();
        let error = |a: &TriMesh, b: &TriMesh, op| boolean(a, b, op, 32).unwrap_err();
        assert!(error(&empty, &b, BooleanOp::Union).contains("first object"));
        assert!(error(&a, &empty, BooleanOp::Union).contains("second object"));
        assert!(error(&a, &far, BooleanOp::Subtract).contains("do not overlap"));
        assert!(error(&a, &far, BooleanOp::Intersect).contains("do not overlap"));
        assert!(boolean(&a, &far, BooleanOp::Union, 32).is_ok());
        // Boxes that overlap as boxes, solids that do not.
        let corner = icosphere(3, 0.2, Vec3::new(0.3, 0.82, 0.81));
        assert!(error(&a, &corner, BooleanOp::Intersect).contains("nothing in common"));
        let big = icosphere(3, 5.0, Vec3::ZERO);
        assert!(error(&a, &big, BooleanOp::Subtract).contains("Nothing is left"));
        let corners = vec![Vec3::new(-2.0, 0.0, -2.0), Vec3::new(2.0, 0.0, -2.0), Vec3::new(0.0, 0.1, 2.0)];
        let sheet = TriMesh::new(corners, vec![0, 1, 2]);
        assert!(error(&a, &sheet, BooleanOp::Subtract).contains("not a closed surface"));
        assert!(boolean_with(&a, &[], BooleanOp::Union, &BooleanSettings::new(32)).is_err());
        let many = [&b; MAX_SOLIDS];
        assert!(boolean_with(&a, &many, BooleanOp::Union, &BooleanSettings::new(32)).is_err());
        let tight = BooleanSettings { max_triangles: 1000, ..BooleanSettings::new(200) };
        assert!(boolean_with(&a, &[&b], BooleanOp::Union, &tight).unwrap_err().contains("or lower"));
    }

    /// One awkward solid: a handful of shapes of one kind, overlapping at
    /// random.
    fn awkward(rng: &mut Rng, kind: usize) -> TriMesh {
        let mut solid = TriMesh::default();
        for _ in 0..1 + rng.below(12) {
            let c = (rng.vec3() - 0.5) * 2.0;
            match kind {
                // Boxes snapped to a coarse lattice: faces, edges and corners
                // that coincide with each other and with the samples.
                0 => {
                    let snap = |v: Vec3| (v * 4.0 + 0.5).floor() / 4.0;
                    let low = snap(c);
                    solid.append(&box_mesh(low, low + snap(rng.vec3() + 0.25).max(Vec3::splat(0.25))));
                }
                // Plates thinner than a cell.
                1 => {
                    let size = Vec3::new(0.5 + rng.next(), 0.004 + rng.next() * 0.02, 0.5 + rng.next());
                    solid.append(&rotated(&box_mesh(c, c + size), rng.vec3() * 90.0));
                }
                // Spheres of all sizes, some wound inside out.
                2 => {
                    let mut sphere = icosphere(1 + rng.below(3) as u32, 0.05 + rng.next() * 0.6, c);
                    if rng.next() < 0.2 {
                        sphere.flip();
                    }
                    solid.append(&sphere);
                }
                // Loose triangles, with no inside at all.
                3 => {
                    let base = solid.positions.len() as u32;
                    solid.positions.extend([c, c + rng.vec3(), c + rng.vec3()]);
                    solid.indices.extend([base, base + 1, base + 2]);
                }
                // Spheres with holes, repeated triangles and triangles of no area.
                _ => {
                    let mut sphere = icosphere(2, 0.2 + rng.next() * 0.5, c);
                    let cut = rng.below(sphere.indices.len() / 3 - 3) * 3;
                    if rng.next() < 0.5 {
                        sphere.indices.drain(cut..cut + 9);
                    }
                    if rng.next() < 0.3 {
                        let again = sphere.indices[..6].to_vec();
                        sphere.indices.extend(again);
                    }
                    sphere.indices.extend([0, 0, 1]);
                    solid.append(&sphere);
                }
            }
        }
        solid
    }

    /// Whatever goes in, what comes out is either a plain refusal or a closed,
    /// consistently wound 2-manifold: never a panic, a hole or an edge shared
    /// by four triangles.
    #[test]
    fn awkward_solids_never_give_a_broken_surface() {
        let mut rng = Rng::new(77);
        let (mut made, mut creases) = (0, 0);
        let mut check = |out: &Remeshed, what: &str| {
            assert_eq!((out.stats.skipped_quads, out.stats.residual_collapses), (0, 0), "{what}");
            assert_eq!(defects(&out.mesh), Defects::default(), "{what}");
            assert!(out.mesh.positions.iter().all(|p| p.is_finite()), "{what}");
            made += 1;
            creases += out.stats.crease_cells;
        };
        for round in 0..150 {
            let solid = awkward(&mut rng, round % 5);
            let resolution = 8 + rng.below(60) as u32;
            let mirror = [rng.next() < 0.3, rng.next() < 0.3, rng.next() < 0.3];
            let what = format!("round {round} at {resolution}");
            let settings = RemeshSettings { mirror, ..RemeshSettings::new(resolution) };
            if let Ok(out) = remesh_with(&solid.positions, &solid.indices, &settings) {
                check(&out, &what);
            }
            // And as a boolean of its first half against its second.
            if solid.triangle_count() < 40 {
                continue;
            }
            let half = solid.indices.len() / 6 * 3;
            let a = TriMesh::new(solid.positions.clone(), solid.indices[..half].to_vec());
            let b = TriMesh::new(solid.positions.clone(), solid.indices[half..].to_vec());
            for op in [BooleanOp::Union, BooleanOp::Subtract, BooleanOp::Intersect] {
                let settings = BooleanSettings { mirror, ..BooleanSettings::new(resolution) };
                if let Ok(out) = boolean_with(&a, &[&b], op, &settings) {
                    check(&out, &format!("{what}, {op:?}"));
                }
            }
        }
        assert!(made > 200 && creases > 2000, "{made} surfaces, {creases} crease cells");
    }
}
