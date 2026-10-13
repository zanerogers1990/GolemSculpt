//! A mesh that mirrors onto itself must subdivide, and take an edit, the
//! same on both sides.

use std::collections::HashMap;

use glam::Vec3;

use super::fixtures::*;
use crate::{LevelMesh, Split};

fn mirrored(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}

/// For each vertex, the vertex at its mirror image across x = 0. The
/// positions must mirror exactly.
fn twins(positions: &[Vec3]) -> Vec<usize> {
    // Adding zero turns a minus zero on the plane into the zero its twin has.
    let key = |p: Vec3| (p + Vec3::ZERO).to_array().map(f32::to_bits);
    let at: HashMap<[u32; 3], usize> = positions.iter().enumerate().map(|(v, &p)| (key(p), v)).collect();
    assert_eq!(at.len(), positions.len(), "no two vertices share a place");
    positions.iter().map(|&p| *at.get(&key(mirrored(p))).expect("every vertex has a mirror twin")).collect()
}

/// The furthest any vertex is from the mirror image of its twin.
fn lopsidedness(positions: &[Vec3], twins: &[usize]) -> f32 {
    (0..positions.len()).map(|v| positions[v].distance(mirrored(positions[twins[v]]))).fold(0.0, f32::max)
}

/// Lumps on a mesh that are the same either side of x = 0, to the bit.
fn lump(mesh: &mut LevelMesh) {
    for p in &mut mesh.positions {
        let swell = 1.0 + 0.25 * (p.x * p.x * 9.0 + p.y * 5.0).sin() * (p.z * 4.0).cos();
        *p *= swell;
    }
}

/// A mesh's levels are numbered the same whatever its shape, so the twins of
/// a finer level are read off a flat split of the exactly mirrored coarser
/// one, which mirrors exactly. This walks both: the flat chain for the twins
/// and the smooth chain under test.
fn smooth_levels_stay_mirrored(base: LevelMesh, levels: usize) -> f32 {
    let (mut flat, mut smooth) = (base.clone(), base);
    let mut worst = 0.0f32;
    for _ in 0..levels {
        flat = crate::subdivide(&flat, Split::Flat).unwrap().mesh;
        smooth = crate::subdivide(&smooth, Split::Smooth).unwrap().mesh;
        assert_eq!(flat.triangles, smooth.triangles);
        worst = worst.max(lopsidedness(&smooth.positions, &twins(&flat.positions)));
    }
    worst
}

/// A few units in the last place of a coordinate of these unit-sized meshes.
const ROUNDING: f32 = 4.0 * f32::EPSILON;

#[test]
fn a_mirrored_triangle_mesh_subdivides_mirrored() {
    let mut mesh = ico_sphere(2);
    lump(&mut mesh);
    assert_eq!(lopsidedness(&mesh.positions, &twins(&mesh.positions)), 0.0, "the test mesh mirrors exactly");
    let worst = smooth_levels_stay_mirrored(mesh, 4);
    println!("Loop: worst departure from mirror symmetry over four levels {worst:e}");
    assert!(worst <= ROUNDING, "{worst}");
}

#[test]
fn a_mirrored_polygon_mesh_subdivides_mirrored() {
    // A cube split flat twice is a mirrored mesh of quads with plenty of vertices on the plane and off it.
    let (_, mut mesh) = raised(&cube(), &[Split::Flat, Split::Flat]);
    lump(&mut mesh);
    assert_eq!(lopsidedness(&mesh.positions, &twins(&mesh.positions)), 0.0, "the test mesh mirrors exactly");
    let worst = smooth_levels_stay_mirrored(mesh, 4);
    println!("Catmull-Clark: worst departure from mirror symmetry over four levels {worst:e}");
    assert!(worst <= ROUNDING, "{worst}");
}

#[test]
fn a_mirrored_open_mesh_subdivides_mirrored() {
    // A bent sheet of quads centred on the plane: rims, corners and all. (The
    // triangle sheet's diagonals all lean one way, so it is not a mirrored mesh.)
    let mut mesh = quad_grid(6, 5);
    mesh.positions.iter_mut().for_each(|p| *p = Vec3::new(p.x - 3.0, p.y, 0.3 * (p.x - 3.0).abs() + 0.1 * p.y * p.y));
    let worst = smooth_levels_stay_mirrored(mesh, 4);
    // Coordinates here run to 5, where a unit in the last place is four times what it is at 1.
    assert!(worst <= ROUNDING * 4.0, "{worst}");
}

/// The spin of a detail frame counts every edge of a vertex's fan alike, so
/// a mirrored edit of a mirrored sculpt comes back up mirrored even where the
/// edit stretches and shears the surface. (With one tangent edge per vertex,
/// picked by number, the two sides pick differently and this fails.)
#[test]
fn a_mirrored_edit_of_a_coarse_level_comes_back_up_mirrored() {
    for base in [ico_sphere(2), raised(&cube(), &[Split::Flat, Split::Flat]).1] {
        let coarse_twins = twins(&base.positions);
        let (mut levels, mut top) = raised(&base, &[Split::Smooth, Split::Smooth]);
        // The finest level's twins, from a flat chain as above.
        let (_, flat) = raised(&base, &[Split::Flat, Split::Flat]);
        let top_twins = twins(&flat.positions);

        // Sculpt the finest level the same on both sides.
        let mut rng = Rng(19);
        let nudges: Vec<Vec3> = (0..top.positions.len()).map(|_| rng.vector() * 0.01).collect();
        for v in 0..top.positions.len() {
            let twin = top_twins[v];
            let both = (nudges[v] + mirrored(nudges[twin])) * 0.5;
            top.positions[v] += if twin == v { Vec3::new(0.0, both.y, both.z) } else { both };
        }
        let before = lopsidedness(&top.positions, &top_twins);
        assert!(before <= ROUNDING, "{before}");

        // A lopsided-looking but mirrored edit: each coarse vertex pulled by a field that bends with y and z.
        let mut coarse = levels.step_to(0, &top.positions, &top.mask).unwrap().mesh;
        let pull = |p: Vec3| Vec3::new(p.x * 0.3 * p.y, 0.2 * (p.z * 3.0).sin() * p.x.abs(), 0.15 * p.y * p.y);
        let pulled: Vec<Vec3> = (0..coarse.positions.len())
            .map(|v| {
                // From the vertex and its twin alike, so rounding cannot tell the sides apart.
                let (p, q) = (coarse.positions[v], mirrored(coarse.positions[coarse_twins[v]]));
                let moved = ((p + pull(p)) + (q + pull(q))) * 0.5;
                if coarse_twins[v] == v { Vec3::new(0.0, moved.y, moved.z) } else { moved }
            })
            .collect();
        coarse.positions = pulled;
        let up = levels.step_to(2, &coarse.positions, &coarse.mask).unwrap().mesh;
        assert!(furthest(&up.positions, &top.positions) > 0.05, "the edit moved the sculpt");
        let after = lopsidedness(&up.positions, &top_twins);
        println!("worst departure from mirror symmetry after a mirrored coarse edit {after:e}");
        assert!(after <= ROUNDING * 4.0, "{after}");
    }
}
