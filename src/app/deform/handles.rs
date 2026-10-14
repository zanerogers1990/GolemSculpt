//! The deformer's handles on the model, drawn where the gizmo's arrows would
//! be and grabbed the same way: the two ends of the range on the axis line,
//! Bend Arc's tip and the Lattice's points. A drag of one changes the card's
//! settings, and the model follows on the next frame like any other change.
//!
//! The ends of the range slide along the axis. The tip and the cage's points
//! go where the pointer takes them across the screen, in the plane through
//! the handle that faces the camera: turn the view to move one in depth.

use glam::{Vec2, Vec3};
use winit::event::MouseButton;

use super::super::{Drag, Running};
use crate::deform::{self, Kind, lattice};
use crate::gizmo::Shape;

/// How near the pointer has to be to a handle to take it, in points.
const GRAB_PX: f32 = 11.0;
const AXIS_COLORS: [[u8; 3]; 3] = [[255, 64, 64], [89, 255, 89], [77, 140, 255]];
const TIP_COLOR: [u8; 4] = [255, 214, 64, 255];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(in crate::app) enum Handle {
    /// The start of the range, or its end.
    Start,
    End,
    /// Bend Arc's tip.
    Tip,
    /// A point of the Lattice's cage.
    Point(usize),
}

/// A handle drag under way.
pub(in crate::app) struct Grab {
    handle: Handle,
    /// The plane the pointer is read on: through the handle, facing the camera
    /// as squarely as the handle's way of moving allows.
    on: Vec3,
    normal: Vec3,
    /// Where the handle was from the pointer at the press, so it does not jump to it.
    offset: Vec3,
}

fn ray_plane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = dir.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denom;
    (t > 0.0).then(|| origin + dir * t)
}

fn disc(c: Vec2, r: f32) -> Vec<Vec2> {
    (0..20).map(|k| c + Vec2::from_angle(k as f32 / 20.0 * std::f32::consts::TAU) * r).collect()
}

impl Running {
    /// Every handle the deformer has, and where it is in the world.
    pub(in crate::app) fn deform_handles(&self) -> Vec<(Handle, Vec3)> {
        let Some(session) = self.transform.deform.session.as_ref() else { return Vec::new() };
        let p = self.doc.opts.deform.params.sanitized();
        let at = |q: glam::DVec3| session.frame.to_world(q).as_vec3();
        let [start, end] = deform::range_ends(&p, &session.bounds);
        let mut out = vec![(Handle::Start, at(start)), (Handle::End, at(end))];
        match p.kind {
            Kind::BendArc => out.push((Handle::Tip, at(deform::tip_point(&p, &session.bounds)))),
            Kind::Lattice => {
                let span = session.bounds.span(p.axis, p.range);
                for i in 0..p.lattice.len() {
                    out.push((Handle::Point(i), at(lattice::point(&p.lattice, i, &session.bounds, p.axis, span))));
                }
            }
            _ => {}
        }
        out
    }

