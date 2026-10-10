//! The transform gizmo: move arrows, rotate rings and scale cubes on one
//! widget, as in the Unity app. Which of the three a drag does is decided by
//! what you grab, not by a mode button.
//!
//! It drives a list of targets and never learns what any of them is: the
//! selected objects, or something a tool points it at (a control point, one
//! copy of an array). One target is moved, turned and scaled about itself;
//! several go together about their shared centre.
//!
//! This is the maths only: where the handles are, which one a pixel is on,
//! what a drag of each kind does to the targets, and the flat shapes that
//! draw it. It is drawn over the viewport (never hidden inside the model)
//! and picked in screen space, so what you can see is what you can grab.

use glam::{DMat3, DMat4, DVec3, Quat, Vec2, Vec3};

use crate::camera::View;
use crate::scene::Object;

/// Lengths along an arm, as fractions of the arm length. The ring, the scale
/// cube and the arrowhead each own a clear stretch of it.
const RING: f32 = 0.63;
const SCALE_CUBE: f32 = 0.82;
const SCALE_CUBE_HALF: f32 = 0.055;
const ARROW_BASE: f32 = 1.0;
const ARROW_TIP: f32 = 1.24;
const ARROW_RADIUS: f32 = 0.09;
const CENTRE_HALF: f32 = 0.08;
const RING_SEGMENTS: usize = 72;

/// Gizmo size relative to what it sits on, and its limits on screen.
const ARM_PER_RADIUS: f32 = 1.8;
const MIN_ARM_PX: f32 = 70.0;
const MAX_ARM_SCREEN: f32 = 0.3;

/// An axis pointing within about 10 degrees of straight into the screen is
/// put away: dragging along it is all depth, and its handles would pile up
/// on the centre cube.
const AXIS_CULL: f32 = 0.985;
/// Below this a ring is too close to edge-on for its plane to be dragged
/// on; it turns by sideways travel instead.
const RING_EDGE_ON: f32 = 0.25;
/// Arrows and cubes share their arms while their two sets of axes are within
/// this angle (half a degree) of each other.
const FRAMES_AGREE: f32 = 0.0087;
/// The middle left to a tool's own target (see `Frame::keeping_clear`): at
/// least this many points across, at most this fraction of the arm, so the
/// rings and cubes always stay outside it.
const CLEAR_PX: f32 = 12.0;
const CLEAR_MAX: f32 = 0.5;
const MIN_SCALE: f32 = 0.02;
const UNIFORM_PER_PX: f32 = 0.004;
const GRAB_PX: f32 = 7.0;

const AXIS_COLORS: [[u8; 3]; 3] = [[255, 64, 64], [89, 255, 89], [77, 140, 255]];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Move(usize),
    Rotate(usize),
    Scale(usize),
    /// The cube at the centre: scales every axis together.
    Uniform,
}

impl Handle {
    pub fn name(self) -> &'static str {
        match self {
            Handle::Move(_) => "Move",
            Handle::Rotate(_) => "Rotate",
            Handle::Scale(_) | Handle::Uniform => "Scale",
        }
    }
}

/// Which way the arrows and rings point. The scale cubes never follow it:
/// size is kept along an object's own axes, and stretching a turned object
/// along a world axis would be a shear no placement can hold.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Orientation {
    /// One object's own axes; the world's when several are selected, which
    /// have no single frame to borrow.
    #[default]
    Auto,
    /// The active object's own axes, always.
    Local,
    /// The world's axes, always.
    Global,
}

impl Orientation {
    pub const ALL: [Orientation; 3] = [Orientation::Auto, Orientation::Local, Orientation::Global];

    pub fn name(self) -> &'static str {
        match self {
            Orientation::Auto => "Auto",
            Orientation::Local => "Local",
            Orientation::Global => "Global",
        }
    }

    /// What the choice does, for its button's tooltip.
    pub fn tip(self) -> &'static str {
        match self {
            Orientation::Auto => {
                "The arrows and rings follow the object's own axes, or the world's when several objects are selected."
            }
            Orientation::Local => "The arrows and rings always follow the selected object's own axes.",
            Orientation::Global => "The arrows and rings always follow the world's axes.",
        }
    }
}

/// A flat piece of the gizmo, in window pixels.
#[derive(Clone, Debug, PartialEq)]
pub enum Shape {
    Line { points: Vec<Vec2>, width: f32, color: [u8; 4] },
    Fill { points: Vec<Vec2>, color: [u8; 4] },
}

/// A place, turn and size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Placement {
    pub fn of(object: &Object) -> Placement {
        Placement { position: object.position, rotation: object.rotation, scale: object.scale }
    }

    pub fn apply(&self, object: &mut Object) {
        object.position = self.position;
        object.rotation = self.rotation;
        object.scale = self.scale;
    }
}

/// One thing a drag moves. The gizmo is told where it is and what it can do,
/// never what it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Target {
    pub placement: Placement,
    /// Roughly how big it is, in world units: the arms are sized to clear it.
    pub radius: f32,
    /// Whether it has a turn of its own. A point has none, though several
    /// points still swing about their shared centre.
    pub turns: bool,
    /// Whether it has a size of its own.
    pub scales: bool,
}

impl Target {
    /// A scene object, which can be moved, turned and scaled.
    pub fn object(object: &Object, radius: f32) -> Target {
        Target { placement: Placement::of(object), radius, turns: true, scales: true }
    }
}

/// Which kinds of handle to offer. What the targets cannot do is left out
/// whatever is asked for: a handle that visibly does nothing reads as broken.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Handles {
    pub moves: bool,
    pub turns: bool,
    pub scales: bool,
}

impl Handles {
    pub const ALL: Handles = Handles { moves: true, turns: true, scales: true };
}

/// The handles that are out, one bit per axis. Worked out once when a frame
/// is made and kept through a drag: the handle under the pointer must not
/// vanish because the view-angle cull changed its mind mid-gesture.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Shown {
    arrows: u8,
    rings: u8,
    cubes: u8,
    /// Cubes standing on a thin stem of their own, because the arrows point
    /// another way (or theirs is put away).
    stems: u8,
    centre: bool,
}

