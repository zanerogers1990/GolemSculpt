//! The test models. Every one is closed or cleanly open, wound outward
//! consistently, and shares its points between neighbouring faces, the way
//! the app's meshes are. Quad models also return their polygon faces in the
//! app's form, with triangles cut from each quad as fans from its first
//! corner.

use std::collections::HashMap;

use glam::Vec3;

/// A model: points, triangles, and the polygon faces (`start`, `corners`) if
/// it was built of quads.
pub struct Model {
    pub positions: Vec<Vec3>,
    pub triangles: Vec<u32>,
    pub polys: Option<(Vec<u32>, Vec<u32>)>,
}

impl Model {
    pub fn triangle_count(&self) -> usize {
        self.triangles.len() / 3
    }

    pub fn polys(&self) -> Option<(&[u32], &[u32])> {
        self.polys.as_ref().map(|(s, c)| (s.as_slice(), c.as_slice()))
    }
}

/// Collects quads into triangles (two per quad, fanned from corner 0) and
/// polygon faces.
#[derive(Default)]
struct Quads {
    triangles: Vec<u32>,
    start: Vec<u32>,
    corners: Vec<u32>,
}

impl Quads {
    fn push(&mut self, q: [u32; 4]) {
        if self.start.is_empty() {
            self.start.push(0);
        }
        self.triangles.extend([q[0], q[1], q[2], q[0], q[2], q[3]]);
        self.corners.extend(q);
        self.start.push(self.corners.len() as u32);
    }

    fn model(self, positions: Vec<Vec3>, keep_polys: bool) -> Model {
        Model { positions, triangles: self.triangles, polys: keep_polys.then_some((self.start, self.corners)) }
    }
}

/// An icosahedron subdivided `levels` times, pushed out onto the unit
/// sphere: `20 * 4^levels` triangles of nearly equal size.
pub fn icosphere(levels: u32) -> Model {
    let g = (1.0 + 5f32.sqrt()) / 2.0;
    let mut positions: Vec<Vec3> = [
        [-1.0, g, 0.0],
        [1.0, g, 0.0],
        [-1.0, -g, 0.0],
        [1.0, -g, 0.0],
        [0.0, -1.0, g],
        [0.0, 1.0, g],
        [0.0, -1.0, -g],
        [0.0, 1.0, -g],
        [g, 0.0, -1.0],
        [g, 0.0, 1.0],
        [-g, 0.0, -1.0],
        [-g, 0.0, 1.0],
    ]
    .iter()
    .map(|&p| Vec3::from_array(p).normalize())
    .collect();
    let mut triangles: Vec<u32> = vec![
        0, 11, 5, 0, 5, 1, 0, 1, 7, 0, 7, 10, 0, 10, 11, 1, 5, 9, 5, 11, 4, 11, 10, 2, 10, 7, 6, 7, 1, 8, 3, 9, 4, 3,
        4, 2, 3, 2, 6, 3, 6, 8, 3, 8, 9, 4, 9, 5, 2, 4, 11, 6, 2, 10, 8, 6, 7, 9, 8, 1,
    ];
    for _ in 0..levels {
        let mut middle: HashMap<(u32, u32), u32> = HashMap::new();
        let mut finer = Vec::with_capacity(triangles.len() * 4);
        for t in triangles.chunks_exact(3) {
            let mut mid = |a: u32, b: u32| {
                *middle.entry((a.min(b), a.max(b))).or_insert_with(|| {
                    let p = (positions[a as usize] + positions[b as usize]).normalize();
                    positions.push(p);
                    positions.len() as u32 - 1
                })
            };
            let (ab, bc, ca) = (mid(t[0], t[1]), mid(t[1], t[2]), mid(t[2], t[0]));
            finer.extend([t[0], ab, ca, t[1], bc, ab, t[2], ca, bc, ab, bc, ca]);
        }
        triangles = finer;
    }
    outward(&positions, &mut triangles);
    Model { positions, triangles, polys: None }
}

/// Turns every triangle of a closed model round the origin to face away
/// from it.
fn outward(positions: &[Vec3], triangles: &mut [u32]) {
    for t in triangles.chunks_exact_mut(3) {
        let [a, b, c] = [0, 1, 2].map(|k| positions[t[k] as usize]);
        if (b - a).cross(c - a).dot(a + b + c) < 0.0 {
            t.swap(1, 2);
        }
    }
}

