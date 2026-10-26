//! What every unwrap must satisfy, measured rather than assumed.
//!
//! * **In the square:** every UV in `[0, 1]`.
//! * **Nothing overlaps:** all UV triangles are drawn into a 1024 by 1024
//!   grid, a pixel belonging to a triangle when its centre is inside. Edges
//!   shared by two triangles go to exactly one of them (the usual top-left
//!   rule, worked out the same way from both sides), so a pixel drawn twice
//!   is a real overlap, inside a chart or between two.
//! * **Every chart one piece:** triangles are joined into islands where they
//!   share an edge with the same UVs at both ends. A chart in two pieces
//!   would make more islands than charts. Triangles with no UV area are in
//!   no island.
//! * **Nothing flipped:** no UV triangle of a face with real surface has zero
//!   or negative area. A face counts as having real surface unless it is
//!   thinner than a millionth of the model's size, the faces the unwrapper
//!   parks on a point because a UV in single precision could not hold them.
//! * **Even texels:** the spread of texel density between charts, as the
//!   ratio of the largest to the smallest island's UV area per surface area.

use glam::{DVec2, Vec2, Vec3};

use super::meshes::Model;
use crate::Unwrapped;

/// The grid overlaps are counted on.
pub const GRID: usize = 1024;

/// The measured quality of an unwrap.
#[derive(Debug)]
pub struct Quality {
    pub charts: usize,
    pub islands: usize,
    /// UVs outside `[0, 1]` (or not numbers).
    pub outside: usize,
    /// Pixels of the grid covered twice.
    pub overlaps: usize,
    /// Faces with real surface whose UV triangle has no area, or is wound
    /// backwards.
    pub flipped: usize,
    /// Faces too thin to have real surface, and so allowed no UV area.
    pub slivers: usize,
    pub stretch: f32,
    /// Share of the square covered by charts.
    pub coverage: f32,
    /// Largest over smallest island texel density (UV area per surface
    /// area), islands of at least a hundredth of the surface only (all of
    /// them if none is that big).
    pub density_spread: f32,
    /// The island of every triangle, [`NO_ISLAND`] for one with no UV area.
    pub island_of: Vec<u32>,
}

/// The island of a triangle with no UV area.
pub const NO_ISLAND: u32 = u32::MAX;

impl Quality {
    pub fn print(&self, name: &str) {
        println!(
            "{name}: {} charts ({} islands), stretch {:.4}, {} overlapping pixels, {} flipped, \
             {} slivers, {} outside, {:.1}% of the square used, density spread {:.3}",
            self.charts,
            self.islands,
            self.stretch,
            self.overlaps,
            self.flipped,
            self.slivers,
            self.outside,
            self.coverage * 100.0,
            self.density_spread
        );
    }

    /// Asserts everything every unwrap must satisfy.
    pub fn assert_valid(&self, name: &str) {
        assert_eq!(self.outside, 0, "{name}: UVs outside the square");
        assert_eq!(self.overlaps, 0, "{name}: overlapping UVs");
        assert_eq!(self.flipped, 0, "{name}: flipped UV triangles");
        assert_eq!(self.islands, self.charts, "{name}: a chart in more than one piece");
        assert!(self.stretch >= 1.0 - 1e-4, "{name}: stretch below 1 is impossible");
    }
}

/// Measures `out`, an unwrap of `model`.
pub fn measure(model: &Model, out: &Unwrapped) -> Quality {
    let (positions, triangles, uvs) = (&model.positions, &model.triangles, &out.uvs);
    assert_eq!(uvs.len(), triangles.len(), "one UV per corner");
    let outside = uvs.iter().filter(|p| !(p.x >= 0.0 && p.x <= 1.0 && p.y >= 0.0 && p.y <= 1.0)).count();
    let areas = surface_areas(positions, triangles);
    let real = real_surface(positions, triangles);
    let flipped = (0..areas.len()).filter(|&t| real[t] && !(uv_area(uvs, t) > 0.0)).count();
    let slivers = real.iter().filter(|&&r| !r).count();
    let island_of = islands(triangles, uvs);
    let islands = island_of.iter().copied().filter(|&i| i != NO_ISLAND).max().map_or(0, |m| m as usize + 1);
    let (overlaps, covered) = overlaps(uvs);

    // Texel density per island.
    let mut flat = vec![0.0f64; islands];
    let mut surface = vec![0.0f64; islands];
    for t in (0..areas.len()).filter(|&t| island_of[t] != NO_ISLAND) {
        flat[island_of[t] as usize] += uv_area(uvs, t);
        surface[island_of[t] as usize] += areas[t];
    }
    let total: f64 = surface.iter().sum();
    let big = (0..islands).filter(|&i| surface[i] > total * 0.01);
    let densities: Vec<f64> = if big.clone().count() > 0 {
        big.map(|i| flat[i] / surface[i]).collect()
    } else {
        // All small: take them all.
        (0..islands).map(|i| flat[i] / surface[i]).collect()
    };
    let spread =
        densities.iter().copied().fold(0.0, f64::max) / densities.iter().copied().fold(f64::INFINITY, f64::min);
    Quality {
        charts: out.charts,
        islands,
        outside,
        overlaps,
        flipped,
        slivers,
        stretch: out.stretch,
        coverage: covered as f32 / (GRID * GRID) as f32,
        density_spread: spread as f32,
        island_of,
    }
}

fn surface_areas(positions: &[Vec3], triangles: &[u32]) -> Vec<f64> {
    triangles
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|k| positions[t[k] as usize].as_dvec3());
            (b - a).cross(c - a).length() / 2.0
        })
        .collect()
}

