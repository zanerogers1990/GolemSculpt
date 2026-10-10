//! Tunnel walls: closing the two openings of a hole punched straight through
//! the object against each other.

use glam::Vec3;

use super::{LoopChart, Pass, shortest_arc_step};

/// Ceilings on the rings a tunnel wall may be divided into (see
/// `stitch_tunnel`). The same bargain the cap's vertex budget makes: match the
/// surrounding density where that is affordable, and stop before a deep hole
/// through a dense model adds more geometry than the trim took away.
const MAX_TUNNEL_RINGS: usize = 400;
const MAX_TUNNEL_RING_VERTICES: usize = 4000;
const MAX_TUNNEL_WALL_VERTICES: usize = 200_000;

/// A wrapping loop's samples sorted by position along the outline, so depth
/// (and mask) can be read off at any arc.
struct ArcTable {
    arc: Vec<f32>,
    depth: Vec<f32>,
    mask: Vec<f32>,
}

impl Pass<'_> {
    /// Joins two loops that each wrap once around the outline into a tube.
    ///
    /// The two are traversed in opposite directions by their own shell
    /// windings (that is forced, since a tube's two rims bound the same
    /// consistently oriented surface), so one is walked forwards and the other
    /// backwards to get both running the same way around the outline. Then a
    /// plain zipper: advance whichever side is proportionally behind and emit
    /// one triangle per step, which handles the two rims having quite
    /// different vertex counts.
    ///
    /// The zipper alone only ever produces triangles that reach the whole way
    /// from one rim to the other. That is fine on a thin wall and badly wrong
    /// on anything else: the rims are sampled at the density of the surface
    /// while the tube is as long as the model is thick, so every triangle
    /// comes out as a needle spanning that whole distance (measured in the
    /// Unity app on a 359k-triangle sculpt: a wall of 100% slivers, triangles
    /// 417 times a shell triangle).
    ///
    /// So the tube is built with intermediate rings, spaced about one mesh
    /// edge apart down its length. Each ring is evaluated straight off the
    /// cutting surface, which makes the wall a proper quad grid instead of a
    /// fan of needles, and leaves both rims untouched and still welded to the
    /// shell.
    pub(super) fn stitch_tunnel(&mut self, first: &LoopChart, second: &LoopChart) {
        if first.wind * second.wind >= 0.0 {
            // Same direction: not the two ends of one tube after all. Close
            // them separately rather than stitching a twisted surface.
            self.fan_loop(first);
            self.fan_loop(second);
            self.unclosed_loops += 2;
            return;
        }
        let perimeter = self.region().perimeter();
        let (a, a_arc) = (&first.vertices, &first.arc);
        let mut b: Vec<u32> = second.vertices.iter().rev().copied().collect();
        let mut b_arc: Vec<f32> = second.arc.iter().rev().copied().collect();

        // Line the two rims up before stitching, or the tube comes out twisted.
        let offset = nearest_arc_index(&b_arc, a_arc[0], perimeter);
        b.rotate_left(offset);
        b_arc.rotate_left(offset);

        // Both rims and every ring start at the same place on the outline, so
        // the strips line up instead of spiralling. The two end strips join an
        // irregular rim to an evenly sampled ring; the many strips in between
        // are ring to ring and become plain quads.
        let rings = self.tunnel_rings(first, second);
        let mut near: &[u32] = a;
        let mut along_near = arc_fractions(a_arc, perimeter);
        for (r, ring) in rings.iter().enumerate() {
            let along_ring = even_fractions(ring.len());
            if r > 0 && near.len() == ring.len() {
                self.zip_evenly(near, ring);
            } else {
                self.zip_unevenly(near, &along_near, ring, &along_ring);
            }
            near = ring;
            along_near = along_ring;
        }
        self.zip_unevenly(near, &along_near, &b, &arc_fractions(&b_arc, perimeter));
    }

    /// The intermediate rings of a tunnel wall, from the one nearest `first`
    /// outwards. Empty when the wall is already about one mesh edge deep,
    /// which is the case the plain zipper was always right for.
    fn tunnel_rings(&mut self, first: &LoopChart, second: &LoopChart) -> Vec<Vec<u32>> {
        let target = self.shell_edge_length();
        let perimeter = self.region().perimeter();
        if target <= 0.0 || perimeter <= 0.0 {
            return Vec::new();
        }
        // Each rim's depth as a function of position along the outline. A loop
        // that wraps exactly once covers every arc position exactly once, so
        // sorting its samples by arc gives a table that can be interpolated.
        let (near, far) = (self.arc_table(first), self.arc_table(second));
        let count = near.arc.len();
        let between = |arc: f32, t: f32, near_values: &[f32], far_values: &[f32]| {
            let from = sample(&near.arc, near_values, arc, perimeter);
            from + (sample(&far.arc, far_values, arc, perimeter) - from) * t
        };

        // How deep the wall is, measured in the geometry rather than in depth
        // units (under perspective a step in depth is not a fixed distance).
        // Sized from the deepest column, not the average one: every column is
        // cut into the same number of rings (they have to be, or the rings
        // stop being closed loops), so the deepest column decides how tall the
        // tallest quad ends up.
        let mut span = 0.0f32;
        for k in 0..count {
            let near_point = self.surface_point(near.arc[k], near.depth[k]);
            let far_point = self.surface_point(near.arc[k], between(near.arc[k], 1.0, &near.depth, &far.depth));
            if let (Some(p), Some(q)) = (near_point, far_point) {
                span = span.max(p.distance(q));
            }
        }
        let mut rings = tunnel_ring_count(span, target);
        if rings == 0 {
            return Vec::new();
        }

        // How many vertices go round each ring, from the wall's real width at
        // mid depth. The rings are sampled evenly along the outline rather
        // than reusing the near rim's own vertices: a cut crosses a triangle
        // on a chord, so consecutive crossings sit anywhere from a whole mesh
        // edge apart to almost on top of each other, and inheriting that
        // spacing for every ring repeats the rim's worst gap all the way down
        // the tunnel.
        let mut width = 0.0f32;
        for k in 0..count {
            let (arc, next) = (near.arc[k], near.arc[(k + 1) % count]);
            let mid = between(arc, 0.5, &near.depth, &far.depth);
            if let (Some(p), Some(q)) = (self.surface_point(arc, mid), self.surface_point(next, mid)) {
                width += p.distance(q);
            }
        }
        let around = tunnel_ring_vertices(width, target);
        rings = rings.clamp(1, (MAX_TUNNEL_WALL_VERTICES / around).max(1));

        // Anchored to where the near rim starts and running the same way round
        // it, so every ring is in phase with both rims rather than spiralling
        // against them.
        let origin = first.arc[0];
        let direction = if first.wind < 0.0 { -1.0 } else { 1.0 };
        let mut built = Vec::with_capacity(rings);
        for r in 1..=rings {
            let t = r as f32 / (rings + 1) as f32;
            let mut ring = Vec::with_capacity(around);
            for k in 0..around {
                let arc = origin + direction * perimeter * k as f32 / around as f32;
                let depth = between(arc, t, &near.depth, &far.depth);
                let mask = between(arc, t, &near.mask, &far.mask);
                let position = self.surface_point(arc, depth).unwrap_or(Vec3::ZERO);
                ring.push(self.add_vertex(position, mask));
            }
            built.push(ring);
        }
        built
    }

    fn arc_table(&self, chart: &LoopChart) -> ArcTable {
        let mut order: Vec<usize> = (0..chart.vertices.len()).collect();
        order.sort_by(|&x, &y| chart.arc[x].total_cmp(&chart.arc[y]));
        ArcTable {
            arc: order.iter().map(|&k| chart.arc[k]).collect(),
            depth: order.iter().map(|&k| chart.d[k]).collect(),
            mask: order.iter().map(|&k| self.mask_of(chart.vertices[k])).collect(),
        }
    }

    /// Two rings with the same vertex count and the same arc positions: a
    /// plain quad strip. Each quad is split along its shorter diagonal, since
    /// always cutting the same way turns every narrow quad into two slivers.
    fn zip_evenly(&mut self, near: &[u32], far: &[u32]) {
        let m = near.len();
        for k in 0..m {
            let k1 = (k + 1) % m;
            let (a, b, c, d) = (near[k], far[k], far[k1], near[k1]);
            let at = |v: u32| self.out_verts[v as usize];
            if at(a).distance_squared(at(c)) <= at(b).distance_squared(at(d)) {
                self.emit_triangle(a, b, c);
                self.emit_triangle(a, c, d);
            } else {
                self.emit_triangle(a, b, d);
                self.emit_triangle(b, c, d);
            }
        }
    }

    /// Two rings with different vertex counts: advance whichever side has got
    /// less far round the outline and emit one triangle per step.
    ///
    /// Position round the outline, not position in the list. The two are the
    /// same only if both rings are evenly sampled, and a rim never is, so
    /// stepping by index against an evenly sampled ring drifts out of phase
    /// and leaves a long triangle reaching back to catch up. `along_a[i]` is
    /// how far round entry `i` sits, as a fraction of the whole way round from
    /// that ring's first vertex (one more entry than the ring, the last being
    /// 1).
    fn zip_unevenly(&mut self, a: &[u32], along_a: &[f32], b: &[u32], along_b: &[f32]) {
        let (ma, mb) = (a.len(), b.len());
        if ma == 0 || mb == 0 {
            return;
        }
        let (mut i, mut j) = (0, 0);
        for _ in 0..ma + mb {
            let advance_a = if i >= ma { false } else { j >= mb || along_a[i + 1] <= along_b[j + 1] };
            let (av, bv) = (a[i % ma], b[j % mb]);
            if advance_a {
                self.emit_triangle(av, bv, a[(i + 1) % ma]);
                i += 1;
            } else {
                self.emit_triangle(av, bv, b[(j + 1) % mb]);
                j += 1;
            }
        }
    }

    /// Last-resort closure: a fan from the loop's first vertex. Right only for
    /// a loop that is star-shaped from that point, but it always produces a
    /// closed surface, which is the property that matters when the alternative
    /// is a hole.
    pub(super) fn fan_loop(&mut self, chart: &LoopChart) {
        let ring = &chart.vertices;
        for k in (2..ring.len()).rev() {
            self.emit_triangle(ring[0], ring[k], ring[k - 1]);
        }
    }
}