impl Shown {
    fn bit(set: u8, i: usize) -> bool {
        set >> i & 1 == 1
    }

    pub fn has(self, handle: Handle) -> bool {
        match handle {
            Handle::Move(i) => Self::bit(self.arrows, i),
            Handle::Rotate(i) => Self::bit(self.rings, i),
            Handle::Scale(i) => Self::bit(self.cubes, i),
            Handle::Uniform => self.centre,
        }
    }

    fn stem(self, i: usize) -> bool {
        Self::bit(self.stems, i)
    }
}

/// The point a set of targets is moved, turned and scaled about: a lone
/// target's own position, the middle of several. `None` for no targets.
pub fn pivot_of(targets: &[Target]) -> Option<Vec3> {
    match targets {
        [] => None,
        [only] => Some(only.placement.position),
        _ => Some(targets.iter().map(|t| t.placement.position).sum::<Vec3>() / targets.len() as f32),
    }
}

/// The axes the arrows and rings follow. `Local` borrows the first target's,
/// so callers list the active object first.
pub fn rotation_of(targets: &[Target], orientation: Orientation) -> Quat {
    match orientation {
        Orientation::Local => targets.first().map_or(Quat::IDENTITY, |t| t.placement.rotation),
        Orientation::Global => Quat::IDENTITY,
        Orientation::Auto => scale_rotation_of(targets),
    }
}

/// The axes the scale cubes follow, whatever the orientation: a lone target's
/// own, and the world's for a set, which has no frame of its own.
pub fn scale_rotation_of(targets: &[Target]) -> Quat {
    match targets {
        [only] => only.placement.rotation,
        _ => Quat::IDENTITY,
    }
}

/// How far the targets reach from the pivot. For a set the arms have to
/// clear the whole spread, not only its biggest member, or two objects far
/// apart get a gizmo buried inside one of them.
pub fn reach_of(targets: &[Target], pivot: Vec3) -> f32 {
    targets.iter().map(|t| t.placement.position.distance(pivot) + t.radius).fold(0.0, f32::max)
}

/// Where the gizmo sits, which way its axes point, how long its arms are and
/// which handles are out.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub pivot: Vec3,
    /// The arrows and rings.
    pub rotation: Quat,
    /// The scale cubes.
    pub scale_rotation: Quat,
    pub arm: f32,
    pub shown: Shown,
    /// World distance from the pivot inside which no arrow is drawn or
    /// grabbed (see `keeping_clear`).
    clear: f32,
}

impl Frame {
    /// The gizmo for one object whose bounding radius is `radius`: on the
    /// object's own origin and axes, every handle offered. `None` if the
    /// object is behind the camera.
    #[cfg(test)]
    pub fn of(object: &Object, radius: f32, view: &View, ppp: f32) -> Option<Frame> {
        Frame::on(&[Target::object(object, radius)], Orientation::Auto, Handles::ALL, view, ppp)
    }

    /// The gizmo for a set of targets: at their pivot, sized to reach past
    /// them but kept a usable size on screen. `None` with no targets, or
    /// with the pivot behind the camera.
    pub fn on(
        targets: &[Target],
        orientation: Orientation,
        handles: Handles,
        view: &View,
        ppp: f32,
    ) -> Option<Frame> {
        let pivot = pivot_of(targets)?;
        let depth = view.depth_of(pivot);
        if view.ortho_half_height.is_none() && depth <= 1e-5 {
            return None;
        }
        let per_px = view.world_per_pixel(depth);
        let min = MIN_ARM_PX * ppp * per_px;
        let max = (MAX_ARM_SCREEN * view.size.y * per_px).max(min);
        let mut frame = Frame {
            pivot,
            rotation: rotation_of(targets, orientation),
            scale_rotation: scale_rotation_of(targets),
            arm: (reach_of(targets, pivot) * ARM_PER_RADIUS).clamp(min, max),
            shown: Shown::default(),
            clear: 0.0,
        };
        frame.shown = frame.cull(targets, handles, view);
        Some(frame)
    }

    /// Leaves the middle of the gizmo to what it sits on: within `radius` of
    /// the pivot (and never less than a fingertip on screen) no arrow is
    /// drawn or grabbed, so a tool's own point or body can still be pressed
    /// and dragged freely. Without this every grab of the target would land
    /// on an arrow shaft, which starts at its very centre.
    pub fn keeping_clear(mut self, radius: f32, view: &View, ppp: f32) -> Frame {
        let fingertip = CLEAR_PX * ppp * view.world_per_pixel(view.depth_of(self.pivot));
        self.clear = radius.max(fingertip).min(CLEAR_MAX * self.arm);
        self
    }

    /// Direction `i` of the arrows and rings.
    pub fn axis(&self, i: usize) -> Vec3 {
        self.rotation * Vec3::AXES[i]
    }

    /// Direction `i` of the scale cubes.
    pub fn scale_axis(&self, i: usize) -> Vec3 {
        self.scale_rotation * Vec3::AXES[i]
    }

    fn point(&self, i: usize, along: f32) -> Vec3 {
        self.pivot + self.axis(i) * (along * self.arm)
    }

    fn scale_point(&self, i: usize, along: f32) -> Vec3 {
        self.pivot + self.scale_axis(i) * (along * self.arm)
    }

    /// Direction from the eye to the pivot.
    fn view_dir(&self, view: &View) -> Vec3 {
        if view.ortho_half_height.is_some() {
            view.forward
        } else {
            (self.pivot - view.pos).normalize_or(view.forward)
        }
    }

