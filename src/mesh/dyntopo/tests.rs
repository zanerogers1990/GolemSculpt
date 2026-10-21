//! Dynamic topology rewrites the triangles in the middle of a stroke, and the
//! failures that matter are silent: an edge with three faces, a triangle
//! turned inside out, adjacency that no longer matches the triangles, a leaf
//! of the spatial index that no longer reaches one of its vertices, an undo
//! that leaves one stray index behind. So the assertions here are structural
//! and exact, and every one that could pass vacuously first checks that the
//! refine actually did something.
//!
//! Most run on the Unity app's own fixture, a lumpy icosphere that is its own
//! mirror image: closed, so "every edge has exactly two triangles" is a real
//! assertion, and uneven, so nothing passes by an accident of regularity.

use std::collections::{HashMap, HashSet};

use glam::{Vec2, Vec3};

use super::*;
use crate::camera::{Camera, View};
use crate::history::History;
use crate::mesh::primitives::{self, Primitive};
use crate::mesh::{DirtyRanges, StrokeDelta};
use crate::scene::Scene;
use crate::sculpt::{Brush, PointerSample, Sculptor, dyntopo};

/// Icosphere subdivisions: edges of about 0.033 on a ball of radius 0.5.
const SUBDIVISIONS: usize = 4;
const REGION_RADIUS: f32 = 0.15;
const MAX_EDGE: f32 = 0.012;
const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

// ---------------------------------------------------------------- fixtures

/// How far the fixture's surface is from the centre in direction `dir`: a
/// ball of radius 0.5 with soft lumps, the same in all eight octants.
fn surface_radius(dir: Vec3) -> f32 {
    let d = dir.abs();
    0.5 * (1.0
        + 0.04 * (d.x * 23.0 + 0.4).sin() * (d.y * 19.0 + 1.3).sin() * (d.z * 17.0 + 0.7).sin()
        + 0.015 * (d.x * 71.0 + d.z * 53.0 + d.y * 11.0).sin())
}

fn on_surface(dir: Vec3) -> Vec3 {
    let dir = dir.normalize();
    dir * surface_radius(dir)
}

/// The Unity tests' mesh: an icosphere pushed out to `surface_radius`, each
/// vertex jittered a little along its own direction, mirror-exact across all
/// three planes. Positions and triangles as plain arrays.
fn icosphere_arrays(subdivisions: usize) -> (Vec<Vec3>, Vec<u32>) {
    let t = (1.0 + 5f32.sqrt()) * 0.5;
    let mut verts: Vec<Vec3> = [
        [-1.0, t, 0.0],
        [1.0, t, 0.0],
        [-1.0, -t, 0.0],
        [1.0, -t, 0.0],
        [0.0, -1.0, t],
        [0.0, 1.0, t],
        [0.0, -1.0, -t],
        [0.0, 1.0, -t],
        [t, 0.0, -1.0],
        [t, 0.0, 1.0],
        [-t, 0.0, -1.0],
        [-t, 0.0, 1.0],
    ]
    .into_iter()
    .map(|p| Vec3::from_array(p).normalize())
    .collect();
    let mut tris: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8, 3, 9, 4, 3,
        4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];
    for _ in 0..subdivisions {
        let mut midpoints: HashMap<(u32, u32), u32> = HashMap::new();
        let mut next = Vec::with_capacity(tris.len() * 4);
        for f in tris.chunks_exact(3) {
            let mut mid = |i: u32, j: u32| {
                *midpoints.entry((i.min(j), i.max(j))).or_insert_with(|| {
                    verts.push((verts[i as usize] + verts[j as usize]).normalize());
                    verts.len() as u32 - 1
                })
            };
            let (a, b, c) = (f[0], f[1], f[2]);
            let (ab, bc, ca) = (mid(a, b), mid(b, c), mid(c, a));
            next.extend_from_slice(&[a, ab, ca, b, bc, ab, c, ca, bc, ab, bc, ca]);
        }
        tris = next;
    }
    let edge = 1.0515 * 0.5 / (1 << subdivisions) as f32;
    for v in &mut verts {
        let u = *v;
        let hash = (u.x.abs() * 12.9898 + u.y.abs() * 78.233 + u.z.abs() * 37.719).sin() * 43758.547;
        let jitter = ((hash - hash.floor()) * 2.0 - 1.0) * 0.2 * edge;
        *v = u * (surface_radius(u) + jitter);
    }
    (verts, tris)
}

fn icosphere(subdivisions: usize) -> Mesh {
    let (positions, indices) = icosphere_arrays(subdivisions);
    Mesh::new(positions, indices, None)
}

fn fixture() -> Mesh {
    icosphere(SUBDIVISIONS)
}

fn region_direction() -> Vec3 {
    Vec3::new(0.2, 0.3, 1.0).normalize()
}

fn region_centre() -> Vec3 {
    on_surface(region_direction())
}

fn refine_at(mesh: &mut Mesh, centre: Vec3) -> RefineStats {
    mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 1_000_000, 10_000_000)
}

fn view_of(mesh: &Mesh) -> View {
    let mut camera = Camera::default();
    camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
    camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
}

/// A sculptor holding `brush` at a fixed size on the model, with dynamic
/// topology on at the Unity app's detail.
fn refining_sculptor(brush: Brush, radius: f32) -> Sculptor {
    let mut sculptor = Sculptor::default();
    let s = &mut sculptor.settings;
    s.brush = brush;
    s.screen_space_size = false;
    s.radius_world = radius;
    s.per_brush[brush.index()].strength = 0.5;
    sculptor.dyntopo.enabled = true;
    sculptor.dyntopo.detail = 0.15;
    sculptor
}

/// Switches the sculptor's strokes to mirror across X.
fn mirror_x(sculptor: &mut Sculptor) {
    sculptor.settings.symmetry = [true, false, false];
}

/// Drags the pointer over the points of the model's front nearest `path`
/// (points in the model's space, seen from the camera), a few samples a frame.
fn run_stroke(mesh: &mut Mesh, sculptor: &mut Sculptor, view: &View, path: &[Vec3]) -> Option<StrokeDelta> {
    let sample = |i: usize| PointerSample {
        pos: view.project(path[i]).expect("the path is in front of the camera"),
        pressure: None,
        time: i as f64 / 240.0,
    };
    assert!(sculptor.begin(mesh, view, sample(0), false, false), "stroke missed the mesh");
    for i in 1..path.len() {
        sculptor.sample(mesh, view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(mesh, view, 1.0 / 60.0);
            let _ = mesh.take_dirty();
        }
    }
    let delta = sculptor.end(mesh, view);
    let _ = mesh.take_dirty();
    delta
}

/// A path across the camera-facing side of the fixture, from x = `from` to
/// x = `to` at height `y`.
fn across(from: f32, to: f32, y: f32, steps: usize) -> Vec<Vec3> {
    (0..=steps).map(|i| on_surface(Vec3::new(from + (to - from) * i as f32 / steps as f32, y, 0.45))).collect()
}

/// Pushes every vertex within `radius` of `centre` outward, the way a brush
/// writes, inside the stroke under way.
fn push(mesh: &mut Mesh, centre: Vec3, amount: f32) {
    let radius = 0.1;
    let mut leaves = Vec::new();
    mesh.bvh.leaves_in_sphere(centre, radius, &mut leaves);
    leaves.sort_unstable();
    mesh.move_vertices(&leaves, |_, vertex| {
        let d = vertex.pos.distance(centre);
        (d < radius).then(|| vertex.pos + vertex.pos.normalize() * amount * (1.0 - d / radius))
    });
    mesh.positions_changed(&leaves);
}

// ---------------------------------------------------------------- checks

/// Every edge is used exactly twice, once each way: no hole, no fold, no
/// edge with three faces, no triangle with a corner twice or past the count.
pub(crate) fn assert_closed_manifold(mesh: &Mesh) {
    let count = mesh.vertex_count() as u32;
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        assert!(c[0] != c[1] && c[1] != c[2] && c[0] != c[2], "degenerate triangle {t}");
        assert!(c.iter().all(|&v| v < count), "triangle {t} names a vertex past the count");
        for k in 0..3 {
            *edges.entry((c[k], c[(k + 1) % 3])).or_default() += 1;
        }
    }
    for (&(a, b), &n) in &edges {
        assert_eq!(n, 1, "edge {a}-{b} runs the same way twice: inconsistent winding or a fold");
        assert!(edges.contains_key(&(b, a)), "edge {a}-{b} has only one triangle: a hole");
    }
}

/// How many edges have exactly `faces` triangles (3 means three or more).
fn count_edges(mesh: &Mesh, faces: u32) -> usize {
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for c in mesh.indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    edges.values().filter(|&&n| if faces >= 3 { n >= 3 } else { n == faces }).count()
}

/// The lists dynamic topology keeps up split by split say what a fresh build
/// from the triangles says.
fn assert_adjacency_is_current(mesh: &Mesh) {
    let count = mesh.vertex_count();
    let mut triangles: Vec<Vec<u32>> = vec![Vec::new(); count];
    let mut neighbours: Vec<HashSet<u32>> = vec![HashSet::new(); count];
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        for k in 0..3 {
            triangles[c[k] as usize].push(t as u32);
            neighbours[c[k] as usize].extend([c[(k + 1) % 3], c[(k + 2) % 3]]);
        }
    }
    assert_eq!(mesh.vt.len(), count);
    assert_eq!(mesh.vv.len(), count);
    for v in 0..count {
        let mut kept = mesh.vt.row(v).to_vec();
        kept.sort_unstable();
        assert_eq!(kept, triangles[v], "triangles of vertex {v}");
        let kept: HashSet<u32> = mesh.vv.row(v).iter().copied().collect();
        assert_eq!(kept.len(), mesh.vv.row(v).len(), "a neighbour of vertex {v} is listed twice");
        assert_eq!(kept, neighbours[v], "neighbours of vertex {v}");
    }
}

/// Everything the brushes and the pointer rely on in the spatial index: a
/// sound tree, every triangle and every vertex inside the box of the leaf
/// that has it, every per-vertex list the right length, and every pair of
/// leaves that share a vertex known to each other.
fn assert_index_is_sound(mesh: &Mesh) {
    let bvh = &mesh.bvh;
    bvh.assert_sound();
    let count = mesh.vertex_count();
    assert_eq!(mesh.indices.len() % 3, 0);
    assert_eq!(bvh.leaves.last().map_or(0, |l| l.tri_end as usize), mesh.triangle_count());
    for list in [mesh.normals.len(), mesh.mask.len(), mesh.orig_pos.len(), mesh.orig_nrm.len()] {
        assert_eq!(list, count);
    }
    assert_eq!((mesh.orig_mask.len(), mesh.stamp.len(), bvh.vert_leaf.len()), (count, count, count));
    let inside = |b: Aabb, p: Vec3| p.cmpge(b.min - 1e-6).all() && p.cmple(b.max + 1e-6).all();
    let mut owned = 0;
    for (number, leaf) in bvh.leaves.iter().enumerate() {
        let around = bvh.leaf_box(number as u32);
        for v in leaf.vert_start..leaf.vert_end {
            assert_eq!(bvh.vert_leaf[v as usize], number as u32);
            assert!(inside(around, mesh.positions[v as usize]), "vertex {v} is outside leaf {number}");
            assert!(inside(around, mesh.start_pos(v)), "vertex {v} started outside leaf {number}");
            owned += 1;
        }
        for &v in &mesh.indices[leaf.tri_start as usize * 3..leaf.tri_end as usize * 3] {
            assert!(inside(around, mesh.positions[v as usize]), "a corner is outside leaf {number}");
            assert!(inside(around, mesh.start_pos(v)), "a corner started outside leaf {number}");
            let owner = bvh.vert_leaf[v as usize];
            assert!(
                owner == number as u32 || bvh.leaf_neighbours(number as u32).contains(&owner),
                "leaf {number} uses vertex {v} of leaf {owner} without knowing it for a neighbour"
            );
        }
    }
    assert_eq!(owned, count, "every vertex belongs to one leaf");
    if mesh.any_hidden() {
        assert_eq!(mesh.hidden_triangles().unwrap().len(), mesh.triangle_count());
        assert_eq!(mesh.hidden_vertices().unwrap().len(), count);
    }
}

