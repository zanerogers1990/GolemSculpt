//! Closing the plain openings a cut leaves: each cut loop charted on the swept
//! surface, and the face built in that chart.

use glam::{DVec3, Vec2, Vec3};
use rayon::prelude::*;

use super::{LoopChart, Pass, shortest_arc_step};
use crate::{cap, planar, polygon};

/// How many interior vertices a cap may add per rim vertex before refinement
/// gives up and leaves the middle of the face coarser than the edge. A roughly
/// circular cut face at full mesh density wants about 0.18 x rim^2 of them,
/// which for a big cut on a dense model is more geometry than the cut is
/// worth, and the density that matters is the density at the rim.
const CAP_VERTICES_PER_RIM_VERTEX: usize = 40;
const MIN_CAP_VERTEX_BUDGET: usize = 512;
const MAX_CAP_VERTEX_BUDGET: usize = 60_000;

/// The face that closes one loop, worked out but not yet added to the result.
struct LoopFace {
    /// The loop's vertices in the order the face walks them.
    rim: Vec<u32>,
    fill: cap::CapFill,
    /// The face had to be filled without the swept surface to place it on.
    approximated: bool,
}

impl Pass<'_> {
    /// Closes every cut loop: plain openings with a face, the two ends of a
    /// tunnel with the wall between them.
    pub(super) fn cap_cut_loops(&mut self, loops: Vec<Vec<u32>>) {
        let charts: Vec<LoopChart> = loops.into_iter().map(|chain| self.build_chart(chain)).collect();
        // A loop that wraps the whole way around the dragged outline is one end
        // of a tunnel: the shape was drawn entirely within the object's
        // silhouette, so it punched straight through. Those close against each
        // other (the tunnel wall), not against themselves.
        if charts.iter().any(|chart| chart.wraps) {
            self.cap_round_the_outline(&charts);
        } else {
            self.cap_flat_loops(&charts);
        }
    }

    /// Closes loops none of which goes round the outline.
    fn cap_flat_loops(&mut self, flat: &[LoopChart]) {
        // A loop inside another is a hole in that one's face, not a face of its
        // own: cut a hollow part and the cavity's wall leaves a loop inside the
        // outer skin's. Capping each by itself stacks a disc over the cavity on
        // top of the outer disc: closed, but two coincident faces facing
        // opposite ways, which no slicer reads as a solid. Loops nest even-odd:
        // a loop inside a hole (a part inside the cavity) is an outer face
        // again.
        let nesting = self.find_nesting(flat);
        let holes_of = |i: usize| (0..flat.len()).filter(|&j| nesting.parent[j] == Some(i)).collect::<Vec<usize>>();
        let mut faces = self.fill_loops(flat, |i| nesting.depth[i].is_multiple_of(2) && holes_of(i).is_empty());
        for i in 0..flat.len() {
            if nesting.depth[i] % 2 == 1 {
                continue; // a hole, capped with the loop around it
            }
            let holes = holes_of(i);
            let first = self.triangle_count();
            if let Some(face) = faces[i].take() {
                self.add_loop_face(face);
            } else if holes.is_empty() || !self.cap_with_holes(&flat[i], flat, &holes, &nesting.shift) {
                self.cap_contractible_loop(&flat[i]);
                // Fallback for a ring that would not triangulate: each loop
                // on its own. Closed, if not a clean solid, so it is owned up to.
                for &hole in &holes {
                    self.cap_contractible_loop(&flat[hole]);
                }
                self.unclosed_loops += holes.len();
            }
            let end = self.triangle_count();
            if end > first {
                self.cut_faces.push(first..end);
            }
        }
    }

    /// Closes the loops of a cut that punched through: at least one of them
    /// goes the whole way round the outline.
    ///
    /// The swept surface is then a tube, and what has to be filled is the part
    /// of the tube inside the object: between the opening where the tube goes
    /// in and the one where it comes out, less wherever it passes through open
    /// air on the way (an earlier tunnel crossing this one, as the copies of a
    /// mirrored or radial cut do in the middle of the object). Laid out flat as
    /// a ring, with depth running outward, every loop becomes a closed curve
    /// in the plane: an opening of the tunnel one that goes round the centre,
    /// anything else one that does not. They nest there exactly as loops on a
    /// flat face do, and are filled the same way: each outer loop with the
    /// loops directly inside it as its holes.
    fn cap_round_the_outline(&mut self, charts: &[LoopChart]) {
        let ring = self.ring_for(charts);
        let polygons: Vec<Vec<Vec2>> = charts.iter().map(|chart| ring.outline(chart)).collect();
        let nesting = nest(&polygons, &[0.0]);
        let mean_depth = |chart: &LoopChart| chart.d.iter().sum::<f32>() / chart.d.len().max(1) as f32;
        let childless = |i: usize| !nesting.parent.contains(&Some(i));
        let mut faces =
            self.fill_loops(charts, |i| nesting.depth[i].is_multiple_of(2) && !charts[i].wraps && childless(i));
        for (i, outer) in charts.iter().enumerate() {
            if nesting.depth[i] % 2 == 1 {
                continue;
            }
            let holes: Vec<&LoopChart> =
                (0..charts.len()).filter(|&j| nesting.parent[j] == Some(i)).map(|j| &charts[j]).collect();
            let openings = holes.iter().filter(|hole| hole.wraps).count();
            let first = self.triangle_count();
            let closed = match (outer.wraps, holes.len(), openings) {
                (false, 0, _) => {
                    match faces[i].take() {
                        Some(face) => self.add_loop_face(face),
                        None => self.cap_contractible_loop(outer),
                    }
                    true
                }
                // The two ends of a plain tunnel, each going steadily round
                // the outline: its wall, built as rings.
                (true, 1, 1) if outer.backtrack.max(holes[0].backtrack) <= STEADY_BACKTRACK * ring.perimeter => {
                    let (near, far) =
                        if mean_depth(outer) <= mean_depth(holes[0]) { (outer, holes[0]) } else { (holes[0], outer) };
                    self.stitch_tunnel(near, far);
                    true
                }
                // A face with holes, or a tunnel wall with holes in it or with
                // an end too crooked for rings.
                (false, _, 0) | (true, _, 1) => self.cap_on_ring(&ring, outer, &holes),
                // A tunnel with one end missing, or with too many: geometry
                // with a boundary of its own, or a cut too tangential to
                // resolve.
                _ => false,
            };
            if !closed {
                // Each loop closed on itself instead: overlapping, but never
                // left open.
                for chart in std::iter::once(outer).chain(holes.iter().copied()) {
                    if chart.wraps {
                        self.fan_loop(chart);
                    } else {
                        self.cap_contractible_loop(chart);
                    }
                }
                self.unclosed_loops += 1 + holes.len();
            }
            let end = self.triangle_count();
            if closed && !outer.wraps && end > first {
                self.cut_faces.push(first..end);
            }
        }
    }

    /// The flat ring the swept surface is laid out on when loops go round it.
    fn ring_for(&self, charts: &[LoopChart]) -> Ring {
        let perimeter = self.region().perimeter();
        let (nearest, farthest) =
            charts.iter().flat_map(|chart| &chart.d).fold((f32::MAX, f32::MIN), |(lo, hi), &d| (lo.min(d), hi.max(d)));
        // Scales from a loop that goes round, where both axes are well exercised.
        let reference = charts.iter().find(|chart| chart.wraps).unwrap_or(&charts[0]);
        let (su, sd) = (reference.scale_u.max(1e-12), reference.scale_d.max(1e-12));
        // The ring keeps the surface's proportions if its radius grows with
        // depth at a rate proportional to itself: a step along the outline and
        // an equal step in depth then come out the same length everywhere. A
        // long narrow tunnel would need a ring thousands of times wider outside
        // than in, more than single precision can hold apart, so the growth is
        // capped and such a wall comes out stretched, but sound.
        let span = (farthest - nearest).max(1e-12);
        let growth = (std::f32::consts::TAU * sd / (perimeter * su)).min(RING_MAX_SPREAD.ln() / span);
        let growth = if growth.is_finite() && growth > 0.0 { growth } else { 1.0 / span };
        Ring { perimeter, nearest, radius: perimeter * su / std::f32::consts::TAU, growth }
    }

    /// Fills the region inside `outer` and outside `holes`, on the ring: a
    /// face with holes that the unrolled chart cannot hold because one of the
    /// loops goes round the outline, or the wall of a tunnel with holes in it.
    /// False, having emitted nothing, if the ring cannot be trusted for these
    /// loops or the region will not triangulate.
    fn cap_on_ring(&mut self, ring: &Ring, outer: &LoopChart, holes: &[&LoopChart]) -> bool {
        let mut points: Vec<Vec2> = Vec::new();
        let mut ids: Vec<u32> = Vec::new();
        let mut add_loop = |chart: &LoopChart| -> Vec<u32> {
            let first = points.len() as u32;
            points.extend(ring.outline(chart));
            ids.extend_from_slice(&chart.vertices);
            (first..points.len() as u32).collect()
        };
        let outer_loop = add_loop(outer);
        let hole_loops: Vec<Vec<u32>> = holes.iter().map(|hole| add_loop(hole)).collect();

        // The ring must give every rim vertex back, as any chart must.
        let tolerance = self.chart_tolerance();
        let on_surface = |point: Vec2| {
            let (arc, depth) = ring.arc_and_depth(point);
            self.surface_point(arc, depth)
        };
        for (point, &id) in points.iter().zip(&ids) {
            if !on_surface(*point).is_some_and(|p| p.distance(self.out_verts[id as usize]) <= tolerance) {
                return false;
            }
        }
        let Ok(mut tris) = triangulate_somehow(&points, &outer_loop, &hole_loops) else {
            return false;
        };
        let rim = points.len();
        let mut positions: Vec<Vec3> = ids.iter().map(|&v| self.out_verts[v as usize]).collect();
        let lift = |point: Vec2| on_surface(point).unwrap_or(Vec3::ZERO);
        let parents = planar::improve_on_surface(
            &mut points,
            &mut positions,
            &mut tris,
            &planar::loop_edges(&outer_loop, &hole_loops),
            &lift,
            self.shell_edge_length(),
            cap_vertex_budget(rim),
        );
        self.emit_face(&tris, ids, &positions[rim..], &parents, (outer_loop[0], outer_loop[1]));
        true
    }

    /// Adds a triangulated face to the result: its new vertices (those past
    /// the rim's, each with the three earlier ones it sits between, from
    /// which it takes its mask) and its triangles, turned to face the right
    /// way.
    fn emit_face(
        &mut self,
        tris: &[u32],
        mut ids: Vec<u32>,
        added: &[Vec3],
        parents: &[[u32; 3]],
        rim_edge: (u32, u32),
    ) {
        let mut masks: Vec<f32> = ids.iter().map(|&v| self.mask_of(v)).collect();
        for (&position, parents) in added.iter().zip(parents) {
            let mask = parents.iter().map(|&p| masks[p as usize]).sum::<f32>() / 3.0;
            masks.push(mask);
            ids.push(self.add_vertex(position, mask));
        }
        // The shell walks each rim edge one way; the face must walk it the
        // other. The triangulator's winding is its own, so it is checked on
        // one rim edge.
        let same_way = tris.chunks_exact(3).any(|t| (0..3).any(|k| t[k] == rim_edge.0 && t[(k + 1) % 3] == rim_edge.1));
        for t in tris.chunks_exact(3) {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| ids[i as usize]);
            if same_way {
                self.emit_triangle(a, c, b);
            } else {
                self.emit_triangle(a, b, c);
            }
        }
    }

    fn build_chart(&self, vertices: Vec<u32>) -> LoopChart {
        let m = vertices.len();
        let perimeter = self.region().perimeter();
        let slot = |v: u32| self.rim_slot[&v] as usize;
        let arc: Vec<f32> = vertices.iter().map(|&v| self.cut_arc[slot(v)]).collect();
        let d: Vec<f32> = vertices.iter().map(|&v| self.cut_depth[slot(v)]).collect();

        let mut u = Vec::with_capacity(m);
        u.push(arc[0]);
        let (mut forward, mut backward) = (0.0, 0.0);
        for k in 1..=m {
            let step = shortest_arc_step(arc[k % m] - arc[k - 1], perimeter);
            if k < m {
                u.push(u[k - 1] + step);
            }
            if step > 0.0 {
                forward += step;
            } else {
                backward -= step;
            }
        }
        let wind = forward - backward;
        let backtrack = forward.min(backward);

        // Half a perimeter of slack: a loop that merely stays put nets out near
        // zero, one that goes the whole way round nets out near plus or minus
        // the perimeter, and nothing real lands between.
        let wraps = perimeter > 0.0 && wind.abs() > perimeter * 0.5;
        let (scale_u, scale_d) = self.fit_chart_scales(&vertices, &arc, &d);
        LoopChart { vertices, u, d, arc, wind, wraps, backtrack, scale_u, scale_d }
    }

    /// Finds the two axis scales that make the chart as close to a true
    /// distance map of the cut surface as a linear rescale can be, by least
    /// squares over the rim's own edges: for each one,
    /// (scale_u du)^2 + (scale_d dd)^2 should come out as its real length
    /// squared.
    ///
    /// Fitted rather than derived from the camera because the two axes are not
    /// even in comparable units to begin with (pixels against view depth,
    /// while the geometry is in the object's own space), so a derivation would
    /// have to unpick the projection, the viewport and the object's scale. Two
    /// unknowns against one linear system does the same job and cannot be
    /// wrong about any of them.
    fn fit_chart_scales(&self, vertices: &[u32], arc: &[f32], depth: &[f32]) -> (f32, f32) {
        let m = vertices.len();
        let perimeter = self.region().perimeter();
        let (mut duu, mut ddd, mut dud, mut ru, mut rd) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
        for k in 0..m {
            let next = (k + 1) % m;
            let du = shortest_arc_step(arc[next] - arc[k], perimeter) as f64;
            let dd = (depth[next] - depth[k]) as f64;
            let len2 =
                self.out_verts[vertices[next] as usize].distance_squared(self.out_verts[vertices[k] as usize]) as f64;
            let (du2, dd2) = (du * du, dd * dd);
            duu += du2 * du2;
            ddd += dd2 * dd2;
            dud += du2 * dd2;
            ru += du2 * len2;
            rd += dd2 * len2;
        }

        let det = duu * ddd - dud * dud;
        if det.abs() > 1e-30 {
            let u = (ru * ddd - rd * dud) / det;
            let w = (rd * duu - ru * dud) / det;
            if u > 0.0 && w > 0.0 {
                return (u.sqrt() as f32, w.sqrt() as f32);
            }
        }
        // A rim that never moves along one axis (a cut exactly along the
        // silhouette, say). Scale the axis that does vary and leave the other
        // alone; a wrong but finite chart still triangulates.
        let scale_u = if duu > 1e-30 && ru > 0.0 { (ru / duu).sqrt() as f32 } else { 1.0 };
        let scale_d = if ddd > 1e-30 && rd > 0.0 { (rd / ddd).sqrt() as f32 } else { 1.0 };
        (scale_u, scale_d)
    }

    /// The point of the cutting surface whose swept-surface coordinates are
    /// (`arc`, `depth`): the exact inverse of how a crossing's chart
    /// coordinates were recorded, so feeding a rim vertex's own coordinates
    /// back through this returns that vertex.
    ///
    /// This is what the cut face is filled from. Its rim is exact from the
    /// start, every crossing bisected onto the boundary, but a face filled by
    /// averaging its corners is not: wherever the outline curves a chord cuts
    /// the corner, the ear clipper's first triangles span the whole face, and
    /// every later round subdivides that error rather than correcting it.
    /// Measured in the Unity app on a 359k-triangle model, cut-face points sat
    /// up to 34 screen pixels off the surface they were meant to be on, which
    /// rendered as fangs and gouges across the cut.
    ///
    /// A projection's clip w depends only on view depth (it is -z under
    /// perspective and 1 under orthographic), so the remaining unknowns are a
    /// 2x2 solve. `None` when that solve is degenerate, which cannot happen for
    /// a real camera but leaves the caller a defined fallback.
    pub(super) fn surface_point(&self, arc: f32, depth: f32) -> Option<Vec3> {
        let screen = self.region().point_at_arc(arc);
        let ndc = self.probe.view.projection.ndc(screen).as_dvec2();
        let z = -(depth as f64); // view space looks down -Z

        let m = &self.view_to_clip;
        let w = m.z_axis.w * z + m.w_axis.w;
        let (a, b, c, d) = (m.x_axis.x, m.y_axis.x, m.x_axis.y, m.y_axis.y);
        let det = a * d - b * c;
        if w.abs() < 1e-9 || det.abs() < 1e-12 {
            return None;
        }
        let rx = ndc.x * w - m.z_axis.x * z - m.w_axis.x;
        let ry = ndc.y * w - m.z_axis.y * z - m.w_axis.y;
        let view = DVec3::new((d * rx - b * ry) / det, (a * ry - c * rx) / det, z);

        // Back from the tested copy to the geometry being cut.
        let model = self.view_to_model.transform_point3(view).as_vec3();
        model.is_finite().then(|| self.probe.symmetry.apply_point(model))
    }

    /// How far the chart may put a rim vertex from where it really is and
    /// still be trusted to place the interior.
    fn chart_tolerance(&self) -> f32 {
        (self.shell_edge_length() * 0.25).max(1e-6)
    }

    /// Fills one loop that closes on itself with a face.
    fn cap_contractible_loop(&mut self, chart: &LoopChart) {
        if let Some(face) = self.fill_loop(chart) {
            self.add_loop_face(face);
        }
    }

    /// The faces of the loops `wanted` picks out, by loop, each worked out on
    /// its own core: a cut through both legs, or across a ring, has several
    /// faces and filling them is most of what a trim costs.
    fn fill_loops(&self, charts: &[LoopChart], wanted: impl Fn(usize) -> bool + Sync) -> Vec<Option<LoopFace>> {
        charts.par_iter().enumerate().map(|(i, chart)| if wanted(i) { self.fill_loop(chart) } else { None }).collect()
    }

    /// Works out the face that closes one loop, changing nothing.
    fn fill_loop(&self, chart: &LoopChart) -> Option<LoopFace> {
        let m = chart.vertices.len();
        if m < 3 {
            return None;
        }
        // Reversed: the surviving shell already walks this rim one way round,
        // and a cap sharing those edges in the same direction would face the
        // wrong way. A closed surface's two sides of an edge run opposite.
        let order: Vec<u32> = chart.vertices.iter().rev().copied().collect();
        let positions: Vec<Vec3> = order.iter().map(|&v| self.out_verts[v as usize]).collect();
        let (scale_u, scale_d) = (chart.scale_u, chart.scale_d);
        let points: Vec<Vec2> = (0..m).rev().map(|k| Vec2::new(chart.u[k] * scale_u, chart.d[k] * scale_d)).collect();

        // Where a new interior point of the face goes, given where it landed in
        // the chart. The chart handed to the triangulator is scaled into mesh
        // units, so the scale comes back off before the coordinates mean arc
        // length and depth again.
        let on_surface = |point: Vec2| -> Option<Vec3> {
            let arc = if scale_u > 1e-12 { point.x / scale_u } else { 0.0 };
            let depth = if scale_d > 1e-12 { point.y / scale_d } else { 0.0 };
            self.surface_point(arc, depth)
        };

        // The mapping is only used on a loop it can be shown to describe, and
        // there are two ways it may not.
        //
        // It must reproduce the rim. Every rim vertex was bisected onto the
        // cutting surface and had its chart coordinates read off it, so
        // putting those coordinates back through has to return the vertex
        // itself; if it does not, the chart's fitted scales or the camera's
        // inverse are not what this loop is lying on.
        //
        // And the chart must actually separate the rim's vertices. A cut
        // passing exactly through a high-valence vertex (a UV sphere's pole,
        // where 160 triangles fan into one point) makes every edge around it
        // cross at that same point, so a long run of the rim arrives with
        // identical chart coordinates. That is not a parametrisation of
        // anything: the refinement cannot tell those triangles apart to
        // subdivide them. Averaging the corners handles that case perfectly
        // well (the face there is flat, the one case a chord is on the
        // surface).
        //
        // Measured as a share of the rim, not as "any coincidence at all". On
        // a dense sculpt two crossings landing on the same point is ordinary
        // luck, and one such pair must not throw away the mapping for the
        // whole loop. A singularity is not subtle: it collapses half the rim,
        // not a thousandth of it.
        let tolerance = self.chart_tolerance();
        let mut coincident = 0;
        let mut trustworthy = true;
        for k in 0..m {
            if points[k] == points[(k + m - 1) % m] {
                coincident += 1;
            } else if !on_surface(points[k]).is_some_and(|p| p.distance(positions[k]) <= tolerance) {
                trustworthy = false;
                break;
            }
        }
        if coincident * 10 > m {
            trustworthy = false;
        }

        // Filled at the density of the surface it was cut from, rather than
        // left as a bare fan of the rim (see `cap` for what a bare fan does).
        let lift = |point: Vec2| on_surface(point).unwrap_or(Vec3::ZERO);
        let fill = cap::fill_loop(
            &positions,
            &points,
            trustworthy.then_some(&lift as &dyn Fn(Vec2) -> Vec3),
            self.shell_edge_length(),
            cap_vertex_budget(m),
        );
        Some(LoopFace { rim: order, fill, approximated: !trustworthy })
    }

    /// Adds a loop's face to the result.
    fn add_loop_face(&mut self, face: LoopFace) {
        self.chart_mapping_rejected += face.approximated as usize;
        // The triangulator numbers the rim from zero in the order it was
        // handed, and anything it added after that.
        let mut ids = face.rim;
        let mut masks: Vec<f32> = ids.iter().map(|&v| self.mask_of(v)).collect();
        for (position, parents) in face.fill.extra_positions.iter().zip(&face.fill.extra_parents) {
            let mask = parents.iter().map(|&p| masks[p as usize]).sum::<f32>() / 3.0;
            masks.push(mask);
            ids.push(self.add_vertex(*position, mask));
        }
        for t in face.fill.triangles.chunks_exact(3) {
            self.emit_triangle(ids[t[0] as usize], ids[t[1] as usize], ids[t[2] as usize]);
        }
    }

    /// Caps an outer loop and the holes inside it as one face (a ring, for a
    /// hollow part), filled on the cutting surface like a single loop's face.
    /// False, having emitted nothing, if the chart cannot be trusted for these
    /// loops or the ring will not triangulate; the caller then caps each loop
    /// by itself.
    fn cap_with_holes(&mut self, outer: &LoopChart, all: &[LoopChart], holes: &[usize], shift: &[f32]) -> bool {
        let (su, sd) = (outer.scale_u, outer.scale_d);
        if !(su > 1e-12 && sd > 1e-12) {
            return false;
        }
        let mut points: Vec<Vec2> = Vec::new();
        let mut ids: Vec<u32> = Vec::new();
        let mut add_loop = |chart: &LoopChart, shift: f32| -> Vec<u32> {
            let first = points.len() as u32;
            for k in 0..chart.vertices.len() {
                points.push(Vec2::new((chart.u[k] + shift) * su, chart.d[k] * sd));
                ids.push(chart.vertices[k]);
            }
            (first..points.len() as u32).collect()
        };
        let outer_loop = add_loop(outer, 0.0);
        let hole_loops: Vec<Vec<u32>> = holes.iter().map(|&h| add_loop(&all[h], shift[h])).collect();

        // The same check a single loop's chart gets, over every rim here: it
        // must reproduce the rim.
        let tolerance = self.chart_tolerance();
        let on_surface = |point: Vec2| self.surface_point(point.x / su, point.y / sd);
        for (point, &id) in points.iter().zip(&ids) {
            if !on_surface(*point).is_some_and(|p| p.distance(self.out_verts[id as usize]) <= tolerance) {
                return false;
            }
        }

        let Ok(mut tris) = triangulate_somehow(&points, &outer_loop, &hole_loops) else {
            return false;
        };
        let rim = points.len();
        let constraints = planar::loop_edges(&outer_loop, &hole_loops);
        let parents =
            planar::improve(&mut points, &mut tris, &constraints, self.shell_edge_length(), cap_vertex_budget(rim));

        // Each new point goes onto the cutting surface, or failing that
        // between the three it was made among.
        let mut positions: Vec<Vec3> = ids.iter().map(|&v| self.out_verts[v as usize]).collect();
        for (i, parents) in parents.iter().enumerate() {
            let mean = parents.iter().map(|&p| positions[p as usize]).sum::<Vec3>() / 3.0;
            positions.push(self.surface_point(points[rim + i].x / su, points[rim + i].y / sd).unwrap_or(mean));
        }
        self.emit_face(&tris, ids, &positions[rim..], &parents, (outer_loop[0], outer_loop[1]));
        true
    }

    /// How the loops of a flat face nest, in the unrolled chart. A loop's arc
    /// coordinate is unwrapped from wherever it starts, so two loops on either
    /// side of the outline's seam can be a perimeter apart in the chart while
    /// side by side on the cut; each is tried shifted a perimeter either way.
    fn find_nesting(&self, loops: &[LoopChart]) -> Nesting {
        let perimeter = self.region().perimeter();
        let polygons: Vec<Vec<Vec2>> =
            loops.iter().map(|chart| chart.u.iter().zip(&chart.d).map(|(&u, &d)| Vec2::new(u, d)).collect()).collect();
        nest(&polygons, &[0.0, perimeter, -perimeter])
    }
}