    /// The handle under the pixel `px`, if any: the tip and the cage's points
    /// before the range's ends, which they may sit on.
    fn deform_handle_at(&self, px: Vec2) -> Option<(Handle, Vec3)> {
        let reach = GRAB_PX * self.egui_ctx.pixels_per_point();
        let near = |h: &(Handle, Vec3)| self.view.project(h.1).map(|at| at.distance(px)).filter(|&d| d <= reach);
        let handles = self.deform_handles();
        let best = |range: bool| {
            handles
                .iter()
                .filter(|h| matches!(h.0, Handle::Start | Handle::End) == range)
                .filter_map(|h| near(h).map(|d| (*h, d)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(h, _)| h)
        };
        best(false).or_else(|| best(true))
    }

    /// A press on the view while the deformers are up. A press on a handle
    /// takes the drag.
    pub(in crate::app) fn deform_pressed(&mut self, button: MouseButton) -> Option<Drag> {
        if !self.deform_on() || button != MouseButton::Left {
            return None;
        }
        self.sync_view();
        let (handle, at) = self.deform_handle_at(self.input.pointer)?;
        let session = self.transform.deform.session.as_ref()?;
        let forward = self.view.forward;
        let normal = match handle {
            Handle::Start | Handle::End => {
                let axis = session.frame.rotation.as_quat() * Vec3::AXES[self.doc.opts.deform.params.axis.min(2)];
                let n = axis.cross(forward).cross(axis);
                if n.length_squared() < 1e-8 { forward } else { n.normalize() }
            }
            Handle::Tip | Handle::Point(_) => forward,
        };
        let (origin, dir) = self.view.ray(self.input.pointer);
        let hit = ray_plane(origin, dir, at, normal).unwrap_or(at);
        if let Handle::Point(i) = handle {
            self.doc.opts.deform.params.lattice.picked = i;
        }
        self.transform.deform.grab = Some(super::Grab { handle, on: at, normal, offset: at - hit });
        Some(Drag::Gizmo)
    }

    /// The pointer moved during a handle drag. True if it was one.
    pub(in crate::app) fn deform_moved(&mut self, pos: Vec2) -> bool {
        let Some(grab) = self.transform.deform.grab.as_ref() else { return false };
        let Some(session) = self.transform.deform.session.as_ref() else { return true };
        let (origin, dir) = self.view.ray(pos);
        let Some(hit) = ray_plane(origin, dir, grab.on, grab.normal) else { return true };
        let to = session.frame.from_world((hit + grab.offset).as_dvec3());
        let (bounds, handle) = (session.bounds, grab.handle);
        let p = &mut self.doc.opts.deform.params;
        let clean = p.sanitized();
        match handle {
            Handle::Start | Handle::End => {
                let (lo, hi) = (bounds.min[clean.axis], bounds.max[clean.axis]);
                let f = (((to[clean.axis] - lo) / (hi - lo).max(1e-12)) as f32).clamp(0.0, 1.0);
                // The two ends never cross: a range needs some length.
                let gap = 0.01;
                match handle {
                    Handle::Start => p.range[0] = f.min(clean.range[1] - gap).max(0.0),
                    _ => p.range[1] = f.max(clean.range[0] + gap).min(1.0),
                }
            }
            Handle::Tip => p.tip = deform::tip_for(&clean, &bounds, to),
            Handle::Point(i) => {
                let span = bounds.span(clean.axis, clean.range);
                if i < p.lattice.moved.len() {
                    p.lattice.moved[i] = lattice::moved_for(&clean.lattice, i, &bounds, clean.axis, span, to);
                }
            }
        }
        self.redraw = true;
        true
    }

    /// The button came up. True if it ended a handle drag.
    pub(in crate::app) fn deform_released(&mut self) -> bool {
        self.transform.deform.grab.take().is_some()
    }

    /// The handles as flat shapes in window pixels, for the gizmo's overlay.
    pub(in crate::app) fn deform_shapes(&self) -> Vec<Shape> {
        let Some(session) = self.transform.deform.session.as_ref().filter(|_| self.deform_on()) else { return Vec::new() };
        let ppp = self.egui_ctx.pixels_per_point();
        let p = self.doc.opts.deform.params.sanitized();
        let px = |q: glam::DVec3| self.view.project(session.frame.to_world(q).as_vec3());
        let grabbed = self.transform.deform.grab.as_ref().map(|g| g.handle);
        let hot = grabbed.or_else(|| {
            let free = self.input.drag == Drag::None && self.input.inside && !self.over_ui();
            free.then(|| self.deform_handle_at(self.input.pointer).map(|h| h.0)).flatten()
        });
        let [r, g, b] = AXIS_COLORS[p.axis];
        let mut out = Vec::new();

        // The axis: the whole box along it faint, the range bright.
        let [start, end] = deform::range_ends(&p, &session.bounds);
        let (mut first, mut last) = (start, end);
        first[p.axis] = session.bounds.min[p.axis];
        last[p.axis] = session.bounds.max[p.axis];
        if let (Some(a), Some(z)) = (px(first), px(last)) {
            out.push(Shape::Line { points: vec![a, z], width: 2.0 * ppp, color: [r, g, b, 90] });
        }
        if let (Some(a), Some(z)) = (px(start), px(end)) {
            out.push(Shape::Line { points: vec![a, z], width: 3.0 * ppp, color: [r, g, b, 255] });
        }

        if p.kind == Kind::Lattice {
            self.lattice_shapes(&p, session, hot, ppp, &mut out);
        }
        if p.kind == Kind::BendArc {
            let anchor = if p.anchor == deform::Anchor::End { end } else { start };
            if let (Some(a), Some(t)) = (px(anchor), px(deform::tip_point(&p, &session.bounds))) {
                out.push(Shape::Line { points: vec![a, t], width: 1.5 * ppp, color: [255, 214, 64, 140] });
                let size = if hot == Some(Handle::Tip) { 10.0 } else { 8.0 } * ppp;
                let diamond = |s: f32| vec![t + Vec2::new(0.0, -s), t + Vec2::new(s, 0.0), t + Vec2::new(0.0, s), t + Vec2::new(-s, 0.0)];
                out.push(Shape::Fill { points: diamond(size + 1.5 * ppp), color: [15, 15, 20, 255] });
                out.push(Shape::Fill { points: diamond(size), color: TIP_COLOR });
            }
        }
        // The ends of the range: round handles on the axis.
        for (handle, at) in [(Handle::Start, start), (Handle::End, end)] {
            let Some(c) = px(at) else { continue };
            let size = if hot == Some(handle) { 8.5 } else { 7.0 } * ppp;
            out.push(Shape::Fill { points: disc(c, size + 1.5 * ppp), color: [r, g, b, 255] });
            out.push(Shape::Fill { points: disc(c, size), color: [245, 245, 245, 255] });
        }
        out
    }

    /// The cage: its edges, then its points, the picked one in yellow.
    fn lattice_shapes(&self, p: &deform::Params, session: &super::Session, hot: Option<Handle>, ppp: f32, out: &mut Vec<Shape>) {
        let span = session.bounds.span(p.axis, p.range);
        let cage = &p.lattice;
        let at: Vec<Option<Vec2>> = (0..cage.len())
            .map(|i| self.view.project(session.frame.to_world(lattice::point(cage, i, &session.bounds, p.axis, span)).as_vec3()))
            .collect();
        for i in 0..cage.len() {
            let ijk = cage.ijk(i);
            for c in 0..3 {
                if ijk[c] + 1 >= cage.count[c] {
                    continue;
                }
                let mut next = ijk;
                next[c] += 1;
                if let (Some(a), Some(b)) = (at[i], at[cage.index(next)]) {
                    out.push(Shape::Line { points: vec![a, b], width: 1.5 * ppp, color: [235, 235, 235, 150] });
                }
            }
        }
        for (i, c) in at.iter().enumerate() {
            let Some(c) = *c else { continue };
            let picked = i == cage.picked;
            let size = if hot == Some(Handle::Point(i)) { 6.5 } else { 5.0 } * ppp;
            out.push(Shape::Fill { points: disc(c, size + 1.5 * ppp), color: [15, 15, 20, 255] });
            out.push(Shape::Fill { points: disc(c, size), color: if picked { TIP_COLOR } else { [245, 245, 245, 255] } });
        }
    }
}