pub(crate) fn assert_sound(mesh: &Mesh) {
    assert_adjacency_is_current(mesh);
    assert_index_is_sound(mesh);
}

/// The smallest angle of a triangle, in degrees.
fn smallest_angle(a: Vec3, b: Vec3, c: Vec3) -> f32 {
    let at = |p: Vec3, q: Vec3, r: Vec3| (q - p).angle_between(r - p).to_degrees();
    at(a, b, c).min(at(b, c, a)).min(at(c, a, b))
}

fn corners_of(mesh: &Mesh, triangle: &[u32]) -> [Vec3; 3] {
    [0, 1, 2].map(|k| mesh.positions[triangle[k] as usize])
}

/// How far the segment a-b passes from `p`.
fn segment_distance(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let along = b - a;
    let t = ((p - a).dot(along) / along.length_squared()).clamp(0.0, 1.0);
    (a + along * t).distance(p)
}

/// The nearest point where a ray meets any triangle of the mesh, the slow
/// and certain way.
fn brute_force_hit(mesh: &Mesh, origin: Vec3, dir: Vec3) -> Option<Vec3> {
    mesh.indices
        .chunks_exact(3)
        .filter_map(|t| {
            let [a, b, c] = corners_of(mesh, t);
            sculpt_geom::tri::ray_hit(origin, dir, a, b, c)
        })
        .min_by(f32::total_cmp)
        .map(|along| origin + dir * along)
}

/// A stand-in for the renderer's copy of a mesh: it takes exactly what
/// `take_dirty` says changed, as `Renderer::update_mesh` does, so anything a
/// refine changed without saying so stays stale here and shows up as a
/// difference. New room is filled with values no mesh has.
struct Sent {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    mask: Vec<f32>,
    indices: Vec<u32>,
    topology: u64,
}

impl Sent {
    fn new(mesh: &mut Mesh) -> Sent {
        let _ = mesh.take_dirty();
        Sent {
            positions: mesh.positions.clone(),
            normals: mesh.normals.clone(),
            mask: mesh.mask.clone(),
            indices: mesh.indices.clone(),
            topology: mesh.topology_version(),
        }
    }

    /// Returns how many triangles were sent again.
    fn follow(&mut self, mesh: &mut Mesh) -> usize {
        let dirty: DirtyRanges = mesh.take_dirty();
        let count = mesh.vertex_count();
        self.positions.resize(count, Vec3::splat(f32::MAX));
        self.normals.resize(count, Vec3::splat(f32::MAX));
        self.mask.resize(count, f32::MAX);
        let span = |r: &std::ops::Range<u32>| r.start as usize..r.end as usize;
        for r in &dirty.positions {
            self.positions[span(r)].copy_from_slice(&mesh.positions[span(r)]);
        }
        for r in &dirty.normals {
            self.normals[span(r)].copy_from_slice(&mesh.normals[span(r)]);
        }
        for r in &dirty.mask {
            self.mask[span(r)].copy_from_slice(&mesh.mask[span(r)]);
        }
        let mut sent = 0;
        if self.topology != mesh.topology_version() {
            self.topology = mesh.topology_version();
            self.indices.resize(mesh.indices.len(), u32::MAX);
            for r in &dirty.triangles {
                let corners = r.start as usize * 3..r.end as usize * 3;
                self.indices[corners.clone()].copy_from_slice(&mesh.indices[corners]);
                sent += (r.end - r.start) as usize;
            }
        } else {
            assert!(dirty.triangles.is_empty(), "triangles to send without a change of topology");
        }
        sent
    }

    fn assert_matches(&self, mesh: &Mesh, when: &str) {
        assert!(self.indices == mesh.indices, "{when}: the faces drawn are not the mesh's");
        assert!(self.positions == mesh.positions, "{when}: vertices are drawn somewhere else");
        assert!(self.normals == mesh.normals, "{when}: stale normals");
        assert!(self.mask == mesh.mask, "{when}: stale mask");
    }
}

// ---------------------------------------------------------------- structure

#[test]
fn refine_adds_detail_under_the_brush_and_nowhere_else() {
    let mut mesh = fixture();
    let (before, centre) = (mesh.vertex_count(), region_centre());
    let stats = refine_at(&mut mesh, centre);
    assert!(stats.splits > 100, "the refine did nothing: every other assertion would be vacuous");
    assert_eq!(mesh.vertex_count(), before + stats.vertices_added);
    assert_eq!(stats.vertices_added, stats.splits, "a closed mesh gains one vertex per split");
    assert_eq!(mesh.triangle_count(), 2 * mesh.vertex_count() - 4, "still a closed surface with no handles");
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    // New vertices only near the region: within its radius plus two original
    // edges (a split edge can reach one original edge length past the sphere
    // it passes through, and longest-edge bisection may first split the
    // longer edge of the triangle beyond it: the one ring of grading between
    // the fine region and the coarse mesh).
    for v in before..mesh.vertex_count() {
        assert!(mesh.positions[v].distance(centre) < REGION_RADIUS + 0.07, "vertex {v}");
        assert!(mesh.split_parents(v as u32).is_some());
    }
    assert!(mesh.split_parents(before as u32 - 1).is_none());

    // Converged: no qualifying edge left inside the region.
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        let p = corners_of(&mesh, c);
        for k in 0..3 {
            let (a, b) = (p[k], p[(k + 1) % 3]);
            if segment_distance(centre, a, b) <= REGION_RADIUS {
                assert!(a.distance(b) <= MAX_EDGE * 1.0001, "an edge was left long in triangle {t}");
            }
        }
    }
    // And asking again finds nothing to do.
    let again = refine_at(&mut mesh, centre);
    assert_eq!((again.splits, again.flips, again.vertices_added), (0, 0, 0));
}

/// Triangle SHAPE, on the surface that used to go wrong: one stretched to
/// three times its length along X, the way Move and Snake Hook leave a
/// region. Splitting edges in plain length order bisected the triangle
/// beyond each edge across its short side and fanned needles out of every
/// original vertex along the stretch (7.5% of refined triangles under 10
/// degrees, 27% under 20, in the Unity app); with longest-edge bisection and
/// the flips there are none under 10 and under 2% under 20.
#[test]
fn refined_triangles_stay_well_shaped_on_a_stretched_surface() {
    let (mut positions, indices) = icosphere_arrays(SUBDIVISIONS);
    for p in &mut positions {
        p.x *= 3.0;
    }
    let mut mesh = Mesh::new(positions, indices, None);
    // A stroke's worth of sites along the stretch, on the camera-facing side.
    let sites: Vec<Vec3> = (0..=24)
        .map(|s| {
            let x = -0.9 + 1.8 * s as f32 / 24.0;
            mesh.raycast(Vec3::new(x, 0.1, 2.0), Vec3::NEG_Z).expect("the ray meets the stretched ball").pos
        })
        .collect();
    let before = mesh.vertex_count() as u32;
    assert!(mesh.refine(&sites, 0.115, 0.015, 1_000_000, 10_000_000).splits > 1000);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    let (mut refined, mut under_10, mut under_20) = (0, 0, 0);
    for t in mesh.indices.chunks_exact(3) {
        if t.iter().all(|&v| v < before) {
            continue;
        }
        let [a, b, c] = corners_of(&mesh, t);
        let smallest = smallest_angle(a, b, c);
        refined += 1;
        under_10 += usize::from(smallest < 10.0);
        under_20 += usize::from(smallest < 20.0);
    }
    println!("stretched: {refined} refined triangles, {under_10} under 10 degrees, {under_20} under 20");
    assert!(under_10 <= refined / 1000, "{under_10} of {refined} refined triangles under 10 degrees");
    assert!(under_20 <= refined / 20, "{under_20} of {refined} refined triangles under 20 degrees");
}

#[test]
fn no_triangle_turns_inside_out() {
    let mut mesh = fixture();
    assert!(refine_at(&mut mesh, region_centre()).splits > 0);
    for (t, c) in mesh.indices.chunks_exact(3).enumerate() {
        let [a, b, c] = corners_of(&mesh, c);
        assert!((b - a).cross(c - a).dot(a + b + c) > 0.0, "triangle {t} faces inward");
    }
}

#[test]
fn incremental_adjacency_matches_a_fresh_build() {
    let mut mesh = fixture();
    assert!(refine_at(&mut mesh, region_centre()).splits > 0);
    assert_adjacency_is_current(&mesh);
    assert!(refine_at(&mut mesh, region_centre() + Vec3::new(0.05, 0.0, 0.0)).splits > 0);
    assert_adjacency_is_current(&mesh);
    // The lists every brush reads agree with the test's own.
    for v in 0..mesh.vertex_count() as u32 {
        assert_eq!(mesh.neighbours(v).len(), mesh.vert_tris(v).len(), "a closed fan around vertex {v}");
    }
}

#[test]
fn raycasts_and_queries_follow_the_new_geometry() {
    let mut mesh = fixture();
    let (before, centre) = (mesh.vertex_count() as u32, region_centre());
    assert!(refine_at(&mut mesh, centre).splits > 0);

    // A brush finds vertices through the leaves its sphere touches.
    let mut leaves = Vec::new();
    mesh.bvh.leaves_in_sphere(centre, 0.05, &mut leaves);
    let found: Vec<u32> = leaves
        .iter()
        .flat_map(|&l| mesh.bvh.leaves[l as usize].vert_start..mesh.bvh.leaves[l as usize].vert_end)
        .filter(|&v| mesh.positions[v as usize].distance(centre) < 0.05)
        .collect();
    let all = (0..mesh.vertex_count() as u32).filter(|&v| mesh.positions[v as usize].distance(centre) < 0.05).count();
    assert!(found.iter().any(|&v| v >= before), "the spatial index does not know about the new vertices");
    assert_eq!(found.len(), all, "and it finds every vertex under the brush, each once");

    // Rays through the refined patch against brute force over every triangle.
    let dir = -region_direction();
    for r in 0..40 {
        let target = centre + Vec3::new((r as f32 * 1.3).sin(), (r as f32 * 2.1).cos(), 0.0) * 0.08;
        let origin = target - dir * 2.0;
        let (hit, sure) = (mesh.raycast(origin, dir), brute_force_hit(&mesh, origin, dir));
        assert_eq!(hit.is_some(), sure.is_some(), "ray {r}");
        if let (Some(hit), Some(sure)) = (hit, sure) {
            assert!(hit.pos.distance(sure) < 1e-4, "ray {r}");
        }
    }
}

