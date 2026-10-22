//! The tree built on every thread is the tree that was built on one: the
//! same nodes, leaves, vertex numbering, neighbours and adjacency, number for
//! number. The one-thread build is kept here, as it was before 2026-10-21,
//! to be compared against. Anything that differed would change where every
//! brush's arithmetic lands, so "the same shape" is not enough.

use glam::Vec3;

use super::*;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

enum Tmp {
    Leaf { start: usize, end: usize },
    Inner(Box<Tmp>, Box<Tmp>),
}

fn flatten(tmp: &Tmp, parent: u32, nodes: &mut Vec<Node>, leaves: &mut Vec<Leaf>) -> u32 {
    let idx = nodes.len() as u32;
    nodes.push(Node { bounds: Aabb::EMPTY, a: 0, b: 0, parent });
    match tmp {
        Tmp::Leaf { start, end } => {
            nodes[idx as usize].a = leaves.len() as u32 | LEAF_BIT;
            leaves.push(Leaf { tri_start: *start as u32, tri_end: *end as u32, vert_start: 0, vert_end: 0, node: idx });
        }
        Tmp::Inner(l, r) => {
            let a = flatten(l, idx, nodes, leaves);
            let b = flatten(r, idx, nodes, leaves);
            nodes[idx as usize].a = a;
            nodes[idx as usize].b = b;
        }
    }
    idx
}

fn reference_rec(ids: &mut [u32], base: usize, centroids: &[Vec3]) -> Tmp {
    let n = ids.len();
    if n <= LEAF_MAX_TRIS {
        return Tmp::Leaf { start: base, end: base + n };
    }
    let mut b = Aabb::EMPTY;
    for &t in ids.iter() {
        b.grow(centroids[t as usize]);
    }
    let ext = b.size();
    let axis = if ext.x >= ext.y && ext.x >= ext.z {
        0
    } else if ext.y >= ext.z {
        1
    } else {
        2
    };
    let mid = n / 2;
    ids.select_nth_unstable_by(mid, |&x, &y| centroids[x as usize][axis].total_cmp(&centroids[y as usize][axis]));
    let (l, r) = ids.split_at_mut(mid);
    let (lt, rt) = if n > 16_384 {
        rayon::join(|| reference_rec(l, base, centroids), || reference_rec(r, base + mid, centroids))
    } else {
        (reference_rec(l, base, centroids), reference_rec(r, base + mid, centroids))
    };
    Tmp::Inner(Box::new(lt), Box::new(rt))
}

fn reference(positions: &[Vec3], indices: &[u32]) -> Built {
    let tri_count = indices.len() / 3;
    let centroids: Vec<Vec3> = indices
        .chunks_exact(3)
        .map(|t| (positions[t[0] as usize] + positions[t[1] as usize] + positions[t[2] as usize]) * (1.0 / 3.0))
        .collect();
    let mut ids: Vec<u32> = (0..tri_count as u32).collect();
    let mut nodes = Vec::new();
    let mut leaves = Vec::new();
    if tri_count > 0 {
        let tmp = reference_rec(&mut ids, 0, &centroids);
        flatten(&tmp, NO_PARENT, &mut nodes, &mut leaves);
    }
    let mut vert_remap = vec![u32::MAX; positions.len()];
    let mut new_indices = Vec::with_capacity(indices.len());
    let mut next = 0u32;
    for leaf in &mut leaves {
        leaf.vert_start = next;
        for &t in &ids[leaf.tri_start as usize..leaf.tri_end as usize] {
            for c in 0..3 {
                let old = indices[t as usize * 3 + c] as usize;
                if vert_remap[old] == u32::MAX {
                    vert_remap[old] = next;
                    next += 1;
                }
                new_indices.push(vert_remap[old]);
            }
        }
        leaf.vert_end = next;
    }
    let mut new_positions = vec![Vec3::ZERO; next as usize];
    for (old, &new) in vert_remap.iter().enumerate() {
        if new != u32::MAX {
            new_positions[new as usize] = positions[old];
        }
    }
    let mut vert_leaf = vec![0u32; next as usize];
    for (li, leaf) in leaves.iter().enumerate() {
        vert_leaf[leaf.vert_start as usize..leaf.vert_end as usize].fill(li as u32);
    }
    let mut pairs: Vec<(u32, u32)> = Vec::new();
    for (li, leaf) in leaves.iter().enumerate() {
        let li = li as u32;
        for &v in &new_indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3] {
            let owner = vert_leaf[v as usize];
            if owner != li {
                pairs.push((li, owner));
                pairs.push((owner, li));
            }
        }
    }
    pairs.sort_unstable();
    pairs.dedup();
    let mut nbr_off = vec![0u32; leaves.len() + 1];
    for &(l, _) in &pairs {
        nbr_off[l as usize + 1] += 1;
    }
    for i in 0..leaves.len() {
        nbr_off[i + 1] += nbr_off[i];
    }
    let nbr = Lists::from_offsets(&nbr_off, pairs.iter().map(|&(_, o)| o).collect());
    let stamp = vec![0u32; nodes.len()];
    let mut bvh = Bvh { nodes, leaves, nbr, vert_leaf, stamp, ..Default::default() };
    let all: Vec<u32> = (0..bvh.leaves.len() as u32).collect();
    bvh.refit_leaves(&all, &new_positions, &new_indices, false);
    Built { bvh, positions: new_positions, indices: new_indices, vert_remap, tri_order: ids }
}