/// How many rings to put between the two ends of a tunnel `span` deep so they
/// sit about `target` apart: none when the wall is a single edge deep. Capped
/// for the same reason the cap has a vertex budget.
pub(crate) fn tunnel_ring_count(span: f32, target: f32) -> usize {
    // A float-to-integer cast saturates, and turns "not a number" into zero.
    ((span / target).round() as usize).saturating_sub(1).clamp(0, MAX_TUNNEL_RINGS)
}

/// How many vertices go round a tunnel ring `width` long.
pub(crate) fn tunnel_ring_vertices(width: f32, target: f32) -> usize {
    ((width / target).round() as usize).clamp(8, MAX_TUNNEL_RING_VERTICES)
}

/// Linear interpolation through a table sorted by arc, wrapping round the
/// outline.
fn sample(arc: &[f32], value: &[f32], at: f32, perimeter: f32) -> f32 {
    let n = arc.len();
    if n == 0 {
        return 0.0;
    }
    if n == 1 {
        return value[0];
    }
    let lerp = |from: f32, to: f32, t: f32| from + (to - from) * t.clamp(0.0, 1.0);
    let at = if perimeter > 0.0 { at - perimeter * (at / perimeter).floor() } else { at };
    if at <= arc[0] || at >= arc[n - 1] {
        // Between the last sample and the first, the short way round the seam.
        let wrap = arc[0] + perimeter - arc[n - 1];
        let along = if at >= arc[n - 1] { at - arc[n - 1] } else { at + perimeter - arc[n - 1] };
        return lerp(value[n - 1], value[0], if wrap > 1e-6 { along / wrap } else { 0.0 });
    }
    let (mut lo, mut hi) = (0, n - 1);
    while lo + 1 < hi {
        let mid = (lo + hi) >> 1;
        if arc[mid] <= at {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let step = arc[hi] - arc[lo];
    lerp(value[lo], value[hi], if step > 1e-6 { (at - arc[lo]) / step } else { 0.0 })
}

/// How far round the outline each entry of a rim sits, as a fraction of the
/// whole way round from its own first vertex, with a closing entry of 1.
/// Accumulated step by step rather than differenced against the start, so it
/// stays monotone across the outline's seam.
fn arc_fractions(arc: &[f32], perimeter: f32) -> Vec<f32> {
    let n = arc.len();
    let mut along = Vec::with_capacity(n + 1);
    along.push(0.0f32);
    for k in 1..=n {
        along.push(along[k - 1] + shortest_arc_step(arc[k % n] - arc[k - 1], perimeter).abs());
    }
    let total = if along[n] > 1e-6 { along[n] } else { 1.0 };
    along.iter_mut().for_each(|a| *a /= total);
    along
}

/// The same, for a ring this module sampled evenly itself.
fn even_fractions(count: usize) -> Vec<f32> {
    (0..=count).map(|k| k as f32 / count.max(1) as f32).collect()
}

fn nearest_arc_index(arc: &[f32], target: f32, perimeter: f32) -> usize {
    let mut best = (0, f32::MAX);
    for (k, &a) in arc.iter().enumerate() {
        let distance = shortest_arc_step(a - target, perimeter).abs();
        if distance < best.1 {
            best = (k, distance);
        }
    }
    best.0
}