/// What is DRAWN, not what the arrays hold. In the Unity app, growing the
/// index buffer replaced the vertex buffer behind the brush's back, and every
/// vertex added afterwards drew at the origin with every CPU-side assertion
/// still passing. Here the renderer is sent ranges, so the test is that the
/// ranges a refine reports are all of what it changed: several refines, each
/// followed by a brush-style move.
#[test]
fn what_the_renderer_is_sent_follows_every_refine() {
    let mut mesh = fixture();
    let mut sent = Sent::new(&mut mesh);
    let original = mesh.triangle_count();
    let mut resent = 0;
    for s in 0..10 {
        let turn = glam::Quat::from_euler(
            glam::EulerRot::XYZ,
            (s as f32 * 9.0 - 45.0).to_radians(),
            (s as f32 * 13.0 - 60.0).to_radians(),
            0.0,
        );
        let centre = on_surface(turn * region_direction());
        mesh.begin_stroke();
        assert!(refine_at(&mut mesh, centre).splits > 0, "refine {s} did nothing");
        push(&mut mesh, centre, 0.01);
        resent += sent.follow(&mut mesh);
        sent.assert_matches(&mesh, &format!("after refine {s}"));
        mesh.end_stroke(false).expect("the stroke changed the mesh");
        resent += sent.follow(&mut mesh);
        sent.assert_matches(&mesh, &format!("after stroke {s}"));
    }
    assert!(mesh.triangle_count() > original * 2, "the mesh never grew enough to mean anything");
    // Only what changed was sent again, not the whole mesh each time.
    assert!(resent < mesh.triangle_count() * 2, "{resent} triangles sent for a mesh of {}", mesh.triangle_count());
}

#[test]
fn a_fully_masked_surface_is_not_refined() {
    let mut mesh = fixture();
    mesh.mask.fill(1.0);
    let stats = refine_at(&mut mesh, region_centre());
    assert_eq!((stats.splits, stats.flips), (0, 0));
    assert_eq!(mesh.topology_version(), 0);
}

#[test]
fn new_vertices_interpolate_the_mask() {
    let mut mesh = fixture();
    let centre = region_centre();
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = if mesh.positions[v].x > centre.x { 0.8 } else { 0.0 };
    }
    let (before, mask_before) = (mesh.vertex_count(), mesh.mask.clone());
    assert!(refine_at(&mut mesh, centre).splits > 0);
    assert_eq!(&mesh.mask[..before], &mask_before[..], "the mask that was painted is untouched");
    let mut between = 0;
    for v in before..mesh.vertex_count() {
        let m = mesh.mask[v];
        assert!((0.0..=0.8).contains(&m), "vertex {v}");
        // Each is the mean of the two it was made from.
        let [a, b] = mesh.split_parents(v as u32).unwrap();
        assert_eq!(m, 0.5 * (mesh.mask[a as usize] + mesh.mask[b as usize]));
        between += usize::from(m > 0.0 && m < 0.8);
    }
    assert!(between > 0, "no split crossed the edge of the mask");
}

// ---------------------------------------------------------------- undo

/// The whole of a mesh that undo has to give back.
#[derive(Clone, PartialEq)]
struct Kept {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    mask: Vec<f32>,
}

fn kept(mesh: &Mesh) -> Kept {
    Kept { positions: mesh.positions.clone(), indices: mesh.indices.clone(), mask: mesh.mask.clone() }
}

#[test]
fn undo_truncates_exactly_and_redo_restores_exactly() {
    let mut mesh = fixture();
    let mut history = History::default();
    for v in (0..mesh.vertex_count()).step_by(7) {
        mesh.mask[v] = 0.5;
    }
    let centre = region_centre();
    let further = centre + Vec3::new(0.06, 0.02, 0.0);
    mesh.flush_normals();
    let (before, normals_before, leaves_before) = (kept(&mesh), mesh.normals.clone(), mesh.bvh.leaves.len());

    // One stroke: refine, move, refine further along, move again, as a brush would.
    mesh.begin_stroke();
    assert!(refine_at(&mut mesh, centre).splits > 0);
    push(&mut mesh, centre, 0.01);
    assert!(refine_at(&mut mesh, further).splits > 0);
    push(&mut mesh, further, 0.01);
    assert_index_is_sound(&mesh);
    let delta = mesh.end_stroke(false).expect("the stroke changed the mesh");
    assert!(matches!(&delta, StrokeDelta::Positions { topology: Some(_), .. }));
    assert!(delta.bytes() > 1000);
    history.push_stroke(delta);
    mesh.flush_normals();
    let (after, normals_after) = (kept(&mesh), mesh.normals.clone());
    assert!(after.positions.len() > before.positions.len() + 100);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    assert_eq!(history.undo(&mut mesh), Some(false), "the mesh itself is not swapped");
    assert!(kept(&mesh) == before, "undo did not give back the mesh as it was");
    assert_eq!(mesh.bvh.leaves.len(), leaves_before);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    mesh.flush_normals();
    for (now, then) in mesh.normals.iter().zip(&normals_before) {
        assert!(now.distance(*then) < 1e-6);
    }

    assert_eq!(history.redo(&mut mesh), Some(false));
    assert!(kept(&mesh) == after, "redo did not give back the mesh as the stroke left it");
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    mesh.flush_normals();
    for (now, then) in mesh.normals.iter().zip(&normals_after) {
        assert!(now.distance(*then) < 1e-6);
    }

    // And back again, then forward again: the record is good any number of times.
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == before);
    assert_eq!(history.redo(&mut mesh), Some(false));
    assert!(kept(&mesh) == after);
    assert_sound(&mesh);
}

#[test]
fn an_older_stroke_undoes_through_a_refined_one() {
    let mut mesh = fixture();
    let mut history = History::default();
    let (original, centre) = (kept(&mesh), region_centre());

    // A plain stroke first, then a refining one, then a plain one on top.
    mesh.begin_stroke();
    push(&mut mesh, centre, 0.01);
    history.push_stroke(mesh.end_stroke(false).unwrap());
    let plain = kept(&mesh);
    mesh.begin_stroke();
    assert!(refine_at(&mut mesh, centre).splits > 0);
    push(&mut mesh, centre, 0.01);
    history.push_stroke(mesh.end_stroke(false).unwrap());
    let refined = kept(&mesh);
    mesh.begin_stroke();
    push(&mut mesh, centre, -0.005);
    let last = mesh.end_stroke(false).unwrap();
    assert!(
        matches!(&last, StrokeDelta::Positions { topology: None, verts, .. } if verts.iter().any(|&v| v as usize >= original.positions.len()))
    );
    history.push_stroke(last);

    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == refined);
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == plain);
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == original);
    assert_sound(&mesh);
    for _ in 0..3 {
        assert_eq!(history.redo(&mut mesh), Some(false));
    }
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
}

#[test]
fn strokes_and_rays_still_work_after_undo_and_redo() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    let mut history = History::default();
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.1);
    let path = across(-0.2, 0.2, 0.1, 30);
    history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &path).unwrap());
    let grown = mesh.vertex_count();
    history.undo(&mut mesh).unwrap();
    assert!(mesh.vertex_count() < grown);

    // A new stroke on the mesh undo left, which drops the redo.
    let other = across(-0.2, 0.2, -0.1, 30);
    history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &other).unwrap());
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    for p in path.iter().chain(&other) {
        let (origin, dir) = view.ray(view.project(*p).unwrap());
        let (hit, sure) = (mesh.raycast(origin, dir).unwrap(), brute_force_hit(&mesh, origin, dir).unwrap());
        assert!(hit.pos.distance(sure) < 1e-4);
    }
    history.undo(&mut mesh).unwrap();
    history.redo(&mut mesh).unwrap();
    history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &path).unwrap());
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
}

// ---------------------------------------------------------------- open edges

/// The Unity app zipped the UV seam of its own primitives shut before
/// splitting along it. Meshes here are welded when they are made or brought
/// in, so there is no seam to zip; what is left of that case is a real open
/// edge, which is split on the boundary and stays one closed loop.
#[test]
fn an_open_edge_is_split_on_the_boundary_and_the_hole_stays_one_loop() {
    // The front half of a ball: a cap with a rim.
    let ball = primitives::build(Primitive::Sphere, 3_000, 0.5);
    let front: Vec<u32> = ball
        .indices
        .chunks_exact(3)
        .filter(|t| t.iter().all(|&v| ball.positions[v as usize].z >= 0.0))
        .flatten()
        .copied()
        .collect();
    let mut mesh = Mesh::new(ball.positions.clone(), front, None);
    let rim_before = count_edges(&mesh, 1);
    assert!(rim_before > 20, "the cap has no rim");

    let on_rim = Vec3::new(0.5, 0.0, 0.0);
    let before = mesh.vertex_count();
    let stats = mesh.refine(&[on_rim], 0.15, 0.012, 1_000_000, 10_000_000);
    assert!(stats.splits > 50);
    assert_sound(&mesh);
    assert_eq!(count_edges(&mesh, 3), 0, "an edge got a third triangle");
    assert!(count_edges(&mesh, 1) > rim_before, "the rim was not refined");

    // Every new rim vertex is exactly on the rim's plane, on a straight edge,
    // and the rim is still one loop: each of its vertices has two rim edges.
    let mut rim: HashMap<u32, u32> = HashMap::new();
    let mut edges: HashMap<(u32, u32), u32> = HashMap::new();
    for c in mesh.indices.chunks_exact(3) {
        for k in 0..3 {
            let (a, b) = (c[k], c[(k + 1) % 3]);
            *edges.entry((a.min(b), a.max(b))).or_default() += 1;
        }
    }
    for (&(a, b), _) in edges.iter().filter(|(_, n)| **n == 1) {
        *rim.entry(a).or_default() += 1;
        *rim.entry(b).or_default() += 1;
    }
    assert!(rim.values().all(|&n| n == 2), "the rim forked or broke");
    let new_on_rim: Vec<u32> = rim.keys().copied().filter(|&v| v as usize >= before).collect();
    assert!(new_on_rim.len() > 5);
    for v in new_on_rim {
        assert_eq!(mesh.positions[v as usize].z, 0.0, "rim vertex {v} left the rim's plane");
    }
}

// ---------------------------------------------------------------- limits