/// The swept surface laid out flat as a ring: position along the outline is
/// the angle round it and depth runs outward, so the surface's seam (where the
/// outline closes) disappears.
struct Ring {
    perimeter: f32,
    /// The depth that sits at `radius`.
    nearest: f32,
    radius: f32,
    /// How fast the radius grows with depth, as a rate per unit of depth.
    growth: f32,
}

/// How many times wider the outside of the ring may be than the inside.
const RING_MAX_SPREAD: f32 = 1000.0;

/// The most a tunnel's opening may double back along the outline, as a share
/// of the outline's length, and still have its wall built as rings. Rings need
/// each opening's depth to be a function of position along the outline, which
/// a loop that doubles back does not give.
const STEADY_BACKTRACK: f32 = 0.002;

impl Ring {
    fn point(&self, arc: f32, depth: f32) -> Vec2 {
        let angle = arc / self.perimeter * std::f32::consts::TAU;
        Vec2::from_angle(angle) * (self.radius * ((depth - self.nearest) * self.growth).exp())
    }

    /// The inverse of `point`.
    fn arc_and_depth(&self, point: Vec2) -> (f32, f32) {
        let angle = point.y.atan2(point.x).rem_euclid(std::f32::consts::TAU);
        (
            angle / std::f32::consts::TAU * self.perimeter,
            self.nearest + (point.length() / self.radius).ln() / self.growth,
        )
    }