/// The cube from -1 to 1, each side cut into `n` by `n` quads. With
/// `keep_polys` the quads are handed back as polygon faces.
pub fn cube(n: u32, keep_polys: bool) -> Model {
    let mut index: HashMap<[u32; 3], u32> = HashMap::new();
    let mut positions = Vec::new();
    let mut point = |p: [u32; 3]| {
        *index.entry(p).or_insert_with(|| {
            positions.push(Vec3::from_array(p.map(|x| x as f32 / n as f32 * 2.0 - 1.0)));
            positions.len() as u32 - 1
        })
    };
    let mut quads = Quads::default();
    for axis in 0..3 {
        for high in [false, true] {
            // Across and down on this side, turned so across x down points out.
            let (mut u, mut v) = ((axis + 1) % 3, (axis + 2) % 3);
            if !high {
                std::mem::swap(&mut u, &mut v);
            }
            let at = |i: u32, j: u32| {
                let mut p = [0; 3];
                p[axis] = if high { n } else { 0 };
                p[u] = i;
                p[v] = j;
                p
            };
            for i in 0..n {
                for j in 0..n {
                    quads.push([point(at(i, j)), point(at(i + 1, j)), point(at(i + 1, j + 1)), point(at(i, j + 1))]);
                }
            }
        }
    }
    quads.model(positions, keep_polys)
}

/// A torus round the z axis, `around` quads round the ring and `across`
/// round the tube, with `bumps` (height, count round the ring, count round
/// the tube) raised on the tube like sculpted detail.
pub fn torus(around: u32, across: u32, bumps: (f32, f32, f32), keep_polys: bool) -> Model {
    let (major, minor) = (1.0f32, 0.35f32);
    let (height, ring_count, tube_count) = bumps;
    let mut positions = Vec::with_capacity((around * across) as usize);
    for i in 0..around {
        let a = i as f32 / around as f32 * std::f32::consts::TAU;
        for j in 0..across {
            let b = j as f32 / across as f32 * std::f32::consts::TAU;
            let r = minor * (1.0 + height * (a * ring_count).sin() * (b * tube_count).sin());
            positions.push(Vec3::new((major + r * b.cos()) * a.cos(), (major + r * b.cos()) * a.sin(), r * b.sin()));
        }
    }
    let at = |i: u32, j: u32| (i % around) * across + (j % across);
    let mut quads = Quads::default();
    for i in 0..around {
        for j in 0..across {
            quads.push([at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    quads.model(positions, keep_polys)
}

/// `count` loose squares, sharing no points, each tilted a little its own
/// way: every one is a chart of its own.
pub fn loose_squares(count: u32) -> Model {
    let side = (count as f32).sqrt().ceil() as u32;
    let mut positions = Vec::with_capacity(count as usize * 4);
    let mut quads = Quads::default();
    for k in 0..count {
        let corner = Vec3::new((k % side) as f32 * 2.0, (k / side) as f32 * 2.0, 0.0);
        let tilt = (k % 7) as f32 * 0.1;
        let first = positions.len() as u32;
        positions.extend([
            corner,
            corner + Vec3::new(1.0, 0.0, tilt),
            corner + Vec3::new(1.0, 1.0, tilt),
            corner + Vec3::new(0.0, 1.0, 0.0),
        ]);
        quads.push([first, first + 1, first + 2, first + 3]);
    }
    quads.model(positions, false)
}

/// A flat square of `n` by `n` quads with the middle third cut out.
pub fn holed_grid(n: u32) -> Model {
    let mut positions = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            positions.push(Vec3::new(i as f32 / n as f32, j as f32 / n as f32, 0.0));
        }
    }
    let at = |i: u32, j: u32| j * (n + 1) + i;
    let hole = n / 3..n - n / 3;
    let mut quads = Quads::default();
    for j in 0..n {
        for i in 0..n {
            if hole.contains(&i) && hole.contains(&j) {
                continue;
            }
            quads.push([at(i, j), at(i + 1, j), at(i + 1, j + 1), at(i, j + 1)]);
        }
    }
    quads.model(positions, false)
}