/// Whether each face has real surface: it is no thinner (its height over its
/// longest side) than a millionth of the model's bounding diagonal.
fn real_surface(positions: &[Vec3], triangles: &[u32]) -> Vec<bool> {
    let lo = positions.iter().fold(Vec3::INFINITY, |m, &p| m.min(p));
    let hi = positions.iter().fold(Vec3::NEG_INFINITY, |m, &p| m.max(p));
    let least = (hi - lo).as_dvec3().length() * 1e-6;
    triangles
        .chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [0, 1, 2].map(|k| positions[t[k] as usize].as_dvec3());
            let longest = (b - a).length().max((c - b).length()).max((a - c).length());
            (b - a).cross(c - a).length() / longest > least
        })
        .collect()
}

fn uv_area(uvs: &[Vec2], t: usize) -> f64 {
    let [a, b, c] = [0, 1, 2].map(|k| uvs[t * 3 + k].as_dvec2());
    (b - a).perp_dot(c - a) / 2.0
}

/// The island of every triangle, numbered from 0 in order of first triangle;
/// [`NO_ISLAND`] for a triangle with no UV area.
pub fn islands(triangles: &[u32], uvs: &[Vec2]) -> Vec<u32> {
    let count = triangles.len() / 3;
    let has_area: Vec<bool> = (0..count).map(|t| uv_area(uvs, t) > 0.0).collect();
    let mut parent: Vec<u32> = (0..count as u32).collect();
    fn root(parent: &mut [u32], mut x: u32) -> u32 {
        while parent[x as usize] != x {
            parent[x as usize] = parent[parent[x as usize] as usize];
            x = parent[x as usize];
        }
        x
    }
    // An edge, keyed by its points and their UVs; two triangles on it join.
    let mut first = std::collections::HashMap::new();
    for t in (0..count).filter(|&t| has_area[t]) {
        for k in 0..3 {
            let (c0, c1) = (t * 3 + k, t * 3 + (k + 1) % 3);
            let (a, b) = ((triangles[c0], uvs[c0]), (triangles[c1], uvs[c1]));
            let (lo, hi) = if a.0 < b.0 { (a, b) } else { (b, a) };
            let key = (lo.0, hi.0, lo.1.to_array().map(f32::to_bits), hi.1.to_array().map(f32::to_bits));
            match first.entry(key) {
                std::collections::hash_map::Entry::Vacant(e) => {
                    e.insert(t as u32);
                }
                std::collections::hash_map::Entry::Occupied(e) => {
                    let (ra, rb) = (root(&mut parent, *e.get()), root(&mut parent, t as u32));
                    if ra != rb {
                        parent[ra.max(rb) as usize] = ra.min(rb);
                    }
                }
            }
        }
    }
    let mut number = vec![u32::MAX; count];
    let mut next = 0;
    (0..count as u32)
        .map(|t| {
            if !has_area[t as usize] {
                return NO_ISLAND;
            }
            let r = root(&mut parent, t) as usize;
            if number[r] == u32::MAX {
                number[r] = next;
                next += 1;
            }
            number[r]
        })
        .collect()
}

/// Pixels of the grid drawn more than once, and pixels drawn at all.
fn overlaps(uvs: &[Vec2]) -> (usize, usize) {
    let mut hits = vec![0u8; GRID * GRID];
    for tri in uvs.chunks_exact(3) {
        let p = [0, 1, 2].map(|k| tri[k].as_dvec2() * GRID as f64);
        if !((p[1] - p[0]).perp_dot(p[2] - p[0]) > 0.0) {
            continue; // flipped or flat: counted as flipped instead
        }
        let lo = p[0].min(p[1]).min(p[2]);
        let hi = p[0].max(p[1]).max(p[2]);
        let (x0, y0) = ((lo.x - 0.5).ceil().max(0.0) as usize, (lo.y - 0.5).ceil().max(0.0) as usize);
        let x1 = ((hi.x - 0.5).floor() as isize).min(GRID as isize - 1);
        let y1 = ((hi.y - 0.5).floor() as isize).min(GRID as isize - 1);
        for y in y0 as isize..=y1 {
            for x in x0 as isize..=x1 {
                let c = DVec2::new(x as f64 + 0.5, y as f64 + 0.5);
                if (0..3).all(|k| inside_edge(p[k], p[(k + 1) % 3], c)) {
                    let h = &mut hits[y as usize * GRID + x as usize];
                    *h = h.saturating_add(1);
                }
            }
        }
    }
    let twice = hits.iter().filter(|&&h| h > 1).count();
    let drawn = hits.iter().filter(|&&h| h > 0).count();
    (twice, drawn)
}

/// Whether `c` is on the inner side of the edge from `a` to `b` of a
/// counter-clockwise triangle, a point exactly on the edge counting for one
/// direction of it only. The edge is worked out from its lower end whichever
/// way round it is walked, so both triangles on it get the same number, one
/// the negative of the other.
fn inside_edge(a: DVec2, b: DVec2, c: DVec2) -> bool {
    let forward = (a.x, a.y) < (b.x, b.y);
    let (lo, hi) = if forward { (a, b) } else { (b, a) };
    let side = (hi - lo).perp_dot(c - lo);
    let side = if forward { side } else { -side };
    // Owned by the direction going down, or going right along a level edge.
    side > 0.0 || (side == 0.0 && (b.y < a.y || (b.y == a.y && b.x > a.x)))
}