/// The vertex-triangle and vertex-vertex lists as the one-thread build made
/// them (`Mesh::build_adjacency` before 2026-10-21).
fn reference_adjacency(n: usize, indices: &[u32]) -> (Lists, Lists) {
    let mut off = vec![0u32; n + 1];
    for &v in indices {
        off[v as usize + 1] += 1;
    }
    for i in 0..n {
        off[i + 1] += off[i];
    }
    let mut cursor = off.clone();
    let mut fans = vec![0u32; indices.len()];
    for (i, &v) in indices.iter().enumerate() {
        let c = &mut cursor[v as usize];
        fans[*c as usize] = (i / 3) as u32;
        *c += 1;
    }
    let ring = |v: usize, buf: &mut Vec<u32>| {
        buf.clear();
        for &t in &fans[off[v] as usize..off[v + 1] as usize] {
            for &o in &indices[t as usize * 3..t as usize * 3 + 3] {
                if o as usize != v {
                    buf.push(o);
                }
            }
        }
        buf.sort_unstable();
        buf.dedup();
    };
    let counts: Vec<u32> = (0..n)
        .into_par_iter()
        .map_init(Vec::new, |buf, v| {
            ring(v, buf);
            buf.len() as u32
        })
        .collect();
    let mut vv_off = vec![0u32; n + 1];
    for i in 0..n {
        vv_off[i + 1] = vv_off[i] + counts[i];
    }
    let vv: Vec<u32> = (0..n)
        .into_par_iter()
        .map_init(Vec::new, |buf, v| {
            ring(v, buf);
            buf.clone()
        })
        .flatten_iter()
        .collect();
    (Lists::from_offsets(&off, fans), Lists::from_offsets(&vv_off, vv))
}

fn same_box(a: &Aabb, b: &Aabb) -> bool {
    a.min.to_array().map(f32::to_bits) == b.min.to_array().map(f32::to_bits)
        && a.max.to_array().map(f32::to_bits) == b.max.to_array().map(f32::to_bits)
}