    /// Which handles are out for these targets, seen from here.
    fn cull(&self, targets: &[Target], handles: Handles, view: &View) -> Shown {
        let view_dir = self.view_dir(view);
        let across = |axis: Vec3| axis.dot(view_dir).abs() < AXIS_CULL;
        let scales = handles.scales && targets.iter().any(|t| t.scales);
        // A lone point has nothing to turn. A set of them has: turning
        // swings them round the shared pivot.
        let turns = handles.turns && (targets.len() > 1 || targets.iter().any(|t| t.turns));
        let agree = self.rotation.angle_between(self.scale_rotation) < FRAMES_AGREE;
        let mut shown = Shown { centre: scales, ..Shown::default() };
        for i in 0..3 {
            let arrow = handles.moves && across(self.axis(i));
            let cube = scales && across(self.scale_axis(i));
            shown.arrows |= (arrow as u8) << i;
            shown.rings |= (turns as u8) << i;
            shown.cubes |= (cube as u8) << i;
            // A cube rides its arrow's shaft only while there is one along
            // its own axis; otherwise it would float unattached.
            shown.stems |= ((cube && !(agree && arrow)) as u8) << i;
        }
        shown
    }

    /// Pixels per world unit at the pivot.
    fn px_per_unit(&self, view: &View) -> f32 {
        1.0 / view.world_per_pixel(view.depth_of(self.pivot)).max(1e-12)
    }

    /// Two unit vectors spanning the plane of ring `i`; turning from the
    /// first to the second is a positive turn about the axis.
    fn ring_basis(&self, i: usize) -> (Vec3, Vec3) {
        (self.axis((i + 1) % 3), self.axis((i + 2) % 3))
    }

    fn ring_point(&self, i: usize, angle: f32) -> Vec3 {
        let (u, v) = self.ring_basis(i);
        self.pivot + (u * angle.cos() + v * angle.sin()) * (RING * self.arm)
    }

    /// The handle under a pixel, if any.
    pub fn pick(&self, view: &View, px: Vec2, ppp: f32) -> Option<Handle> {
        let grab = GRAB_PX * ppp;
        let scale = self.px_per_unit(view);
        let centre = view.project(self.pivot)?;
        let inside = |c: Vec2, half: f32| (px - c).abs().max_element() <= half;

        if self.shown.centre && inside(centre, (CENTRE_HALF * self.arm * scale).max(6.0 * ppp) + grab * 0.3) {
            return Some(Handle::Uniform);
        }
        // The middle that is kept clear is a circle on screen as well as a
        // stretch of each shaft: an arrow pointing at the eye is drawn right
        // across it, and a ring seen edge-on runs through it.
        if self.clear > 0.0 && px.distance(centre) <= self.clear * scale {
            return None;
        }
        let cube_half = (SCALE_CUBE_HALF * self.arm * scale).max(5.0 * ppp) + grab * 0.5;
        // Where the shafts and stems begin: the pivot, or the edge of the
        // middle that is kept clear.
        let from = self.clear / self.arm.max(1e-12);
        let mut best: Option<(Handle, f32)> = None;
        let mut offer = |handle: Handle, distance: f32| {
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some((handle, distance));
            }
        };
        for i in 0..3 {
            if self.shown.has(Handle::Scale(i)) {
                if let Some(c) = view.project(self.scale_point(i, SCALE_CUBE)) {
                    if inside(c, cube_half) {
                        offer(Handle::Scale(i), -1.0);
                    }
                    // Nothing else lives on a stem's line, so it grabs too.
                    if let Some(root) = view.project(self.scale_point(i, from)).filter(|_| self.shown.stem(i)) {
                        let d = segment_distance(px, root, c);
                        if d <= grab {
                            offer(Handle::Scale(i), d);
                        }
                    }
                }
            }
            if self.shown.has(Handle::Move(i)) {
                if let (Some(root), Some(tip)) =
                    (view.project(self.point(i, from)), view.project(self.point(i, ARROW_TIP)))
                {
                    let d = segment_distance(px, root, tip);
                    if d <= grab {
                        offer(Handle::Move(i), d);
                    }
                }
            }
            if self.shown.has(Handle::Rotate(i)) {
                let ring: Vec<Vec2> = self.ring(i, view).into_iter().flatten().collect();
                let d = ring.windows(2).map(|w| segment_distance(px, w[0], w[1])).fold(f32::INFINITY, f32::min);
                if d <= grab {
                    offer(Handle::Rotate(i), d);
                }
            }
        }
        best.map(|(handle, _)| handle)
    }

    /// Ring `i` as a closed loop of pixels (`None` where it is behind the eye).
    fn ring(&self, i: usize, view: &View) -> Vec<Option<Vec2>> {
        (0..=RING_SEGMENTS)
            .map(|k| view.project(self.ring_point(i, k as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU)))
            .collect()
    }

    /// The gizmo as flat shapes, back to front. `hot` is the handle under the
    /// pointer or being dragged, drawn brighter.
    pub fn shapes(&self, view: &View, hot: Option<Handle>, ppp: f32) -> Vec<Shape> {
        let mut out = Vec::new();
        let Some(centre) = view.project(self.pivot) else { return out };
        let scale = self.px_per_unit(view);
        let view_dir = self.view_dir(view);
        let color = |i: usize, handle: Handle, alpha: u8| {
            let [r, g, b] = AXIS_COLORS[i];
            let lift = |c: u8| if hot == Some(handle) { c + (255 - c) / 2 } else { c };
            [lift(r), lift(g), lift(b), alpha]
        };

        // Rings first, so the arms read as standing in front of them. The
        // half of each ring behind the pivot is drawn faint.
        for i in (0..3).filter(|&i| self.shown.has(Handle::Rotate(i))) {
            let handle = Handle::Rotate(i);
            let width = if hot == Some(handle) { 3.5 } else { 2.5 } * ppp;
            let ring = self.ring(i, view);
            let near = |k: usize| {
                let a = k as f32 / RING_SEGMENTS as f32 * std::f32::consts::TAU;
                (self.ring_point(i, a) - self.pivot).dot(view_dir) <= 0.0
            };
            let mut run: Vec<Vec2> = Vec::new();
            let mut run_near = true;
            let mut flush = |run: &mut Vec<Vec2>, is_near: bool| {
                if run.len() > 1 {
                    let alpha = if is_near { 255 } else { 70 };
                    out.push(Shape::Line { points: std::mem::take(run), width, color: color(i, handle, alpha) });
                }
                run.clear();
            };
            for k in 0..RING_SEGMENTS {
                let (Some(a), Some(b)) = (ring[k], ring[k + 1]) else {
                    flush(&mut run, run_near);
                    continue;
                };
                let is_near = near(k) && near(k + 1);
                if is_near != run_near {
                    flush(&mut run, run_near);
                    run_near = is_near;
                }
                if run.is_empty() {
                    run.push(a);
                }
                run.push(b);
            }
            flush(&mut run, run_near);
        }

        // Arms: far ones first. A cube on a stem of its own is an arm of its
        // own; one riding an arrow is drawn with that arrow, over its shaft.
        let from = self.clear / self.arm.max(1e-12);
        let mut arms: Vec<(f32, Handle)> = Vec::new();
        for i in 0..3 {
            if self.shown.has(Handle::Move(i)) {
                arms.push((self.axis(i).dot(view_dir), Handle::Move(i)));
            }
            if self.shown.has(Handle::Scale(i)) && self.shown.stem(i) {
                arms.push((self.scale_axis(i).dot(view_dir) * (SCALE_CUBE / ARROW_TIP), Handle::Scale(i)));
            }
        }
        arms.sort_by(|a, b| b.0.total_cmp(&a.0));
        let cube = |out: &mut Vec<Shape>, i: usize| {
            let Some(c) = view.project(self.scale_point(i, SCALE_CUBE)) else { return };
            let scaling = Handle::Scale(i);
            let grow = if hot == Some(scaling) { 1.25 } else { 1.0 };
            let half = (SCALE_CUBE_HALF * self.arm * scale).max(5.0 * ppp) * grow;
            out.push(Shape::Fill { points: square(c, half + 1.5 * ppp), color: [15, 15, 20, 255] });
            out.push(Shape::Fill { points: square(c, half), color: color(i, scaling, 255) });
        };
        for (_, arm) in arms {
            match arm {
                Handle::Move(i) => {
                    let (Some(root), Some(base), Some(tip)) = (
                        view.project(self.point(i, from)),
                        view.project(self.point(i, ARROW_BASE)),
                        view.project(self.point(i, ARROW_TIP)),
                    ) else {
                        continue;
                    };
                    let moving = Handle::Move(i);
                    let width = if hot == Some(moving) { 4.0 } else { 3.0 } * ppp;
                    out.push(Shape::Line { points: vec![root, base], width, color: color(i, moving, 255) });
                    // The arrowhead: a triangle, plus its round base where the
                    // arrow is turned far enough toward the eye for that to show.
                    let along = (tip - base).normalize_or_zero();
                    let side = Vec2::new(-along.y, along.x) * (ARROW_RADIUS * self.arm * scale).max(4.0 * ppp);
                    let facing = self.axis(i).dot(view_dir).abs();
                    let depth = along * (side.length() * facing);
                    let rim = (0..16).map(|k| {
                        let a = k as f32 / 16.0 * std::f32::consts::TAU;
                        base + side * a.cos() + depth * a.sin()
                    });
                    out.push(Shape::Fill { points: rim.collect(), color: color(i, moving, 255) });
                    out.push(Shape::Fill { points: vec![tip, base + side, base - side], color: color(i, moving, 255) });
                    if self.shown.has(Handle::Scale(i)) && !self.shown.stem(i) {
                        cube(&mut out, i);
                    }
                }
                Handle::Scale(i) => {
                    let scaling = Handle::Scale(i);
                    if let (Some(root), Some(c)) =
                        (view.project(self.scale_point(i, from)), view.project(self.scale_point(i, SCALE_CUBE)))
                    {
                        // Thinner than an arrow's shaft, so the two never
                        // read as the same handle.
                        let width = if hot == Some(scaling) { 2.5 } else { 1.5 } * ppp;
                        out.push(Shape::Line { points: vec![root, c], width, color: color(i, scaling, 255) });
                    }
                    cube(&mut out, i);
                }
                _ => {}
            }
        }

        if self.shown.centre {
            let grow = if hot == Some(Handle::Uniform) { 1.25 } else { 1.0 };
            let half = (CENTRE_HALF * self.arm * scale).max(6.0 * ppp) * grow;
            out.push(Shape::Fill { points: square(centre, half + 1.5 * ppp), color: [15, 15, 20, 255] });
            out.push(Shape::Fill { points: square(centre, half), color: [255, 255, 255, 255] });
        }
        out
    }
}