#[test]
fn the_split_budget_and_the_triangle_limit_are_kept() {
    let mut mesh = fixture();
    let centre = region_centre();
    let mut whole = fixture();
    let all = refine_at(&mut whole, centre).splits;

    // A budget stops the refine, says so, and the next calls finish the job.
    let first = mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 50, 10_000_000);
    assert_eq!(first.splits, 50);
    assert!(first.hit_budget && !first.hit_limit);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    let mut calls = 1;
    loop {
        let more = mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 50, 10_000_000);
        calls += 1;
        assert!(more.splits <= 50);
        if !more.hit_budget {
            break;
        }
        assert!(calls < 1000, "the refine never finishes");
    }
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    let done = refine_at(&mut mesh, centre);
    assert_eq!(done.splits, 0, "converged, as the unbudgeted refine does");
    // Much the same amount of detail either way. (Not the same: every call
    // starts by flipping what the last one left, so the steps find better
    // triangles to bisect and need somewhat fewer splits.)
    let (budgeted, at_once) = (mesh.vertex_count() as f32, whole.vertex_count() as f32);
    println!("{calls} calls of 50 splits made {budgeted} vertices; one call of {all} splits made {at_once}");
    assert!(calls > all / 100, "{calls} calls for {all} splits");
    assert!((budgeted / at_once - 1.0).abs() < 0.25, "{budgeted} vertices in steps, {at_once} at once");

    // The triangle limit is never passed, and is reported.
    let mut mesh = fixture();
    let limit = mesh.triangle_count() + 100;
    let stats = mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 1_000_000, limit);
    assert!(stats.hit_limit && !stats.hit_budget && stats.splits > 0);
    assert!(mesh.triangle_count() <= limit && mesh.triangle_count() > limit - 4);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    let full = mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 1_000_000, mesh.triangle_count());
    assert!(full.hit_limit && full.splits == 0);
    // The lowest of each: no triangles allowed at all, and no splits.
    let none = mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 1_000_000, 0);
    assert!(none.hit_limit && none.splits == 0);
    let mut mesh = fixture();
    let unspent = mesh.refine(&[centre], REGION_RADIUS, MAX_EDGE, 0, 10_000_000);
    assert!(unspent.hit_budget && unspent.splits == 0 && unspent.vertices_added == 0);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    // Nothing to work with is not an error.
    for stats in [
        mesh.refine(&[], REGION_RADIUS, MAX_EDGE, 100, usize::MAX),
        mesh.refine(&[centre], 0.0, MAX_EDGE, 100, usize::MAX),
        mesh.refine(&[centre], REGION_RADIUS, f32::NAN, 100, usize::MAX),
        mesh.refine(&[Vec3::splat(9.0)], REGION_RADIUS, MAX_EDGE, 100, usize::MAX),
        Mesh::new(Vec::new(), Vec::new(), None).refine(&[centre], REGION_RADIUS, MAX_EDGE, 100, usize::MAX),
    ] {
        assert_eq!((stats.splits, stats.hit_limit, stats.hit_budget), (0, false, false));
    }
}

#[test]
fn detail_lands_at_the_size_asked_for() {
    for max_edge in [0.02f32, 0.012, 0.006] {
        let mut mesh = fixture();
        let (before, centre) = (mesh.vertex_count() as u32, region_centre());
        let stats = mesh.refine(&[centre], REGION_RADIUS, max_edge, 1_000_000, 10_000_000);
        assert!(stats.splits > 20);
        // Well inside the region every triangle is new, with no edge over
        // the size asked for and none needlessly small: bisection halves an
        // edge only while it is too long.
        let (mut count, mut longest_sum, mut shortest) = (0, 0.0, f32::MAX);
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = corners_of(&mesh, t);
            if ((a + b + c) / 3.0).distance(centre) > REGION_RADIUS * 0.8 {
                continue;
            }
            let sides = [a.distance(b), b.distance(c), c.distance(a)];
            let longest = sides.into_iter().fold(0.0, f32::max);
            assert!(longest <= max_edge * 1.0001);
            assert!(t.iter().any(|&v| v >= before), "an original triangle was left inside the region");
            shortest = sides.into_iter().fold(shortest, f32::min);
            longest_sum += longest;
            count += 1;
        }
        let mean = longest_sum / count as f32 / max_edge;
        println!(
            "max edge {max_edge}: {count} triangles, longest side {mean:.2} of it on average, shortest {:.2}",
            shortest / max_edge
        );
        assert!(count > 100);
        assert!(mean > 0.5 && mean <= 1.0, "longest sides average {mean} of the size asked for");
        assert!(shortest > max_edge * 0.2, "an edge of {shortest} for a detail size of {max_edge}");
    }
}

// ---------------------------------------------------------------- the index

#[test]
fn the_spatial_index_stays_sound_through_many_refines_large_and_small() {
    let mut mesh = fixture();
    let leaves_before = mesh.bvh.leaves.len();
    let mut deepest = 0;
    // A long walk over the surface in small steps, as a stroke goes: each
    // refine adds a sliver of new surface, a few triangles at a time.
    let mut sent = Sent::new(&mut mesh);
    for step in 0..400 {
        let a = step as f32 * 0.021;
        let centre = on_surface(Vec3::new(a.cos() * (1.0 + 0.3 * (a * 0.37).sin()), (a * 0.61).sin(), a.sin()));
        let radius = if step % 50 == 49 { 0.2 } else { 0.05 };
        mesh.refine(&[centre], radius, 0.01, 1_000_000, 10_000_000);
        deepest = deepest.max(mesh.bvh.depth());
        if step % 40 == 0 {
            assert_sound(&mesh);
            sent.follow(&mut mesh);
            sent.assert_matches(&mesh, &format!("step {step}"));
        }
    }
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    let (triangles, leaves) = (mesh.triangle_count(), mesh.bvh.leaves.len());
    assert!(triangles > 40_000, "only {triangles} triangles");
    // Small additions share leaves instead of each making its own.
    let added = (leaves - leaves_before) as f32;
    let per_leaf = (triangles - 5120) as f32 / added;
    println!("{triangles} triangles in {leaves} leaves ({per_leaf:.1} per new leaf), tree depth up to {deepest}");
    assert!(per_leaf > 16.0, "{per_leaf} triangles per new leaf");
    assert!(mesh.bvh.leaves.iter().all(|l| l.tri_count() <= LEAF_MAX_TRIS));

    // Laid out afresh, as at the end of a stroke, it is shallow again and
    // answers every ray as before.
    let rays: Vec<(Vec3, Vec3)> = (0..200)
        .map(|i| {
            let a = i as f32 * 0.7;
            (Vec3::new(a.cos() * 0.4, a.sin() * 0.4, 3.0), Vec3::NEG_Z)
        })
        .collect();
    let hits: Vec<Option<Vec3>> = rays.iter().map(|&(o, d)| mesh.raycast(o, d).map(|h| h.pos)).collect();
    mesh.bvh.rebuild_tree();
    assert_index_is_sound(&mesh);
    assert!(mesh.bvh.depth() <= 16, "depth {}", mesh.bvh.depth());
    for (&(o, d), hit) in rays.iter().zip(&hits) {
        assert_eq!(mesh.raycast(o, d).map(|h| h.pos), *hit);
        assert_eq!(hit.is_some(), brute_force_hit(&mesh, o, d).is_some());
    }
}

#[test]
fn a_deep_tree_is_laid_out_again_before_the_walks_run_out_of_stack() {
    let mut mesh = fixture();
    // Refining finer and finer at one spot hangs each refine's leaves under
    // the last one's.
    let centre = region_centre();
    let (mut deepest, mut last, mut laid_out_again) = (0, 0, 0);
    for step in 0..300 {
        let max_edge = 0.02 * 0.985f32.powi(step);
        mesh.refine(&[centre], 0.03, max_edge, 1_000_000, 10_000_000);
        let depth = mesh.bvh.depth();
        laid_out_again += usize::from(depth + 8 < last);
        (deepest, last) = (deepest.max(depth), depth);
    }
    println!(
        "{} triangles; deepest {deepest}, now {last}, laid out again {laid_out_again} times",
        mesh.triangle_count()
    );
    assert!(deepest > 30 && laid_out_again > 0, "the tree never got deep: this tested nothing");
    assert_sound(&mesh);
    let (origin, dir) = (centre + region_direction(), -region_direction());
    assert!(mesh.raycast(origin, dir).unwrap().pos.distance(brute_force_hit(&mesh, origin, dir).unwrap()) < 1e-4);
}

// ---------------------------------------------------------------- hidden faces

#[test]
fn hidden_faces_are_left_alone_and_stay_hidden() {
    let mut mesh = fixture();
    let centre = region_centre();
    // Everything left of the region's centre, which cuts the region in two.
    let flags: Vec<bool> = mesh.positions.iter().map(|p| p.x < centre.x).collect();
    let hidden = sculpt_cut::select::triangles_inside(&mesh.indices, &flags);
    assert!(mesh.hide(&hidden).is_some());
    let hidden_corners: Vec<[u32; 3]> = hidden.iter().map(|&t| mesh.corners(t)).collect();
    let (hidden_vertices, version) = (mesh.hidden_vertices().unwrap().to_vec(), mesh.visibility_version());
    let (before, positions) = (mesh.vertex_count(), mesh.positions.clone());
    let mut sent = Sent::new(&mut mesh);

    let stats = refine_at(&mut mesh, centre);
    assert!(stats.splits > 30, "the showing half was not refined");
    assert_sound(&mesh);
    assert_eq!(count_edges(&mesh, 3), 0);
    assert_eq!(count_edges(&mesh, 1), 0, "a crack along the cut");
    // The same faces are hidden, corner for corner, and nothing else is.
    assert_eq!(mesh.hidden_triangle_count(), hidden.len());
    for (&t, corners) in hidden.iter().zip(&hidden_corners) {
        assert!(mesh.is_triangle_hidden(t));
        assert_eq!(mesh.corners(t), *corners, "hidden triangle {t} was rewritten");
    }
    assert_eq!(&mesh.hidden_vertices().unwrap()[..before], &hidden_vertices[..]);
    assert!(mesh.hidden_vertices().unwrap()[before..].iter().all(|h| !h), "a new vertex is hidden");
    assert!(mesh.visibility_version() > version, "the renderer is not told the faces changed");
    assert_eq!(mesh.visible_indices().len(), (mesh.triangle_count() - hidden.len()) * 3);
    // No new vertex on the hidden side of the cut edge.
    for v in before..mesh.vertex_count() {
        assert!(mesh.positions[v].x >= centre.x - 0.04, "vertex {v} was made in the hidden part");
    }
    assert_eq!(&mesh.positions[..before], &positions[..]);
    sent.follow(&mut mesh);
    assert!(sent.positions == mesh.positions && sent.mask == mesh.mask);

    // A refining stroke across the cut moves and refines only what shows,
    // and undo and redo keep the hidden part hidden.
    let mut history = History::default();
    let view = view_of(&mesh);
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.08);
    // From the showing side up to the cut: the brush reaches across it.
    let path = across(centre.x + 0.25, centre.x + 0.01, centre.y, 30);
    let grown = mesh.vertex_count();
    history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &path).unwrap());
    assert!(mesh.vertex_count() > grown);
    assert_sound(&mesh);
    for v in 0..before {
        if hidden_vertices[v] {
            assert_eq!(mesh.positions[v], positions[v], "hidden vertex {v} moved");
        }
    }
    let after = mesh.hidden_triangles().unwrap().to_vec();
    assert_eq!(after.iter().filter(|h| **h).count(), hidden.len());
    history.undo(&mut mesh).unwrap();
    assert_eq!(mesh.hidden_triangle_count(), hidden.len());
    assert!(hidden.iter().all(|&t| mesh.is_triangle_hidden(t)));
    assert_sound(&mesh);
    history.redo(&mut mesh).unwrap();
    assert_eq!(mesh.hidden_triangles().unwrap(), &after[..]);
    assert_sound(&mesh);

    // Showing everything again shows a whole, closed surface.
    assert!(mesh.show_all().is_some());
    assert_closed_manifold(&mesh);
}