fn assert_same(what: &str, new: &Built, old: &Built) {
    assert_eq!(new.tri_order, old.tri_order, "{what}: triangle order");
    assert_eq!(new.indices, old.indices, "{what}: triangles");
    assert_eq!(new.vert_remap, old.vert_remap, "{what}: vertex numbering");
    let bits = |p: &[Vec3]| p.iter().flat_map(|v| v.to_array().map(f32::to_bits)).collect::<Vec<_>>();
    assert_eq!(bits(&new.positions), bits(&old.positions), "{what}: positions");
    let (a, b) = (&new.bvh, &old.bvh);
    assert_eq!(a.nodes.len(), b.nodes.len(), "{what}: node count");
    for (i, (x, y)) in a.nodes.iter().zip(&b.nodes).enumerate() {
        assert!((x.a, x.b, x.parent) == (y.a, y.b, y.parent) && same_box(&x.bounds, &y.bounds), "{what}: node {i}");
    }
    assert_eq!(a.leaves.len(), b.leaves.len(), "{what}: leaf count");
    for (i, (x, y)) in a.leaves.iter().zip(&b.leaves).enumerate() {
        let runs = |l: &Leaf| (l.tri_start, l.tri_end, l.vert_start, l.vert_end, l.node);
        assert_eq!(runs(x), runs(y), "{what}: leaf {i}");
        assert_eq!(a.leaf_neighbours(i as u32), b.leaf_neighbours(i as u32), "{what}: neighbours of leaf {i}");
    }
    assert_eq!(a.vert_leaf, b.vert_leaf, "{what}: owning leaves");
}

/// Builds `positions` and `indices` both ways, and the sculpt mesh from them,
/// and holds every number to the old build's.
fn check(what: &str, positions: Vec<Vec3>, indices: Vec<u32>) {
    let n_in = positions.len() as u32;
    let sound: Vec<u32> = indices.chunks_exact(3).filter(|t| super::super::sound(t, n_in)).flatten().copied().collect();
    let old = reference(&positions, &sound);
    assert_same(what, &Bvh::build(&positions, &sound), &old);
    let mesh = Mesh::new(positions, indices, None);
    assert_eq!(mesh.indices, old.indices, "{what}: the sculpt mesh's triangles");
    let (vt, vv) = reference_adjacency(mesh.positions.len(), &mesh.indices);
    for v in 0..mesh.positions.len() {
        assert_eq!(mesh.vt.row(v), vt.row(v), "{what}: triangles round vertex {v}");
        assert_eq!(mesh.vv.row(v), vv.row(v), "{what}: neighbours of vertex {v}");
    }
}

/// A small deterministic generator, so a failure can be run again.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[test]
fn the_tree_has_the_size_its_triangle_count_says() {
    for n in (1..2_000).chain([4_095, 4_096, 4_097, 65_536, 1_000_003]) {
        let (here, next) = leaf_counts(n);
        let count = |n: usize| {
            let mut ids: Vec<u32> = (0..n as u32).collect();
            let centroids: Vec<Vec3> = (0..n).map(|i| Vec3::new(i as f32, 0.0, 0.0)).collect();
            let mut leaves = Vec::new();
            flatten(&reference_rec(&mut ids, 0, &centroids), NO_PARENT, &mut Vec::new(), &mut leaves);
            leaves.len()
        };
        if n < 70 || n % 97 == 0 || n > 4_000 {
            assert_eq!((here, next), (count(n), count(n + 1)), "{n} triangles");
        }
    }
}

#[test]
fn a_ball_is_built_as_before() {
    for triangles in [2_000, 400_000] {
        let ball = primitives::build(Primitive::Sphere, triangles, 0.5);
        check(&format!("ball of {triangles}"), ball.positions.clone(), ball.indices.clone());
    }
}

/// A voxel remesh lays its surface on a lattice: centroids tie on every axis
/// by the thousand, which is where a selection made another way would part
/// from the old one.
#[test]
fn a_remeshed_surface_full_of_ties_is_built_as_before() {
    let ball = primitives::build(Primitive::Sphere, 20_000, 0.5);
    let settings = sculpt_volume::RemeshSettings::new(320);
    let rebuilt = sculpt_volume::remesh_with(&ball.positions, &ball.indices, &settings).expect("remesh");
    assert!(rebuilt.mesh.triangle_count() > 200_000, "{} triangles", rebuilt.mesh.triangle_count());
    check("remeshed ball", rebuilt.mesh.positions, rebuilt.mesh.indices);
}