fn square(c: Vec2, half: f32) -> Vec<Vec2> {
    vec![c + Vec2::new(-half, -half), c + Vec2::new(half, -half), c + Vec2::new(half, half), c + Vec2::new(-half, half)]
}

fn segment_distance(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_squared().max(1e-12)).clamp(0.0, 1.0);
    p.distance(a + ab * t)
}

fn ray_plane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = dir.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denom;
    (t > 0.0).then(|| origin + dir * t)
}

/// How a rotate drag reads the pointer.
#[derive(Clone, Copy, Debug)]
enum Turn {
    /// The pointer's angle around the pivot, on the ring's own plane.
    Plane { last: f32 },
    /// Ring seen edge-on: travel along this screen direction, in radians per
    /// pixel.
    Slide { along: Vec2, per_px: f32, from: Vec2 },
}

/// One handle drag, press to release. Everything is measured against the
/// state at the press rather than added to the last frame's result, so a long
/// drag does not drift and dragging back to the start returns to the start.
/// The set being dragged is kept here too, so nothing that changes the
/// selection mid-drag can pull the drag onto something else.
#[derive(Clone, Debug)]
pub struct Drag {
    pub handle: Handle,
    /// The gizmo as it was at the press, handles and all. Only a move
    /// carries it along (see `shown_frame`).
    pub frame: Frame,
    /// Each target as it was at the press.
    start: Vec<Target>,
    /// Where the drag has put each of them.
    now: Vec<Placement>,
    axis: Vec3,
    plane_normal: Vec3,
    /// Where along the axis the press was (move, scale).
    start_value: f32,
    turn: Turn,
    /// How far the drag has gone: distance along the axis for a move, the
    /// angle in radians for a turn.
    travel: f32,
    /// How many times bigger, for a scale.
    factor: f32,
    last_px: Vec2,
}