// ---------------------------------------------------------------- through the brushes

#[test]
fn a_clay_stroke_with_dynamic_topology_stays_manifold_and_undoes() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    let before = kept(&mesh);
    let mut sculptor = refining_sculptor(Brush::Clay, 0.1);
    // 0.015 at this radius, under the fixture's 0.033 edge.
    let delta = run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.2, 0.2, 0.1, 30)).unwrap();
    assert!(mesh.vertex_count() > before.positions.len() + 100);
    assert!(!sculptor.dyntopo.at_limit);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    assert!(mesh.positions.iter().all(|p| p.is_finite()));
    let moved = mesh.positions[..before.positions.len()].iter().zip(&before.positions).filter(|(a, b)| a != b).count();
    assert!(moved > 50, "the stroke only refined: {moved} vertices moved");

    let mut history = History::default();
    history.push_stroke(delta);
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == before);
    assert_sound(&mesh);
}

#[test]
fn dynamic_topology_off_leaves_topology_alone() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    let before = kept(&mesh);
    let mut sculptor = refining_sculptor(Brush::Clay, 0.1);
    sculptor.dyntopo.enabled = false;
    let delta = run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.2, 0.2, 0.1, 30)).unwrap();
    assert!(matches!(delta, StrokeDelta::Positions { topology: None, .. }));
    assert_eq!(mesh.vertex_count(), before.positions.len());
    assert!(mesh.indices == before.indices);
    assert_eq!(mesh.topology_version(), 0);
    assert!(mesh.positions != before.positions, "the stroke itself still sculpts");
}

#[test]
fn the_brushes_that_deposit_or_carve_refine_and_the_others_do_not() {
    for brush in Brush::ALL {
        let mut mesh = fixture();
        let view = view_of(&mesh);
        let (vertices, triangles) = (mesh.vertex_count(), mesh.indices.clone());
        let mut sculptor = refining_sculptor(brush, 0.1);
        run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.15, 0.15, 0.1, 24));
        let refined = mesh.vertex_count() > vertices;
        assert_eq!(refined, dyntopo::refines(brush), "{}", brush.name());
        if refined {
            assert!(mesh.vertex_count() > vertices + 100, "{}", brush.name());
            assert_closed_manifold(&mesh);
            assert_sound(&mesh);
        } else {
            assert!(mesh.indices == triangles);
        }
    }
    // Move and Pose hold vertex numbers from press to release: the mesh
    // must be the same mesh all through their drags.
    assert!(!dyntopo::refines(Brush::Move) && !dyntopo::refines(Brush::Pose));
    assert!(
        Brush::ALL
            .into_iter()
            .filter(|&b| dyntopo::refines(b))
            .map(Brush::name)
            .eq(["Clay", "Crease", "Inflate", "Flatten", "Standard", "Layer"])
    );

    // Mask painting and Shift's Smooth do not, whichever brush is in hand.
    for (mask, smooth) in [(true, false), (false, true)] {
        let mut mesh = fixture();
        let view = view_of(&mesh);
        let vertices = mesh.vertex_count();
        let mut sculptor = refining_sculptor(Brush::Clay, 0.1);
        sculptor.settings.mask_mode = mask;
        let at = |x: f32| PointerSample {
            pos: view.project(on_surface(Vec3::new(x, 0.1, 0.45))).unwrap(),
            pressure: None,
            time: 0.0,
        };
        assert!(sculptor.begin(&mut mesh, &view, at(-0.1), false, smooth));
        for i in 0..20 {
            sculptor.sample(&mut mesh, &view, at(-0.1 + i as f32 * 0.01));
        }
        sculptor.end(&mut mesh, &view);
        assert_eq!(mesh.vertex_count(), vertices);
    }
}

#[test]
fn a_stroke_refines_to_the_brush_detail_and_a_smaller_brush_goes_finer() {
    let mut sizes = Vec::new();
    for (radius, detail) in [(0.1f32, 0.15f32), (0.1, 0.3), (0.05, 0.15)] {
        let mut mesh = fixture();
        let view = view_of(&mesh);
        let before = mesh.vertex_count() as u32;
        let mut sculptor = refining_sculptor(Brush::Inflate, radius);
        sculptor.dyntopo.detail = detail;
        sculptor.settings.per_brush[Brush::Inflate.index()].strength = 0.03;
        let path = across(-0.2, 0.2, 0.1, 40);
        run_stroke(&mut mesh, &mut sculptor, &view, &path);
        assert!(mesh.vertex_count() as u32 > before + 50);
        // The triangles the stroke's centre line ran over.
        let (mut count, mut over, mut longest_sum) = (0, 0, 0.0);
        for t in mesh.indices.chunks_exact(3) {
            let [a, b, c] = corners_of(&mesh, t);
            let centroid = (a + b + c) / 3.0;
            if centroid.z < 0.0 || !path.iter().any(|p| p.distance(centroid) < radius * 0.5) {
                continue;
            }
            let longest = a.distance(b).max(b.distance(c)).max(c.distance(a));
            count += 1;
            // The dabs that follow a refine move what it made, a little.
            over += usize::from(longest > radius * detail * 1.25);
            longest_sum += longest;
        }
        let mean = longest_sum / count as f32;
        println!(
            "radius {radius}, detail {detail}: {count} triangles, longest side {mean:.4} on average ({:.2} of the detail), {over} over",
            mean / (radius * detail)
        );
        assert!(count > 50);
        assert!(over * 100 <= count, "{over} of {count} triangles are coarser than the detail asked for");
        assert!(mean > radius * detail * 0.55);
        sizes.push(mean);
    }
    // Coarser detail leaves bigger triangles; half the brush, half the size.
    assert!(sizes[1] > sizes[0] * 1.4 && sizes[2] < sizes[0] * 0.65, "{sizes:?}");
}

#[test]
fn a_vertex_born_mid_stroke_starts_where_the_surface_started() {
    let mut mesh = fixture();
    let centre = region_centre();
    let before = mesh.vertex_count();
    let rest = mesh.positions.clone();
    let (origin, dir) = (centre + region_direction(), -region_direction());
    let found = mesh.raycast(origin, dir).unwrap().pos;
    mesh.begin_stroke();
    // Raised first, refined after: the new vertices are made on a surface
    // the stroke has already moved.
    push(&mut mesh, centre, 0.05);
    assert!(mesh.refine(&[centre], 0.08, MAX_EDGE, 1_000_000, 10_000_000).splits > 50);
    let mut born_moved = 0;
    for v in before..mesh.vertex_count() {
        let [a, b] = mesh.split_parents(v as u32).unwrap();
        let (start, live) = (mesh.start_pos(v as u32), mesh.positions[v]);
        if mesh.start_pos(a) == mesh.positions[a as usize] && mesh.start_pos(b) == mesh.positions[b as usize] {
            assert_eq!(start, live, "vertex {v} was born on untouched surface");
        } else {
            // Halfway between where its parents started: on the surface the
            // stroke found, not on the one it has raised.
            assert_eq!(start, (mesh.start_pos(a) + mesh.start_pos(b)) * 0.5);
            born_moved += usize::from(start.distance(live) > 0.01);
        }
    }
    assert!(born_moved > 20, "{born_moved}");
    // A ray at the stroke-start surface still finds it where it was, to
    // within the bow of a split made on surface the stroke had not reached.
    let then = mesh.raycast_start(origin, dir).unwrap().pos;
    let now = mesh.raycast(origin, dir).unwrap().pos;
    assert!(then.distance(found) < 0.001, "{}", then.distance(found));
    assert!(now.distance(found) > 0.03);
    assert_index_is_sound(&mesh);
    let delta = mesh.end_stroke(false).unwrap();
    // Undone, the surface is where it was, vertex for vertex.
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.positions, rest);
}

#[test]
fn the_mask_survives_a_refining_stroke_and_still_protects() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    // Fully masked above the stroke's centre line, a soft edge below it,
    // free below that.
    for v in 0..mesh.vertex_count() {
        mesh.mask[v] = ((mesh.positions[v].y - 0.05) / 0.06).clamp(0.0, 1.0);
    }
    let (before, mask, rest) = (mesh.vertex_count(), mesh.mask.clone(), mesh.positions.clone());
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.1);
    let delta = run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.2, 0.2, 0.1, 30)).unwrap();
    assert!(mesh.vertex_count() > before + 100);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    // The mask as painted is untouched, and what was fully masked has not
    // moved, though the brush went straight over it.
    assert_eq!(&mesh.mask[..before], &mask[..]);
    let mut held = 0;
    for v in 0..before {
        if mask[v] >= 1.0 {
            assert_eq!(mesh.positions[v], rest[v], "masked vertex {v} moved");
            held += usize::from(rest[v].distance(on_surface(Vec3::new(rest[v].x, 0.1, 0.45))) < 0.08);
        }
    }
    assert!(held > 20, "the stroke never reached the masked part");
    // Every new vertex is masked as the surface it was made on: between its
    // two parents' values, and so fully masked only between two that are.
    let (mut soft, mut solid) = (0, 0);
    for v in before..mesh.vertex_count() {
        let [a, b] = mesh.split_parents(v as u32).unwrap();
        let (low, high) =
            (mesh.mask[a as usize].min(mesh.mask[b as usize]), mesh.mask[a as usize].max(mesh.mask[b as usize]));
        assert!(mesh.mask[v] >= low && mesh.mask[v] <= high);
        soft += usize::from(mesh.mask[v] > 0.0 && mesh.mask[v] < 1.0);
        solid += usize::from(mesh.mask[v] >= 1.0);
    }
    assert!(soft > 20, "{soft} new vertices on the mask's edge");
    // Nothing was added where both ends of an edge were frozen.
    assert_eq!(solid, 0, "{solid} vertices were added inside the fully masked part");

    // Undo gives the mask back with the mesh.
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.mask, mask);
    assert_eq!(mesh.positions, rest);
}

#[test]
fn a_fast_stroke_is_refined_from_end_to_end() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    let before = mesh.vertex_count() as u32;
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.05);
    // Barely raising the surface, so it can be looked for where it was.
    sculptor.settings.per_brush[Brush::Inflate.index()].strength = 0.02;
    // Three samples, a frame apart, each eight brush radii from the last:
    // the refining has to cover the path between them, not just where they
    // landed.
    let path = across(-0.4, 0.4, 0.1, 2);
    let sample =
        |i: usize| PointerSample { pos: view.project(path[i]).unwrap(), pressure: None, time: i as f64 / 60.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    for i in 1..path.len() {
        sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        sculptor.sample(&mut mesh, &view, sample(i));
    }
    sculptor.end(&mut mesh, &view);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    let refined_near = |p: Vec3| {
        mesh.indices.chunks_exact(3).any(|t| {
            let [a, b, c] = corners_of(&mesh, t);
            t.iter().any(|&v| v >= before) && sculpt_geom::tri::closest_point(p, a, b, c).distance(p) < 0.02
        })
    };
    for p in across(-0.4, 0.4, 0.1, 40) {
        assert!(refined_near(p), "the stroke left a coarse gap at {p}");
    }
    // And only along the path.
    assert!(!refined_near(on_surface(Vec3::new(0.0, -0.2, 0.45))));
}