/// Triangles in no order, some of them twice over or broken, and vertices
/// no triangle uses: every way a file can come in.
#[test]
fn a_shuffled_soup_with_broken_triangles_is_built_as_before() {
    let ball = primitives::build(Primitive::Sphere, 300_000, 0.5);
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15);
    let mut triangles: Vec<[u32; 3]> = ball.indices.chunks_exact(3).map(|t| [t[0], t[1], t[2]]).collect();
    for i in (1..triangles.len()).rev() {
        triangles.swap(i, (rng.next() % (i as u64 + 1)) as usize);
    }
    let count = triangles.len();
    for k in 0..count / 50 {
        let twin = triangles[k * 7 % count];
        triangles.push(twin);
    }
    triangles.push([0, 0, 1]);
    triangles.push([1, 2, u32::MAX]);
    let mut positions = ball.positions.clone();
    positions.extend((0..100).map(|i| Vec3::splat(i as f32)));
    check("soup", positions, triangles.into_iter().flatten().collect());
}

/// Old against new, stage by stage, back to back in one process, best of
/// five: `cargo test --release --bin sculpt -- build_tests::timing --ignored --nocapture`.
#[test]
#[ignore]
fn timing() {
    use std::time::Instant;
    let ball = primitives::build(Primitive::Sphere, 200_000, 0.5);
    let resolution = std::env::var("SCULPT_TIMING_RESOLUTION").ok().and_then(|r| r.parse().ok()).unwrap_or(1024);
    let rebuilt = sculpt_volume::remesh_with(&ball.positions, &ball.indices, &sculpt_volume::RemeshSettings::new(resolution))
        .expect("remesh");
    let (positions, indices) = (rebuilt.mesh.positions, rebuilt.mesh.indices);
    println!("{} triangles, {} threads", indices.len() / 3, rayon::current_num_threads());
    let centroids: Vec<Vec3> = indices
        .chunks_exact(3)
        .map(|t| (positions[t[0] as usize] + positions[t[1] as usize] + positions[t[2] as usize]) * (1.0 / 3.0))
        .collect();
    let time = |f: &mut dyn FnMut()| {
        let t = Instant::now();
        f();
        t.elapsed().as_secs_f64()
    };
    let mut best = [[f64::MAX; 2]; 4];
    for _ in 0..5 {
        for new in [false, true] {
            let k = usize::from(new);
            let mut ids: Vec<u32> = (0..centroids.len() as u32).collect();
            let tree = time(&mut || {
                if new {
                    let count = leaf_counts(ids.len()).0;
                    let mut nodes = vec![Node { bounds: Aabb::EMPTY, a: 0, b: 0, parent: NO_PARENT }; 2 * count - 1];
                    let mut leaves = vec![Leaf { tri_start: 0, tri_end: 0, vert_start: 0, vert_end: 0, node: 0 }; count];
                    let top = Place { node: 0, leaf: 0, parent: NO_PARENT };
                    build_rec(&mut ids, 0, &centroids, top, &SharedMut::new(&mut nodes), &SharedMut::new(&mut leaves));
                } else {
                    let tmp = reference_rec(&mut ids, 0, &centroids);
                    flatten(&tmp, NO_PARENT, &mut Vec::new(), &mut Vec::new());
                }
            });
            best[0][k] = best[0][k].min(tree);
            let renumber = time(&mut || {
                let _ = if new { Bvh::build(&positions, &indices) } else { reference(&positions, &indices) };
            });
            best[1][k] = best[1][k].min(renumber - tree);
            let mut mesh = Mesh::new(positions.clone(), indices.clone(), None);
            let adjacency = time(&mut || {
                if new {
                    mesh.build_adjacency();
                } else {
                    let _ = reference_adjacency(mesh.positions.len(), &mesh.indices);
                }
            });
            best[2][k] = best[2][k].min(adjacency);
            let whole = time(&mut || {
                let _ = Mesh::new(positions.clone(), indices.clone(), None);
            });
            best[3][k] = best[3][k].min(whole);
        }
    }
    for (name, [old, new]) in ["tree", "rest of Bvh::build", "adjacency", "Mesh::new (new only)"].iter().zip(best) {
        println!("{name:>22}: old {old:.3} s, new {new:.3} s");
    }
}