impl Drag {
    /// A drag of one object's gizmo.
    #[cfg(test)]
    pub fn begin(handle: Handle, frame: Frame, object: &Object, view: &View, px: Vec2) -> Drag {
        Drag::on(handle, frame, &[Target::object(object, 0.0)], view, px)
    }

    /// A drag of `handle` on the gizmo `frame` made for `targets`, pressed
    /// at `px`.
    pub fn on(handle: Handle, frame: Frame, targets: &[Target], view: &View, px: Vec2) -> Drag {
        let axis = match handle {
            Handle::Move(i) | Handle::Rotate(i) => frame.axis(i),
            // A cube pulls along the axis it is drawn on, which is not the
            // arrows' when the two sets of axes have parted.
            Handle::Scale(i) => frame.scale_axis(i),
            Handle::Uniform => Vec3::Y,
        };
        // A plane holding the axis that faces the camera as squarely as it
        // can, so the handle tracks the pointer.
        let mut plane_normal = axis.cross(view.forward).cross(axis);
        if plane_normal.length_squared() < 1e-6 {
            plane_normal = axis.cross(view.up);
        }
        let mut drag = Drag {
            handle,
            frame,
            start: targets.to_vec(),
            now: targets.iter().map(|t| t.placement).collect(),
            axis,
            plane_normal: plane_normal.normalize_or(view.forward),
            start_value: 0.0,
            turn: Turn::Plane { last: 0.0 },
            travel: 0.0,
            factor: 1.0,
            last_px: px,
        };
        match handle {
            Handle::Move(_) | Handle::Scale(_) => drag.start_value = drag.along_axis(view, px).unwrap_or(0.0),
            Handle::Rotate(_) => {
                let view_dir = frame.view_dir(view);
                drag.turn = match drag.ring_angle(view, px) {
                    Some(last) if axis.dot(view_dir).abs() >= RING_EDGE_ON => Turn::Plane { last },
                    _ => {
                        // The part of the ring you can see is the near side;
                        // dragging it sideways carries it round.
                        let toward = -(view_dir - axis * view_dir.dot(axis)).normalize_or(view.up);
                        let radius = RING * frame.arm;
                        let near = frame.pivot + toward * radius;
                        let tangent = axis.cross(toward);
                        let step = radius * 0.05;
                        let along = match (view.project(near), view.project(near + tangent * step)) {
                            (Some(a), Some(b)) => (b - a).normalize_or(Vec2::X),
                            _ => Vec2::X,
                        };
                        Turn::Slide { along, per_px: 1.0 / (radius * frame.px_per_unit(view)).max(1.0), from: px }
                    }
                };
            }
            Handle::Uniform => {}
        }
        drag
    }

    fn along_axis(&self, view: &View, px: Vec2) -> Option<f32> {
        let (origin, dir) = view.ray(px);
        ray_plane(origin, dir, self.frame.pivot, self.plane_normal).map(|hit| (hit - self.frame.pivot).dot(self.axis))
    }

    fn ring_angle(&self, view: &View, px: Vec2) -> Option<f32> {
        let Handle::Rotate(i) = self.handle else { return None };
        let (origin, dir) = view.ray(px);
        let hit = ray_plane(origin, dir, self.frame.pivot, self.axis)?;
        let (u, v) = self.frame.ring_basis(i);
        let offset = hit - self.frame.pivot;
        Some(offset.dot(v).atan2(offset.dot(u)))
    }

    /// Moves the drag to the pointer at `px`. Returns the first target's
    /// placement; `placements` has them all.
    pub fn update(&mut self, view: &View, px: Vec2) -> Placement {
        match self.handle {
            Handle::Move(_) => {
                if let Some(value) = self.along_axis(view, px) {
                    self.travel = value - self.start_value;
                }
            }
            Handle::Rotate(_) => {
                let angle = self.ring_angle(view, px);
                match (&mut self.turn, angle) {
                    (Turn::Plane { last }, Some(angle)) => {
                        // Summed in small signed steps, so a drag can go
                        // round more than half a turn.
                        use std::f32::consts::{PI, TAU};
                        self.travel += (angle - *last + PI).rem_euclid(TAU) - PI;
                        *last = angle;
                    }
                    (Turn::Slide { along, per_px, from }, _) => self.travel = (px - *from).dot(*along) * *per_px,
                    _ => {}
                }
            }
            Handle::Scale(_) => {
                if let Some(value) = self.along_axis(view, px) {
                    let floor = 0.01 * self.frame.arm;
                    let start = if self.start_value.abs() < floor { floor } else { self.start_value };
                    self.factor = (value / start).max(0.05);
                }
            }
            Handle::Uniform => {
                // Nothing to measure against at the pivot itself: dragging up
                // grows, down shrinks.
                let step = (1.0 - (px.y - self.last_px.y) * UNIFORM_PER_PX).max(0.01);
                self.factor = (self.factor * step).max(0.01);
            }
        }
        self.last_px = px;
        self.place();
        self.now[0]
    }

    /// Works every target's placement out again from where it was at the
    /// press. A lone target sits on the pivot, so the terms that carry a set
    /// round or away from the pivot are exactly zero for it.
    fn place(&mut self) {
        let pivot = self.frame.pivot;
        for (now, start) in self.now.iter_mut().zip(&self.start) {
            let from = start.placement;
            *now = from;
            match self.handle {
                Handle::Move(_) => now.position = from.position + self.axis * self.travel,
                Handle::Rotate(_) => {
                    let spin = Quat::from_axis_angle(self.axis, self.travel);
                    now.position = pivot + spin * (from.position - pivot);
                    if start.turns {
                        now.rotation = (spin * from.rotation).normalize();
                    }
                }
                Handle::Scale(i) if start.scales => {
                    // The set spreads along the pulled axis as each member
                    // grows, so several objects scale as one body.
                    let along = (from.position - pivot).dot(self.axis);
                    now.position = from.position + self.axis * (along * (self.factor - 1.0));
                    now.scale[i] = (from.scale[i] * self.factor).max(MIN_SCALE);
                }
                Handle::Uniform if start.scales => {
                    now.position = pivot + (from.position - pivot) * self.factor;
                    now.scale = (from.scale * self.factor).max(Vec3::splat(MIN_SCALE));
                }
                Handle::Scale(_) | Handle::Uniform => {}
            }
        }
    }