#[test]
fn a_refine_that_runs_out_of_budget_carries_on_while_the_pointer_rests() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    // A big brush asking for fine detail: more splits than one frame allows.
    let radius = 0.25;
    let mut sculptor = refining_sculptor(Brush::Inflate, radius);
    sculptor.dyntopo.detail = 0.03;
    let centre = on_surface(Vec3::new(0.0, 0.0, 1.0));
    let press = PointerSample { pos: view.project(centre).unwrap(), pressure: None, time: 0.0 };
    let before = mesh.vertex_count();
    assert!(sculptor.begin(&mut mesh, &view, press, false, false));
    let first = mesh.vertex_count() - before;
    assert_eq!(first, 3000, "the press refines one frame's worth");
    // The pointer does not move again. The frame of the press has done its
    // share already; each frame after it does its own, until none is left.
    sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
    assert_eq!(mesh.vertex_count() - before, first);
    let mut frames = 0;
    loop {
        let had = mesh.vertex_count();
        sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
        let added = mesh.vertex_count() - had;
        assert!(added <= 3000);
        if added == 0 {
            break;
        }
        frames += 1;
        assert!(frames < 100, "the refine never finishes");
    }
    assert!(frames >= 1, "it was all done at the press: the budget was never the limit");
    let delta = sculptor.end(&mut mesh, &view).expect("the stroke changed the mesh");
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    // Finished: nothing under the brush is coarser than was asked for.
    let edge = radius * 0.03;
    for t in mesh.indices.chunks_exact(3) {
        let [a, b, c] = corners_of(&mesh, t);
        if ((a + b + c) / 3.0).distance(centre) < radius * 0.9 {
            assert!(a.distance(b).max(b.distance(c)).max(c.distance(a)) <= edge * 1.3);
        }
    }
    // And all of it is the one stroke's undo step.
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.vertex_count(), before);
}

#[test]
fn the_panel_is_told_when_the_triangle_limit_stops_a_stroke() {
    let mut mesh = fixture();
    let view = view_of(&mesh);
    let mut sculptor = refining_sculptor(Brush::Clay, 0.1);
    let limit = mesh.triangle_count() + 300;
    sculptor.dyntopo.limit_triangles(limit);
    let delta = run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.2, 0.2, 0.1, 30));
    assert!(sculptor.dyntopo.at_limit);
    assert!(mesh.triangle_count() <= limit && mesh.triangle_count() > limit - 4);
    assert!(delta.is_some(), "the stroke itself still sculpts");
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    // A stroke with it switched off clears the notice.
    sculptor.dyntopo.enabled = false;
    run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.2, 0.2, -0.1, 10));
    assert!(!sculptor.dyntopo.at_limit);
    assert!(mesh.triangle_count() <= limit);
}

// ---------------------------------------------------------------- symmetry

#[test]
fn mirrored_refine_adds_detail_on_both_sides() {
    let mut mesh = fixture();
    let before = mesh.vertex_count();
    let site = on_surface(Vec3::new(0.45, 0.1, 1.0));
    let mirrored = site * Vec3::new(-1.0, 1.0, 1.0);
    assert!(mesh.refine(&[site, mirrored], 0.115, 0.015, 1_000_000, 10_000_000).splits > 100);
    assert_closed_manifold(&mesh);
    let right = mesh.positions[before..].iter().filter(|p| p.x > 0.0).count();
    let left = mesh.positions.len() - before - right;
    assert!(right > 50);
    assert!((left as f32) > right as f32 * 0.8 && (left as f32) < right as f32 * 1.25, "right {right} left {left}");
}

/// How far the mesh is from being its own mirror image across X: for every
/// vertex, the distance from its reflection to the nearest point of the
/// surface. Returns the mean and the worst.
fn mirror_error(mesh: &Mesh) -> (f32, f32) {
    let (mut sum, mut worst) = (0.0f64, 0.0f32);
    for (v, &p) in mesh.positions.iter().enumerate() {
        let image = p * Vec3::new(-1.0, 1.0, 1.0);
        // The triangles near the image, through the leaves around it.
        let mut leaves = Vec::new();
        let mut reach = 0.02;
        while leaves.is_empty() {
            mesh.bvh.leaves_in_sphere(image, reach, &mut leaves);
            reach *= 2.0;
        }
        let nearest = leaves
            .iter()
            .flat_map(|&l| mesh.bvh.leaves[l as usize].tri_range())
            .map(|t| {
                let [a, b, c] = corners_of(mesh, &mesh.indices[t * 3..t * 3 + 3]);
                sculpt_geom::tri::closest_point(image, a, b, c).distance(image)
            })
            .fold(f32::MAX, f32::min);
        assert!(nearest < 0.02, "vertex {v} has nothing across the mirror plane");
        sum += nearest as f64;
        worst = worst.max(nearest);
    }
    ((sum / mesh.positions.len() as f64) as f32, worst)
}

#[test]
fn a_mirrored_stroke_leaves_a_mirrored_mesh() {
    for brush in [Brush::Clay, Brush::Inflate, Brush::Crease] {
        let mut mesh = fixture();
        let view = view_of(&mesh);
        let before = mesh.vertex_count();
        assert!(mirror_error(&mesh).1 < 1e-6, "the fixture is its own mirror image");
        let mut sculptor = refining_sculptor(brush, 0.08);
        mirror_x(&mut sculptor);
        // Starts to one side, crosses the mirror plane and runs along it:
        // the hard case, with both copies of the brush refining the same
        // triangles.
        let mut path: Vec<Vec3> =
            (0..=30).map(|i| on_surface(Vec3::new(0.3 - 0.01 * i as f32, 0.15 - 0.004 * i as f32, 0.45))).collect();
        path.extend((1..=20).map(|i| on_surface(Vec3::new(0.01, 0.03 - 0.01 * i as f32, 0.45))));
        let delta = run_stroke(&mut mesh, &mut sculptor, &view, &path).unwrap();
        assert_closed_manifold(&mesh);
        assert_sound(&mesh);

        // As much detail on one side as on the other...
        let added = &mesh.positions[before..];
        let right = added.iter().filter(|p| p.x > 0.0).count() as f32;
        let left = added.iter().filter(|p| p.x < 0.0).count() as f32;
        assert!(right > 100.0, "{}: {right}", brush.name());
        assert!((left / right - 1.0).abs() < 0.1, "{}: right {right} left {left}", brush.name());
        // ...and the same shape: every vertex's reflection lies on the
        // surface, to a small part of what the stroke moved it by. The
        // triangles themselves need not match across the plane (which of two
        // equal edges splits first is decided by their numbers), so it is the
        // surface that is compared.
        let StrokeDelta::Positions { verts, before: from, after: to, .. } = &delta else { panic!("a mask stroke?") };
        assert!(verts.len() > 30);
        let moved = from.iter().zip(to).map(|(a, b)| a.distance(*b)).fold(0.0, f32::max);
        let (mean, worst) = mirror_error(&mesh);
        println!("{}: moved up to {moved:.4}, mirror error mean {mean:.6} worst {worst:.6}", brush.name());
        assert!(moved > 0.003);
        assert!(mean < moved * 0.01, "{}: mean mirror error {mean} for a stroke of {moved}", brush.name());
        assert!(worst < moved * 0.1, "{}: worst mirror error {worst} for a stroke of {moved}", brush.name());
    }
}

// ---------------------------------------------------------------- instances, quads, reset

#[test]
fn an_instance_sharing_the_mesh_shows_the_refined_mesh() {
    let mut mesh = fixture();
    let mut scene = Scene::new("Ball");
    let original = scene.active_id();
    let twin = scene.instance_active();
    scene.get_mut(twin).unwrap().position = Vec3::new(2.0, 0.0, 0.0);
    scene.select(original, &mut mesh);
    let shared = scene.active_mesh();
    assert_eq!(scene.get(twin).unwrap().mesh, shared);
    let mut sent = Sent::new(&mut mesh);

    // A refining stroke on the original.
    let view = view_of(&mesh);
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.1);
    scene.keep_baseline(&mesh);
    let path = across(-0.2, 0.2, 0.1, 30);
    let sample =
        |i: usize| PointerSample { pos: view.project(path[i]).unwrap(), pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &view, sample(0), false, false));
    for i in 1..path.len() {
        sculptor.sample(&mut mesh, &view, sample(i));
        if i % 4 == 0 {
            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
            // One copy of the mesh is sent, whatever number of objects show
            // it, and it is complete after every frame of the stroke.
            sent.follow(&mut mesh);
            sent.assert_matches(&mesh, &format!("frame at sample {i}"));
        }
    }
    sculptor.end(&mut mesh, &view).unwrap();
    sent.follow(&mut mesh);
    sent.assert_matches(&mesh, "after the stroke");
    assert!(mesh.vertex_count() > 2562 + 100);

    // Both objects still show that one mesh, and a ray at the instance meets
    // the refined surface through the instance's own placement.
    assert_eq!((scene.active_mesh(), scene.get(twin).unwrap().mesh), (shared, shared));
    assert_eq!(scene.users(shared), 2);
    let on_original = scene.pick(Vec3::new(0.0, 0.1, 3.0), Vec3::NEG_Z, &mesh).unwrap();
    let on_twin = scene.pick(Vec3::new(2.0, 0.1, 3.0), Vec3::NEG_Z, &mesh).unwrap();
    assert_eq!((on_original.id, on_twin.id), (original, twin));
    assert!((on_twin.pos - Vec3::new(2.0, 0.0, 0.0)).distance(on_original.pos) < 1e-5);
    let unsculpted = fixture().raycast(Vec3::new(0.0, 0.1, 3.0), Vec3::NEG_Z).unwrap().pos;
    assert!(on_original.pos.z > unsculpted.z + 0.004, "the stroke raised the surface");

    // Sculpting through the instance refines the same mesh.
    scene.select(twin, &mut mesh);
    let local = view.local_to(scene.active().matrix());
    let grown = mesh.vertex_count();
    let lower = across(-0.2, 0.2, -0.15, 30);
    let world =
        |i: usize| PointerSample { pos: local.project(lower[i]).unwrap(), pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &local, world(0), false, false));
    for i in 1..lower.len() {
        sculptor.sample(&mut mesh, &local, world(i));
    }
    sculptor.end(&mut mesh, &local).unwrap();
    assert!(mesh.vertex_count() > grown + 100);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    sent.follow(&mut mesh);
    sent.assert_matches(&mesh, "after the stroke through the instance");
}

