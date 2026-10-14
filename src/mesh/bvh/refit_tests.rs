//! The boxes a stroke's refits leave: always round the live surface and the
//! one the stroke started on, and no bigger than those two together.

use glam::Vec3;

use super::Aabb;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

fn holds(b: &Aabb, p: Vec3) -> bool {
    p.cmpge(b.min).all() && p.cmple(b.max).all()
}

fn within(inner: &Aabb, outer: &Aabb) -> bool {
    inner.min.cmpge(outer.min).all() && inner.max.cmple(outer.max).all()
}

/// Every leaf's box holds every corner of its triangles where it is now and
/// where the stroke found it, and every box holds its children's.
fn check(mesh: &Mesh, what: &str) {
    let bvh = &mesh.bvh;
    for (l, leaf) in bvh.leaves.iter().enumerate() {
        let b = bvh.leaf_box(l as u32);
        for &v in &mesh.indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3] {
            assert!(holds(&b, mesh.positions[v as usize]), "{what}: leaf {l} lost vertex {v} where it is");
            assert!(holds(&b, mesh.start_pos(v)), "{what}: leaf {l} lost vertex {v} where it started");
        }
    }
    for node in &bvh.nodes {
        if node.leaf().is_none() {
            for child in [node.a, node.b] {
                assert!(within(&bvh.nodes[child as usize].bounds, &node.bounds), "{what}: a box lost a child");
            }
        }
    }
}

/// A Snake Hook's worth of dabs dragging a patch far out and back. The boxes
/// hold both surfaces all the way, and once the patch is home again they are
/// back to the size the stroke found them, where boxes that took in every
/// place the surface passed through stayed swollen to the far end.
#[test]
fn a_stroke_keeps_both_surfaces_in_its_boxes_and_lets_go_of_the_rest() {
    let mut mesh = primitives::build(Primitive::Sphere, 20_000, 0.5);
    let found: Vec<Aabb> = (0..mesh.bvh.leaves.len() as u32).map(|l| mesh.bvh.leaf_box(l)).collect();
    let centre = Vec3::new(0.0, 0.0, 0.5);
    let mut leaves = Vec::new();
    mesh.bvh.leaves_in_sphere(centre, 0.15, &mut leaves);
    leaves.sort_unstable();
    mesh.begin_stroke();
    let steps: Vec<f32> = (0..20).map(|_| 0.05).chain((0..20).map(|_| -0.05)).collect();
    for (i, &step) in steps.iter().enumerate() {
        mesh.move_vertices(&leaves, |_, vt| {
            let w = (1.0 - vt.start.distance(centre) / 0.15).max(0.0);
            (w > 0.0).then(|| vt.pos + Vec3::Z * (step * w))
        });
        mesh.positions_changed(&leaves);
        check(&mesh, &format!("step {i}"));
    }
    // Home again, give or take rounding: each box is the one the stroke
    // found, widened by no more than the rounding.
    for &l in &leaves {
        let (now, then) = (mesh.bvh.leaf_box(l), found[l as usize]);
        let off = (now.max - then.max).abs().max((now.min - then.min).abs()).max_element();
        assert!(off < 1e-4, "leaf {l} is still swollen: {now:?} against {then:?}");
    }
    let _ = mesh.end_stroke(false);
    check(&mesh, "after the stroke");
}

/// What dynamic topology has a leaf take in during a stroke stays in its box
/// through the refits after, and a stroke begun later starts from scratch.
#[test]
fn what_a_leaf_is_made_to_take_in_stays_for_the_stroke() {
    let mut mesh = primitives::build(Primitive::Sphere, 5_000, 0.5);
    let far = Aabb { min: Vec3::splat(2.0), max: Vec3::splat(2.5) };
    mesh.begin_stroke();
    let leaf = 3u32;
    mesh.positions_changed(&[leaf]);
    mesh.bvh.enclose(leaf, far);
    mesh.positions_changed(&[leaf]);
    assert!(within(&far, &mesh.bvh.leaf_box(leaf)), "the refit dropped what the leaf was made to take in");
    let _ = mesh.end_stroke(false);
    mesh.begin_stroke();
    mesh.positions_changed(&[leaf]);
    mesh.positions_changed(&[leaf]);
    let _ = mesh.end_stroke(false);
    assert!(!within(&far, &mesh.bvh.leaf_box(leaf)), "a finished stroke's box is exact again");
}