    /// Where the drag has put each target, in the order they were given.
    pub fn placements(&self) -> &[Placement] {
        &self.now
    }

    /// Whether any target is somewhere other than where it was pressed. A
    /// click that picked a handle without moving it is not an edit.
    #[cfg(test)]
    pub fn changed(&self) -> bool {
        self.now.iter().zip(&self.start).any(|(now, start)| *now != start.placement)
    }

    /// The whole drag so far as one change to the world: a point `p` of what
    /// is dragged belongs at `world_delta() * p`. This is what reshapes a
    /// surface point by point instead of placing objects. In doubles, so a
    /// drag brought back to its start comes out as no change at all.
    pub fn world_delta(&self) -> DMat4 {
        let pivot = self.frame.pivot.as_dvec3();
        let axis = self.axis.as_dvec3();
        let about = |m: DMat4| DMat4::from_translation(pivot) * m * DMat4::from_translation(-pivot);
        match self.handle {
            Handle::Move(_) => DMat4::from_translation(axis * self.travel as f64),
            Handle::Rotate(_) => about(DMat4::from_axis_angle(axis, self.travel as f64)),
            Handle::Scale(_) => {
                // Stretch along one line: only the part of a point's offset
                // that lies along the axis grows.
                let along = DMat3::from_cols(axis * axis.x, axis * axis.y, axis * axis.z);
                about(DMat4::from_mat3(DMat3::IDENTITY + along * (self.factor as f64 - 1.0)))
            }
            Handle::Uniform => about(DMat4::from_scale(DVec3::splat(self.factor as f64))),
        }
    }