/// A live mirror copy is the object's own mesh drawn again through a
/// reflected matrix, and a press on the copy sculpts through that matrix. A
/// refining stroke begun there has to add its detail under the pointer, on
/// the part of the mesh the copy shows there, and both sides have to show it.
#[test]
fn a_refining_stroke_begun_on_a_mirror_copy_lands_under_the_pointer() {
    let mut mesh = fixture();
    let mut scene = Scene::new("Ball");
    let id = scene.active_id();
    scene.active_mut().position = Vec3::new(1.0, 0.0, 0.0);
    scene.active_mut().rotation = glam::Quat::from_rotation_y(0.4);
    assert!(scene.set_mirror(id, 1, Vec3::ZERO));
    let copy = scene.active().placed_matrix(1);
    assert!(copy.determinant() < 0.0, "the copy is a reflection");
    let mut camera = Camera::default();
    camera.frame(scene.bounds(&mesh), SIZE.x / SIZE.y, false);
    let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
    let mut sent = Sent::new(&mut mesh);

    // A path over the part of the copy that faces the camera, as places on
    // the mesh and as the pixels they are drawn at through the copy.
    let on_mesh: Vec<Vec3> = (0..=30)
        .map(|i| {
            let world = Vec3::new(-1.0 - 0.2 + 0.4 * i as f32 / 30.0, 0.1, 3.0);
            let hit = scene.pick(world, Vec3::NEG_Z, &mesh).expect("the ray meets the copy");
            assert_eq!((hit.id, hit.copy), (id, 1), "the copy is what is under the pointer");
            copy.inverse().transform_point3(hit.pos)
        })
        .collect();
    let pixel = |i: usize| view.project(copy.transform_point3(on_mesh[i])).unwrap();

    // What a press on the copy does in the app: aim, then the copy's own view.
    scene.aim_brush(id, 1);
    assert_eq!(scene.brush_matrix(), copy);
    let local = view.local_to(scene.brush_matrix());
    let before = mesh.vertex_count();
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.08);
    // Raising the surface only a little, so what is added stays by the path
    // (as little as 10% raised it before the Strength slider was mapped).
    sculptor.settings.per_brush[Brush::Inflate.index()].strength = crate::sculpt::strength::from_old(Brush::Inflate, 0.1);
    let sample = |i: usize| PointerSample { pos: pixel(i), pressure: None, time: i as f64 / 240.0 };
    assert!(sculptor.begin(&mut mesh, &local, sample(0), false, false), "the press missed the copy");
    for i in 1..on_mesh.len() {
        sculptor.sample(&mut mesh, &local, sample(i));
        if i % 4 == 0 {
            sculptor.frame(&mut mesh, &local, 1.0 / 60.0);
            sent.follow(&mut mesh);
            sent.assert_matches(&mesh, "mid-stroke on the copy");
        }
    }
    let delta = sculptor.end(&mut mesh, &local).expect("the stroke changed the mesh");
    scene.release_brush();
    sent.follow(&mut mesh);
    sent.assert_matches(&mesh, "after the stroke on the copy");
    assert!(mesh.vertex_count() > before + 100);
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);

    // Every vertex the stroke added is by the path, where the pointer went.
    for v in before..mesh.vertex_count() {
        let from_path = on_mesh.iter().map(|p| p.distance(mesh.positions[v])).fold(f32::MAX, f32::min);
        assert!(from_path < 0.08 * 1.15 + 0.1, "vertex {v} was added {from_path} from the stroke");
    }
    // A ray down the middle of the stroke meets raised, refined surface on
    // the copy, and the same on the object itself at the reflected place.
    let middle = copy.transform_point3(on_mesh[15]);
    let on_copy = scene.pick(Vec3::new(middle.x, middle.y, 3.0), Vec3::NEG_Z, &mesh).unwrap();
    assert_eq!(on_copy.copy, 1);
    assert!(on_copy.pos.z > middle.z + 0.002, "the stroke raised the surface under the pointer");
    let own = scene.active().matrix();
    let reflected = own.transform_point3(on_mesh[15]);
    assert!((reflected.x + middle.x).abs() < 1e-5 && (reflected.y - middle.y).abs() < 1e-5);
    let toward = own.transform_vector3(on_mesh[15]).normalize();
    let on_object = scene.pick(reflected + toward, -toward, &mesh).unwrap();
    assert_eq!((on_object.id, on_object.copy), (id, 0));
    let hit = mesh.raycast(on_mesh[15] * 3.0, -on_mesh[15].normalize()).unwrap();
    assert!(mesh.corners(hit.tri).iter().any(|&v| v as usize >= before), "the surface there was not refined");

    // And it is one undo step, whichever side it was drawn on.
    mesh.apply_delta(&delta, true);
    assert_eq!(mesh.vertex_count(), before);
    assert_sound(&mesh);
}

#[test]
fn quads_are_retired_by_the_first_refine_and_undo_brings_them_back() {
    // A sheet of quads facing the camera, each cut in two for the engine, as
    // a quad remesh leaves them.
    let n = 24usize;
    let at = |i: usize, j: usize| (j * (n + 1) + i) as u32;
    let positions: Vec<Vec3> = (0..=n)
        .flat_map(|j| (0..=n).map(move |i| Vec3::new(i as f32 / n as f32 - 0.5, j as f32 / n as f32 - 0.5, 0.0)))
        .collect();
    let (mut triangles, mut corners) = (Vec::new(), Vec::new());
    for j in 0..n {
        for i in 0..n {
            let q = [at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)];
            triangles.extend_from_slice(&[q[0], q[1], q[2], q[0], q[2], q[3]]);
            corners.extend_from_slice(&q);
        }
    }
    let (mut mesh, went) = Mesh::new_mapped(positions, triangles, None);
    let quads: Vec<u32> = corners.iter().map(|&v| went[v as usize]).collect();
    let faces = quads.len() / 4;
    mesh.polys = Some(Polys { start: (0..=faces as u32).map(|f| f * 4).collect(), indices: quads.clone() });
    let before = kept(&mesh);
    let view = view_of(&mesh);
    let mut history = History::default();

    // An ordinary stroke leaves the quads alone.
    let mut sculptor = refining_sculptor(Brush::Inflate, 0.1);
    sculptor.dyntopo.enabled = false;
    let front: Vec<Vec3> = (0..=20).map(|i| Vec3::new(-0.15 + 0.015 * i as f32, 0.1, 0.0)).collect();
    history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &front).unwrap());
    assert!(mesh.polys.as_ref().is_some_and(|p| p.indices == quads));

    // A refining one retires them: they describe triangles that are gone.
    sculptor.dyntopo.enabled = true;
    history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &front).unwrap());
    assert!(mesh.vertex_count() > before.positions.len());
    assert!(mesh.polys.is_none());
    assert_sound(&mesh);
    assert_eq!(count_edges(&mesh, 3), 0);

    // Undo gives the quads back with the triangles they were made of; redo
    // retires them again.
    history.undo(&mut mesh).unwrap();
    let back = mesh.polys.as_ref().expect("the quads are back");
    assert_eq!((back.indices.len(), back.start.len()), (quads.len(), faces + 1));
    assert!(back.indices == quads);
    assert!(mesh.indices == before.indices);
    history.redo(&mut mesh).unwrap();
    assert!(mesh.polys.is_none());
    history.undo(&mut mesh).unwrap();
    history.undo(&mut mesh).unwrap();
    assert!(kept(&mesh) == before && mesh.polys.is_some());
}

#[test]
fn reset_shape_after_refining_goes_back_to_the_surface_it_was_refined_from() {
    let mut mesh = fixture();
    let (made, triangles) = (mesh.positions.clone(), mesh.indices.clone());
    let made_mesh = Mesh::new(made.clone(), triangles, None);
    let mut scene = Scene::new("Ball");
    let mut history = History::default();
    let view = view_of(&mesh);
    let mut sculptor = refining_sculptor(Brush::Clay, 0.1);
    for y in [0.1, -0.05] {
        scene.keep_baseline(&mesh);
        history.push_stroke(run_stroke(&mut mesh, &mut sculptor, &view, &across(-0.2, 0.2, y, 30)).unwrap());
    }
    let sculpted = kept(&mesh);
    assert!(sculpted.positions.len() > made.len() + 200);
    assert!(scene.can_reset(&mesh), "Reset Shape is lost once the mesh has grown");

    let delta = scene.reset_shape(&mut mesh).expect("there is a sculpt to put back");
    history.push_stroke(delta);
    // The vertices the mesh was made with are back where they were made...
    assert_eq!(&mesh.positions[..made.len()], &made[..]);
    // ...and every vertex added since lies on the surface they spanned,
    // halfway between the two it was made from. (Not exactly on it where an
    // edge was flipped across two of the original faces first.)
    let off = |p: Vec3| {
        made_mesh.raycast(p * 3.0, -p.normalize()).expect("a ray at the centre meets the ball").pos.distance(p)
    };
    let (mut worst, mut sculpted_off) = (0.0f32, 0.0f32);
    for (v, &p) in mesh.positions.iter().enumerate().skip(made.len()) {
        let [a, b] = mesh.split_parents(v as u32).unwrap();
        assert_eq!(p, (mesh.positions[a as usize] + mesh.positions[b as usize]) * 0.5);
        worst = worst.max(off(p));
        sculpted_off = sculpted_off.max(off(sculpted.positions[v]));
    }
    println!(
        "at rest the new vertices are within {worst:.5} of the made surface; sculpted they were up to {sculpted_off:.4} off"
    );
    assert!(sculpted_off > 0.05);
    // The fixture's own facets are jittered by 0.0066 either way, and a flip
    // across two of them cuts the corner between them by about that much.
    assert!(worst < 0.008, "a vertex rests {worst} off the surface the mesh was made with");
    assert!(scene.reset_shape(&mut mesh).is_none(), "already at rest");
    assert_closed_manifold(&mesh);

    // Reset is an ordinary undo step on the refined mesh.
    assert_eq!(history.undo(&mut mesh), Some(false));
    assert!(kept(&mesh) == sculpted);
    // With the refining strokes undone as well, it is the mesh as made, at rest.
    history.undo(&mut mesh).unwrap();
    assert!(scene.can_reset(&mesh));
    history.undo(&mut mesh).unwrap();
    assert_eq!(mesh.positions, made);
    assert!(scene.reset_shape(&mut mesh).is_none());
}

// ---------------------------------------------------------------- a long session

/// A small generator of repeatable "random" numbers in 0..1.
struct Dice(u64);

impl Dice {
    fn roll(&mut self) -> f32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 40) as f32) / (1u64 << 24) as f32
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        low + (high - low) * self.roll()
    }

    fn pick<T: Copy>(&mut self, from: &[T]) -> T {
        from[((self.roll() * from.len() as f32) as usize).min(from.len() - 1)]
    }
}

/// Everything at once, in an order no one planned: refining strokes of every
/// kind, mirrored and not, plain strokes, mask painting, faces hidden and
/// shown, and undo and redo through all of it. After every step the mesh has
/// to be whole, its index and adjacency sound, the renderer's copy current,
/// and every undo and redo has to land exactly on the state that step left.
#[test]
fn a_long_session_of_strokes_undo_and_redo_keeps_everything_consistent() {
    let (refined, hidden_steps, undone, redone) = session(7, 70);
    assert!(refined > 10 && undone > 5 && redone > 2 && hidden_steps > 2, "the session never did the things it tests");
}

