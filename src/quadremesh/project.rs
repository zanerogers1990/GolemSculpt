//! Closest point on a triangle mesh, through a uniform grid of triangle
//! bounding boxes.
//!
//! Used to put the quad remesher's output back onto the sculpt: extracted
//! vertices sit on tangent-plane lattice points, slightly off a curved
//! surface, and relaxation moves them further. Queries are expected to start
//! near the surface (within a cell or two), which is what makes the
//! expanding-ring search cheap.

use glam::Vec3;

pub struct Projector<'a> {
    v: &'a [Vec3],
    t: &'a [u32],
    origin: Vec3,
    cell: f32,
    inv_cell: f32,
    n: [i32; 3],
    cell_start: Vec<u32>,
    cell_tris: Vec<u32>,
}

const MAX_CELLS: f64 = 4_000_000.0;

impl<'a> Projector<'a> {
    /// Cells are sized from `cell_size` but grown if the grid would be huge.
    pub fn new(vertices: &'a [Vec3], triangles: &'a [u32], cell_size: f32) -> Projector<'a> {
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for &p in vertices {
            min = min.min(p);
            max = max.max(p);
        }
        let size = (max - min).max(Vec3::ZERO);
        let mut cell = cell_size.max(1e-6);
        let cells = |cell: f32| {
            let c = (size / cell).as_dvec3() + 1.0;
            c.x * c.y * c.z
        };
        while cells(cell) > MAX_CELLS {
            cell *= 1.25;
        }
        let inv_cell = 1.0 / cell;
        let n = [0, 1, 2].map(|a| (size[a] * inv_cell) as i32 + 1);
        let mut grid = Projector {
            v: vertices,
            t: triangles,
            origin: min,
            cell,
            inv_cell,
            n,
            cell_start: vec![0; (n[0] * n[1] * n[2]) as usize + 1],
            cell_tris: Vec::new(),
        };

        // Count, then fill.
        for f in 0..triangles.len() / 3 {
            grid.for_cells_of(f, |c, start, _| start[c + 1] += 1);
        }
        for c in 0..grid.cell_start.len() - 1 {
            grid.cell_start[c + 1] += grid.cell_start[c];
        }
        // The fill pass uses `cell_start` as its write cursors; the saved copy
        // puts the real starts back afterwards.
        let starts = grid.cell_start.clone();
        grid.cell_tris = vec![0; *starts.last().unwrap() as usize];
        for f in 0..triangles.len() / 3 {
            grid.for_cells_of(f, |c, cursor, tris| {
                tris[cursor[c] as usize] = f as u32;
                cursor[c] += 1;
            });
        }
        grid.cell_start = starts;
        grid
    }

    fn for_cells_of(&mut self, f: usize, mut visit: impl FnMut(usize, &mut [u32], &mut [u32])) {
        let [a, b, c] = [0, 1, 2].map(|k| self.v[self.t[f * 3 + k] as usize]);
        let lo = self.cell_of(a.min(b).min(c));
        let hi = self.cell_of(a.max(b).max(c));
        for z in lo[2]..=hi[2] {
            for y in lo[1]..=hi[1] {
                for x in lo[0]..=hi[0] {
                    let cell = ((z * self.n[1] + y) * self.n[0] + x) as usize;
                    visit(cell, &mut self.cell_start, &mut self.cell_tris);
                }
            }
        }
    }

    #[inline]
    fn cell_of(&self, p: Vec3) -> [i32; 3] {
        let c = (p - self.origin) * self.inv_cell;
        [0, 1, 2].map(|a| (c[a] as i32).clamp(0, self.n[a] - 1))
    }

    /// Closest point on the surface to `p`.
    pub fn project(&self, p: Vec3) -> Vec3 {
        self.project_with_triangle(p).0
    }

    /// The closest point, and the triangle it lies on (None on an empty mesh).
    pub fn project_with_triangle(&self, p: Vec3) -> (Vec3, Option<u32>) {
        let [cx, cy, cz] = self.cell_of(p);
        let [nx, ny, nz] = self.n;
        let mut best = f32::MAX;
        let mut best_point = p;
        let mut best_tri = None;
        for ring in 0..=nx.max(ny).max(nz) {
            // Everything within `ring - 1` cells has been seen; stop once the
            // best hit is closer than any unseen cell could be.
            if best_tri.is_some() && ring > 0 && (ring - 1) as f32 * self.cell > best.sqrt() {
                break;
            }
            for z in (cz - ring).max(0)..=(cz + ring).min(nz - 1) {
                for y in (cy - ring).max(0)..=(cy + ring).min(ny - 1) {
                    let shell = z == cz - ring || z == cz + ring || y == cy - ring || y == cy + ring;
                    let step = if shell || ring == 0 { 1 } else { 2 * ring };
                    let mut x = cx - ring;
                    while x <= cx + ring {
                        if x >= 0 && x < nx {
                            let c = ((z * ny + y) * nx + x) as usize;
                            for &f in &self.cell_tris[self.cell_start[c] as usize..self.cell_start[c + 1] as usize] {
                                let [a, b, c] = [0, 1, 2].map(|k| self.v[self.t[f as usize * 3 + k] as usize]);
                                let q = closest_on_triangle(p, a, b, c);
                                let d = q.distance_squared(p);
                                if d < best {
                                    (best, best_point, best_tri) = (d, q, Some(f));
                                }
                            }
                        }
                        x += step;
                    }
                }
            }
        }
        (best_point, best_tri)
    }
}

/// Closest point on triangle abc to p (Ericson, Real-Time Collision
/// Detection, 5.1.5).
pub fn closest_on_triangle(p: Vec3, a: Vec3, b: Vec3, c: Vec3) -> Vec3 {
    let (ab, ac, ap) = (b - a, c - a, p - a);
    let (d1, d2) = (ab.dot(ap), ac.dot(ap));
    if d1 <= 0.0 && d2 <= 0.0 {
        return a;
    }
    let bp = p - b;
    let (d3, d4) = (ab.dot(bp), ac.dot(bp));
    if d3 >= 0.0 && d4 <= d3 {
        return b;
    }
    let vc = d1 * d4 - d3 * d2;
    if vc <= 0.0 && d1 >= 0.0 && d3 <= 0.0 {
        return a + ab * (d1 / (d1 - d3));
    }
    let cp = p - c;
    let (d5, d6) = (ab.dot(cp), ac.dot(cp));
    if d6 >= 0.0 && d5 <= d6 {
        return c;
    }
    let vb = d5 * d2 - d1 * d6;
    if vb <= 0.0 && d2 >= 0.0 && d6 <= 0.0 {
        return a + ac * (d2 / (d2 - d6));
    }
    let va = d3 * d6 - d5 * d4;
    if va <= 0.0 && (d4 - d3) >= 0.0 && (d5 - d6) >= 0.0 {
        return b + (c - b) * ((d4 - d3) / ((d4 - d3) + (d5 - d6)));
    }
    let denom = 1.0 / (va + vb + vc);
    a + ab * (vb * denom) + ac * (vc * denom)
}