    fn outline(&self, chart: &LoopChart) -> Vec<Vec2> {
        chart.arc.iter().zip(&chart.d).map(|(&arc, &depth)| self.point(arc, depth)).collect()
    }
}

/// For each closed curve: how many of the others contain it, the innermost of
/// them, and which of `shifts` (tried along x) showed it to be inside. A
/// nested loop only counts as a hole if it runs the opposite way round to its
/// parent, which is what a cavity's wall does; anything else is left to be
/// capped by itself.
fn nest(polygons: &[Vec<Vec2>], shifts: &[f32]) -> Nesting {
    let n = polygons.len();
    let areas: Vec<f64> = polygons.iter().map(|polygon| polygon::signed_area(polygon)).collect();
    let bounds: Vec<(Vec2, Vec2)> = polygons
        .iter()
        .map(|polygon| polygon.iter().fold((Vec2::MAX, Vec2::MIN), |(lo, hi), &p| (lo.min(p), hi.max(p))))
        .collect();

    // For each loop, the loops that contain it and the shift that showed it.
    let mut containers: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
    for j in 0..n {
        let probe = polygons[j][0];
        for i in (0..n).filter(|&i| i != j) {
            let (lo, hi) = bounds[i];
            for &shift in shifts {
                let p = probe + Vec2::new(shift, 0.0);
                if p.cmpge(lo).all() && p.cmple(hi).all() && polygon::contains(&polygons[i], p) {
                    containers[j].push((i, shift));
                    break;
                }
            }
        }
    }

    let mut nesting =
        Nesting { depth: containers.iter().map(Vec::len).collect(), parent: vec![None; n], shift: vec![0.0; n] };
    for j in 0..n {
        for &(i, shift) in &containers[j] {
            if containers[i].len() + 1 == containers[j].len() && areas[i] * areas[j] < 0.0 {
                nesting.parent[j] = Some(i);
                nesting.shift[j] = shift;
            }
        }
    }
    // A nested loop whose winding does not make it a hole is its own outer
    // face.
    for j in 0..n {
        if nesting.depth[j] % 2 == 1 && nesting.parent[j].is_none() {
            nesting.depth[j] = 0;
        }
    }
    nesting
}

struct Nesting {
    depth: Vec<usize>,
    parent: Vec<Option<usize>>,
    shift: Vec<f32>,
}

/// A region with holes triangulated cleanly if it can be, and failing that as
/// one sheet with some triangles overlapping. Either is better than the
/// alternative, which is to close the outline and each hole separately and
/// leave them lying on top of one another.
fn triangulate_somehow(points: &[Vec2], outer: &[u32], holes: &[Vec<u32>]) -> Result<Vec<u32>, String> {
    planar::triangulate(points, outer, holes).or_else(|_| planar::triangulate_lenient(points, outer, holes))
}

/// How many interior vertices the face inside a rim of `rim` vertices may add.
pub(crate) fn cap_vertex_budget(rim: usize) -> usize {
    (rim * CAP_VERTICES_PER_RIM_VERTEX).clamp(MIN_CAP_VERTEX_BUDGET, MAX_CAP_VERTEX_BUDGET)
}