/// The same from many other starting points, for when the refining or its
/// bookkeeping has been changed. `cargo test --release many_long -- --ignored`
#[test]
#[ignore]
fn many_long_sessions_keep_everything_consistent() {
    for seed in 100..140 {
        session(seed, 120);
    }
}

/// Plays `steps` steps of a session chosen by `seed`, checking everything
/// after each. Returns how many refining strokes, hides and shows, undos and
/// redos it came to.
fn session(seed: u64, steps: usize) -> (usize, usize, usize, usize) {
    #[derive(PartialEq)]
    struct State {
        mesh: Kept,
        hidden: Vec<bool>,
    }
    let state =
        |mesh: &Mesh| State { mesh: kept(mesh), hidden: mesh.hidden_triangles().map_or(Vec::new(), <[bool]>::to_vec) };

    let mut mesh = fixture();
    let view = view_of(&mesh);
    let mut history = History::default();
    let mut sent = Sent::new(&mut mesh);
    let mut dice = Dice(seed);
    // `states[n]` is the mesh after `n` steps; `done` of them are applied.
    let mut states = vec![state(&mesh)];
    let mut done = 0;
    let (mut refined, mut undone, mut redone, mut hidden_steps) = (0, 0, 0, 0);

    for step in 0..steps {
        let what = dice.roll();
        if what < 0.2 && done > 0 {
            assert_eq!(history.undo(&mut mesh), Some(false));
            done -= 1;
            undone += 1;
            assert!(state(&mesh) == states[done], "step {step}: undo did not land on the state before");
        } else if what < 0.3 && done + 1 < states.len() {
            assert_eq!(history.redo(&mut mesh), Some(false));
            done += 1;
            redone += 1;
            assert!(state(&mesh) == states[done], "step {step}: redo did not land on the state after");
        } else {
            let recorded = if what < 0.4 {
                // Hide a patch, or show everything again.
                let delta = if mesh.any_hidden() && dice.roll() < 0.5 {
                    mesh.show_all()
                } else {
                    let towards = Vec3::new(dice.between(-1.0, 1.0), dice.between(-1.0, 1.0), dice.between(-0.2, 1.0));
                    let flags: Vec<bool> = mesh.positions.iter().map(|p| p.dot(towards.normalize()) > 0.3).collect();
                    mesh.hide(&sculpt_cut::select::triangles_inside(&mesh.indices, &flags))
                };
                hidden_steps += usize::from(delta.is_some());
                delta.map(|delta| history.push_visibility(delta)).is_some()
            } else {
                // The twelve that sculpt a plain mesh (Erase Layer, the thirteenth, needs a sculpt layer).
                let brush = dice.pick(&Brush::ALL[..12]);
                let mut sculptor = refining_sculptor(brush, dice.between(0.04, 0.12));
                sculptor.dyntopo.detail = dice.between(0.1, 0.3);
                sculptor.dyntopo.enabled = dice.roll() < 0.8;
                sculptor.settings.mask_mode = dice.roll() < 0.1;
                if dice.roll() < 0.4 {
                    mirror_x(&mut sculptor);
                }
                let (from, to) = (dice.between(-0.3, 0.3), dice.between(-0.3, 0.3));
                let path = across(from, to, dice.between(-0.25, 0.25), 16);
                let sample = |i: usize| PointerSample {
                    pos: view.project(path[i]).unwrap(),
                    pressure: None,
                    time: i as f64 / 240.0,
                };
                let grown = mesh.vertex_count();
                // A press on a hidden part goes through to nothing: no stroke.
                let delta = if sculptor.begin(&mut mesh, &view, sample(0), dice.roll() < 0.2, false) {
                    for i in 1..path.len() {
                        sculptor.sample(&mut mesh, &view, sample(i));
                        if i % 3 == 0 {
                            sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
                            sent.follow(&mut mesh);
                            sent.assert_matches(&mesh, &format!("step {step}, mid-stroke"));
                        }
                    }
                    sculptor.end(&mut mesh, &view)
                } else {
                    None
                };
                refined += usize::from(mesh.vertex_count() > grown);
                delta.map(|delta| history.push_stroke(delta)).is_some()
            };
            if recorded {
                states.truncate(done + 1);
                states.push(state(&mesh));
                done += 1;
            } else {
                assert!(state(&mesh) == states[done], "step {step}: the mesh changed without an undo step");
            }
        }
        assert_closed_manifold(&mesh);
        assert_sound(&mesh);
        sent.follow(&mut mesh);
        sent.assert_matches(&mesh, &format!("step {step}"));
    }
    println!(
        "seed {seed}: {} vertices; {refined} refining strokes, {hidden_steps} hides and shows, {undone} undos, {redone} redos",
        mesh.vertex_count()
    );

    // All the way back, and all the way forward again.
    while done > 0 {
        history.undo(&mut mesh).unwrap();
        done -= 1;
        assert!(state(&mesh) == states[done]);
    }
    assert_sound(&mesh);
    while done + 1 < states.len() {
        history.redo(&mut mesh).unwrap();
        done += 1;
        assert!(state(&mesh) == states[done]);
    }
    assert_closed_manifold(&mesh);
    assert_sound(&mesh);
    (refined, hidden_steps, undone, redone)
}

// ---------------------------------------------------------------- measurements

/// How long a refining stroke takes on a large mesh, beside the same stroke
/// with dynamic topology off: the Unity app's benchmark. A measurement, not a
/// gate. `cargo test --release timing -- --ignored --nocapture`
#[test]
#[ignore]
fn timing_of_a_clay_stroke_on_a_large_mesh() {
    use std::time::Instant;
    println!("threads: {}", rayon::current_num_threads());
    // The fixture at two densities, and a plain ball near the triangle limit;
    // with each, the length of its edges.
    type Build = fn() -> Mesh;
    let meshes: [(Build, f32); 3] = [
        (|| icosphere(7), 1.0515 * 0.5 / 128.0),
        (|| icosphere(8), 1.0515 * 0.5 / 256.0),
        (|| primitives::build(Primitive::Sphere, 3_000_000, 0.5), 0.0019),
    ];
    for (build, edge) in meshes {
        for refine in [false, true] {
            let mut mesh = build();
            let view = view_of(&mesh);
            let (triangles, vertices) = (mesh.triangle_count(), mesh.vertex_count());
            // A brush small enough that the mesh is coarser than the detail
            // it asks for (six tenths of an edge): the case dynamic topology
            // exists for.
            let radius = 6.0 * edge;
            let mut sculptor = refining_sculptor(Brush::Clay, radius);
            sculptor.dyntopo.enabled = refine;
            sculptor.dyntopo.detail = 0.1;
            let frames = 40;
            let at = |f: usize, s: usize| {
                let t = (f * 4 + s) as f32 / (frames * 4) as f32;
                let p = on_surface(Vec3::new(-0.15 + 0.3 * t, 0.05 * (t * 12.0).sin(), 0.45));
                PointerSample { pos: view.project(p).unwrap(), pressure: None, time: (f * 4 + s) as f64 / 240.0 }
            };
            assert!(sculptor.begin(&mut mesh, &view, at(0, 0), false, false));
            let mut frame_ms = Vec::new();
            for f in 0..frames {
                let start = Instant::now();
                for s in 1..=4 {
                    sculptor.sample(&mut mesh, &view, at(f, s));
                }
                sculptor.frame(&mut mesh, &view, 1.0 / 60.0);
                mesh.flush_normals();
                let _ = mesh.take_dirty();
                // The first frame pays for one-off buffers.
                if f > 0 {
                    frame_ms.push(start.elapsed().as_secs_f64() * 1000.0);
                }
            }
            let start = Instant::now();
            let delta = sculptor.end(&mut mesh, &view).unwrap();
            let end_ms = start.elapsed().as_secs_f64() * 1000.0;
            let added = mesh.vertex_count() - vertices;
            let (leaves, depth) = (mesh.bvh.leaves.len(), mesh.bvh.depth());
            let start = Instant::now();
            mesh.apply_delta(&delta, true);
            let undo_ms = start.elapsed().as_secs_f64() * 1000.0;
            let start = Instant::now();
            mesh.apply_delta(&delta, false);
            let redo_ms = start.elapsed().as_secs_f64() * 1000.0;
            frame_ms.sort_by(f64::total_cmp);
            let mean = frame_ms.iter().sum::<f64>() / frame_ms.len() as f64;
            println!(
                "{triangles:>8} tris, dynamic topology {}: frame mean {mean:.2} p95 {:.2} max {:.2} ms; added {added} vertices; \
                 stroke end {end_ms:.1} ms, undo {undo_ms:.1} ms, redo {redo_ms:.1} ms; {leaves} leaves, depth {depth}",
                if refine { "on " } else { "off" },
                frame_ms[frame_ms.len() * 95 / 100],
                frame_ms[frame_ms.len() - 1],
            );
        }
    }
}

/// Triangle shape after dynamic topology: what the structural tests do not
/// look at. Prints the smallest-angle distribution of the triangles a
/// refining stroke made, on the fixture and on one stretched along X, with
/// and without the brush moving them. A measurement, not a gate.
#[test]
#[ignore]
fn report_of_triangle_shape_after_a_refining_stroke() {
    for stretch in [1.0f32, 3.0] {
        for with_brush in [false, true] {
            let (mut positions, indices) = icosphere_arrays(SUBDIVISIONS);
            positions.iter_mut().for_each(|p| p.x *= stretch);
            let mut mesh = Mesh::new(positions, indices, None);
            let view = view_of(&mesh);
            let before = mesh.vertex_count() as u32;
            let radius = 0.1;
            let path: Vec<Vec3> = (0..=40)
                .map(|f| {
                    mesh.raycast(Vec3::new((-0.3 + 0.6 * f as f32 / 40.0) * stretch, 0.1, 2.0), Vec3::NEG_Z)
                        .unwrap()
                        .pos
                })
                .collect();
            if with_brush {
                let mut sculptor = refining_sculptor(Brush::Clay, radius);
                run_stroke(&mut mesh, &mut sculptor, &view, &path);
            } else {
                for p in &path {
                    mesh.refine(&[*p], radius * 1.15, radius * 0.15, 3000, 4_000_000);
                }
            }
            let (mut count, mut under_10, mut under_20, mut sum) = (0, 0, 0, 0.0);
            for t in mesh.indices.chunks_exact(3).filter(|t| t.iter().any(|&v| v >= before)) {
                let [a, b, c] = corners_of(&mesh, t);
                let smallest = smallest_angle(a, b, c);
                count += 1;
                sum += smallest;
                under_10 += usize::from(smallest < 10.0);
                under_20 += usize::from(smallest < 20.0);
            }
            println!(
                "stretch {stretch} brush {with_brush}: {count} refined triangles, smallest angle {:.1} on average, {:.1}% under 10 degrees, {:.1}% under 20",
                sum / count as f32,
                100.0 * under_10 as f32 / count as f32,
                100.0 * under_20 as f32 / count as f32,
            );
        }
    }
}