    /// The gizmo to draw mid-drag: as it was pressed, with the handles it had
    /// then, except that a move carries it along with what it moves. A ring
    /// that re-centred on its own output would slide out from under the
    /// pointer, and a scale that did would chase its own spread.
    pub fn shown_frame(&self) -> Frame {
        match self.handle {
            Handle::Move(_) => Frame { pivot: self.frame.pivot + self.axis * self.travel, ..self.frame },
            _ => self.frame,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::FRAC_PI_2;

    use super::*;
    use crate::camera::Camera;

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    /// A camera ten units from the origin. Pitched, it looks down on the
    /// scene, so a ring about Y is never edge-on to it.
    fn view(yaw: f32, pitch: f32) -> View {
        let mut camera = Camera::default();
        (camera.yaw, camera.pitch, camera.distance) = (yaw, pitch, 10.0);
        camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
    }

    fn body(position: Vec3, rotation: Quat, scale: Vec3) -> Target {
        Target { placement: Placement { position, rotation, scale }, radius: 0.5, turns: true, scales: true }
    }

    fn at(position: Vec3) -> Target {
        body(position, Quat::IDENTITY, Vec3::ONE)
    }

    fn point(position: Vec3) -> Target {
        Target { radius: 0.0, turns: false, scales: false, ..at(position) }
    }

    fn near(a: Vec3, b: Vec3) -> bool {
        a.distance(b) < 1e-3
    }

    #[test]
    fn the_axes_follow_the_orientation_setting_on_a_turned_object() {
        let turned = Quat::from_rotation_y(30f32.to_radians());
        let a = body(Vec3::new(0.0, 2.0, 3.0), turned, Vec3::ONE);
        let b = at(Vec3::new(4.0, 2.0, 3.0));
        let (one, two) = ([a], [a, b]);

        assert!(rotation_of(&one, Orientation::Auto).angle_between(turned) < 2e-3);
        assert!(rotation_of(&two, Orientation::Auto).angle_between(Quat::IDENTITY) < 2e-3);
        assert!(rotation_of(&two, Orientation::Local).angle_between(turned) < 2e-3, "Local borrows the first target's");
        assert!(rotation_of(&one, Orientation::Global).angle_between(Quat::IDENTITY) < 2e-3);
        assert!(scale_rotation_of(&one).angle_between(turned) < 2e-3, "scale always uses the object's own axes");
        assert!(scale_rotation_of(&two).angle_between(Quat::IDENTITY) < 2e-3);

        let pivot = pivot_of(&two).unwrap();
        assert!(near(pivot, Vec3::new(2.0, 2.0, 3.0)));
        assert_eq!(pivot_of(&one), Some(a.placement.position));
        assert_eq!(pivot_of(&[]), None);
        // Each target 2 from the middle plus its 0.5 radius, times the arm factor.
        assert!((reach_of(&two, pivot) * ARM_PER_RADIUS - 4.5).abs() < 1e-4);

        // The same on the frame the gizmo is drawn and dragged in.
        let view = view(25.0, 20.0);
        let frame = |orientation| Frame::on(&one, orientation, Handles::ALL, &view, 1.0).unwrap();
        for i in 0..3 {
            let (own, world) = (turned * Vec3::AXES[i], Vec3::AXES[i]);
            assert!(near(frame(Orientation::Local).axis(i), own));
            assert!(near(frame(Orientation::Auto).axis(i), own));
            assert!(near(frame(Orientation::Global).axis(i), world));
            // In Global the arrows and rings follow the world; the cubes stay.
            assert!(near(frame(Orientation::Global).scale_axis(i), own));
        }
        // Cubes ride the arrows while the axes agree, and stand on stems of
        // their own once they part.
        assert_eq!(frame(Orientation::Local).shown.stems, 0);
        assert_eq!(frame(Orientation::Global).shown.stems, 0b111);
        assert!(Frame::on(&[], Orientation::Auto, Handles::ALL, &view, 1.0).is_none());
    }

    #[test]
    fn in_global_a_turned_object_moves_and_turns_on_world_axes_and_scales_on_its_own() {
        let turned = Quat::from_rotation_y(30f32.to_radians());
        let start = Vec3::new(0.5, 0.2, -0.3);
        let target = [body(start, turned, Vec3::new(1.0, 1.5, 2.0))];
        let view = view(25.0, 20.0);
        let frame = Frame::on(&target, Orientation::Global, Handles::ALL, &view, 1.0).unwrap();
        let px = |p: Vec3| view.project(p).unwrap();

        // The arrow is where the world's X is, and moves along it.
        let grab = start + Vec3::X * frame.arm * 1.1;
        assert_eq!(frame.pick(&view, px(grab), 1.0), Some(Handle::Move(0)));
        let mut drag = Drag::on(Handle::Move(0), frame, &target, &view, px(grab));
        let moved = drag.update(&view, px(grab + Vec3::X * 0.4));
        assert!(near(moved.position, start + Vec3::X * 0.4), "{:?}", moved.position);
        assert_eq!((moved.rotation, moved.scale), (turned, target[0].placement.scale));
        assert!(drag.changed());
        assert_eq!(drag.update(&view, px(grab)).position, start, "back at the press it is back at the start");
        assert!(!drag.changed());

        // A quarter turn of the world's Y ring.
        let (u, v) = (Vec3::Z * frame.arm * RING, Vec3::X * frame.arm * RING);
        let mut drag = Drag::on(Handle::Rotate(1), frame, &target, &view, px(start + u));
        let mut spun = target[0].placement;
        for k in 1..=30 {
            let a = k as f32 / 30.0 * FRAC_PI_2;
            spun = drag.update(&view, px(start + u * a.cos() + v * a.sin()));
        }
        assert!(spun.rotation.angle_between(Quat::from_rotation_y(FRAC_PI_2) * turned) < 0.02);
        assert_eq!(spun.position, start, "a lone target turns in place");

        // The cube is on the object's own X, on a stem, and stretches that axis alone.
        let own = turned * Vec3::X;
        let grab = start + own * frame.arm * SCALE_CUBE;
        assert_eq!(frame.pick(&view, px(grab), 1.0), Some(Handle::Scale(0)));
        assert_eq!(frame.pick(&view, px(start + own * frame.arm * 0.4), 1.0), Some(Handle::Scale(0)), "the stem grabs too");
        let mut drag = Drag::on(Handle::Scale(0), frame, &target, &view, px(grab));
        let scaled = drag.update(&view, px(start + own * frame.arm * SCALE_CUBE * 2.0));
        assert!(near(scaled.scale, Vec3::new(2.0, 1.5, 2.0)), "{:?}", scaled.scale);
        assert_eq!((scaled.position, scaled.rotation), (start, turned));
    }

    #[test]
    fn a_set_moves_together_and_turns_about_its_shared_centre() {
        let targets = [at(Vec3::new(0.0, 2.0, 3.0)), at(Vec3::new(2.0, 2.0, 3.0))];
        let centre = Vec3::new(1.0, 2.0, 3.0);
        let view = view(0.0, 35.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let frame = Frame::on(&targets, Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        assert!(near(frame.pivot, centre), "a set pivots on its middle");
        assert!(near(frame.axis(0), Vec3::X), "and takes the world's axes");

        let mut drag = Drag::on(Handle::Move(0), frame, &targets, &view, px(centre));
        drag.update(&view, px(centre + Vec3::X));
        assert!(near(drag.placements()[0].position, Vec3::new(1.0, 2.0, 3.0)));
        assert!(near(drag.placements()[1].position, Vec3::new(3.0, 2.0, 3.0)));
        // The gizmo goes with a move, keeping the handles it had.
        assert!(near(drag.shown_frame().pivot, centre + Vec3::X));

        // The pointer goes from +X round to +Z, and the set with it.
        let mut drag = Drag::on(Handle::Rotate(1), frame, &targets, &view, px(centre + Vec3::X));
        drag.update(&view, px(centre + Vec3::Z));
        let [a, b] = [drag.placements()[0], drag.placements()[1]];
        assert!(near(a.position, centre - Vec3::Z), "orbits the shared pivot: {:?}", a.position);
        assert!(near(b.position, centre + Vec3::Z));
        assert!(near(a.rotation * Vec3::X, Vec3::Z), "and each turns with it");
        assert_eq!(drag.shown_frame(), frame, "a turn leaves the gizmo where it was pressed");
    }

    #[test]
    fn a_set_spreads_apart_as_it_is_scaled() {
        let targets = [at(Vec3::new(0.0, 2.0, 3.0)), at(Vec3::new(2.0, 2.0, 3.0))];
        let centre = Vec3::new(1.0, 2.0, 3.0);
        let view = view(0.0, 35.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let frame = Frame::on(&targets, Orientation::Local, Handles::ALL, &view, 1.0).unwrap();

        let grab = centre + Vec3::X * frame.arm * SCALE_CUBE;
        let mut drag = Drag::on(Handle::Scale(0), frame, &targets, &view, px(grab));
        drag.update(&view, px(centre + Vec3::X * frame.arm * SCALE_CUBE * 2.0));
        let [a, b] = [drag.placements()[0], drag.placements()[1]];
        assert!(near(a.position, Vec3::new(-1.0, 2.0, 3.0)), "{:?}", a.position);
        assert!(near(b.position, Vec3::new(3.0, 2.0, 3.0)));
        assert!(near(a.scale, Vec3::new(2.0, 1.0, 1.0)) && near(b.scale, Vec3::new(2.0, 1.0, 1.0)));

        // The centre cube: 250 pixels up doubles, half of that back down halves.
        let middle = px(centre);
        let mut drag = Drag::on(Handle::Uniform, frame, &targets, &view, middle);
        drag.update(&view, middle - Vec2::new(0.0, 250.0));
        let [a, b] = [drag.placements()[0], drag.placements()[1]];
        assert!(near(a.position, centre - Vec3::X * 2.0) && near(b.position, centre + Vec3::X * 2.0));
        assert!(near(a.scale, Vec3::splat(2.0)));
        drag.update(&view, middle - Vec2::new(0.0, 125.0));
        assert!(near(drag.placements()[0].position, targets[0].placement.position));
        assert!(near(drag.placements()[1].scale, Vec3::ONE));
    }

    #[test]
    fn a_drag_keeps_the_handles_it_began_with() {
        // Seen from straight in front, the Z arrow points into the screen
        // and is put away.
        let view = view(0.0, 0.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let target = [at(Vec3::ZERO)];
        let frame = Frame::on(&target, Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        assert!(frame.shown.has(Handle::Move(0)) && frame.shown.has(Handle::Move(1)));
        assert!(!frame.shown.has(Handle::Move(2)) && !frame.shown.has(Handle::Scale(2)));
        assert!(frame.shown.has(Handle::Rotate(2)), "rings are never put away");

        // Carried well off to the side, the gizmo is seen at an angle where
        // a new one would have its Z arrow out. This one does not grow it.
        let grab = Vec3::X * frame.arm;
        let mut drag = Drag::on(Handle::Move(0), frame, &target, &view, px(grab));
        let there = drag.update(&view, px(grab + Vec3::X * 6.0));
        let fresh = Frame::on(&[at(there.position)], Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        assert!(fresh.shown.has(Handle::Move(2)));
        let shown = drag.shown_frame();
        assert!(near(shown.pivot, there.position));
        assert_eq!(shown.shown, frame.shown);
        let count = |f: &Frame| f.shapes(&view, None, 1.0).len();
        assert!(count(&fresh) > count(&shown));
    }

    #[test]
    fn a_point_gets_arrows_only_and_keeps_its_middle_to_itself() {
        let view = view(25.0, 20.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let spot = Vec3::new(0.3, 0.1, 0.2);
        let arrows = Handles { moves: true, turns: false, scales: false };
        let frame = Frame::on(&[point(spot)], Orientation::Auto, arrows, &view, 1.0).unwrap().keeping_clear(0.0, &view, 1.0);
        assert_eq!((frame.shown.rings, frame.shown.cubes, frame.shown.centre), (0, 0, false));
        assert_eq!(frame.pick(&view, px(spot), 1.0), None, "the middle is the point's own");
        assert_eq!(frame.pick(&view, px(spot + Vec3::X * frame.arm * 0.05), 1.0), None);
        for i in 0..3 {
            assert_eq!(frame.pick(&view, px(spot + Vec3::AXES[i] * frame.arm * 0.7), 1.0), Some(Handle::Move(i)));
        }
        // A point of no size still gets arms long enough to grab.
        assert!(frame.arm * frame.px_per_unit(&view) >= MIN_ARM_PX - 0.01);

        // Asked for everything, a lone point still has nothing to turn or
        // scale; two points swing about their middle, so they get rings.
        let alone = Frame::on(&[point(spot)], Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        assert_eq!((alone.shown.rings, alone.shown.cubes, alone.shown.centre), (0, 0, false));
        let pair = [point(spot), point(spot + Vec3::X)];
        let both = Frame::on(&pair, Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        assert_eq!((both.shown.rings, both.shown.cubes, both.shown.centre), (0b111, 0, false));
        // Turned a quarter, the points change places round the middle and
        // keep their own (lack of) turn.
        let middle = spot + Vec3::X * 0.5;
        let reach = Vec3::Z * both.arm * RING;
        let mut drag = Drag::on(Handle::Rotate(1), both, &pair, &view, px(middle + reach));
        for k in 1..=30 {
            let a = k as f32 / 30.0 * FRAC_PI_2;
            drag.update(&view, px(middle + reach * a.cos() + Vec3::X * both.arm * RING * a.sin()));
        }
        assert!(drag.placements()[0].position.distance(middle + Vec3::Z * 0.5) < 0.02);
        assert_eq!(drag.placements()[0].rotation, Quat::IDENTITY);
    }

    #[test]
    fn the_world_change_of_a_drag_matches_where_it_puts_an_object() {
        let turned = Quat::from_rotation_z(0.4) * Quat::from_rotation_y(0.5);
        let start = Placement { position: Vec3::new(0.3, -0.2, 0.4), rotation: turned, scale: Vec3::new(1.0, 1.4, 0.7) };
        let target = [Target { placement: start, radius: 0.5, turns: true, scales: true }];
        let view = view(25.0, 20.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let matrix = |p: Placement| glam::Mat4::from_scale_rotation_translation(p.scale, p.rotation, p.position);
        let corner = Vec3::new(0.5, -0.5, 0.25);
        for orientation in Orientation::ALL {
            let frame = Frame::on(&target, orientation, Handles::ALL, &view, 1.0).unwrap();
            let grabs = [
                (Handle::Move(1), frame.axis(1) * frame.arm, frame.axis(1) * (frame.arm + 0.3)),
                (Handle::Rotate(2), frame.axis(0) * frame.arm * RING, (frame.axis(0) + frame.axis(1) * 0.4) * frame.arm * RING),
                (Handle::Scale(0), frame.scale_axis(0) * frame.arm * SCALE_CUBE, frame.scale_axis(0) * frame.arm * 1.1),
                (Handle::Uniform, Vec3::ZERO, view.up * 0.5),
            ];
            for (handle, from, to) in grabs {
                let mut drag = Drag::on(handle, frame, &target, &view, px(start.position + from));
                let placed = drag.update(&view, px(start.position + to));
                assert!(drag.changed(), "{handle:?} went nowhere");
                // A point of the object lands where the placement puts it.
                let by_delta = drag.world_delta().as_mat4().transform_point3(matrix(start).transform_point3(corner));
                let by_placement = matrix(placed).transform_point3(corner);
                assert!(by_delta.distance(by_placement) < 1e-4, "{orientation:?} {handle:?}: {by_delta} {by_placement}");
                // Back at the press there is no change at all.
                drag.update(&view, px(start.position + from));
                if handle != Handle::Uniform {
                    assert!(drag.world_delta().abs_diff_eq(DMat4::IDENTITY, 1e-5), "{handle:?}");
                }
            }
        }
    }
}
