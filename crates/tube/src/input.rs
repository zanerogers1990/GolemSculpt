//! Drawing, picking and gestures.
//!
//! Two ground rules: a drag is worked out from the press, not from the last
//! move (so nothing creeps and Esc puts it back exactly), and a grabbed
//! handle stays under the pointer.
//!
//! Every step takes the pixel it happens at, so the demo and the tests drive
//! exactly the path the pointer does.

use glam::{Mat3, Vec2, Vec3};
use sculpt_geom::SymmetryOp;
use sculpt_toolkit::{Ctx, Hit, Key, ObjectId, Ray, Scene, View};

use crate::curve;
use crate::live::LiveTube;
use crate::spine::SURFACE_LIFT;
use crate::tool::{COALESCE_SECONDS, TubeTool, pixels_for};
use crate::types::{Kind, TubePoint, TubeSettings};

/// Pick distances and thresholds, in interface points.
const HANDLE_PICK: f32 = 11.0;
const CURVE_PICK: f32 = 9.0;
const DRAG_THRESHOLD: f32 = 3.0;
const STROKE_SPACING: f32 = 3.0;
/// The radius handle is never drawn closer to its point than this, so it
/// can be told from the point however thin the tube is there.
const RING_HANDLE_MIN: f32 = 18.0;
const RESIZE_PER_DOUBLING: f32 = 160.0;

/// A muscle stroke this close to the model's outline, but off it, is pulled onto the model.
const SNAP_REACH: f32 = 40.0;
const SNAP_STEP: f32 = 4.0;
const SNAP_DIRECTIONS: usize = 16;

const DOUBLE_CLICK_SECONDS: f64 = 0.3;
const MAX_STROKE_POINTS: usize = 40;
const WHEEL_SIZE_STEP: f32 = 1.1;

/// A point of a copy of a tube.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HandleHit {
    pub tube: usize,
    pub copy: usize,
    pub point: usize,
}

/// A place on a tube's centre line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct CurveHit {
    pub tube: usize,
    pub copy: usize,
    pub segment: usize,
    pub t: f32,
    pub world: Vec3,
}

/// The radius handle of the selected point, on screen.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RingHandle {
    pub tube: usize,
    pub point: usize,
    pub centre: Vec2,
    pub at: Vec2,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct PointDrag {
    tube: usize,
    copy: usize,
    point: usize,
    /// The point as it was, for Esc.
    start: TubePoint,
    press: Vec2,
    /// From the pointer to the handle at the press, so the handle does not
    /// jump to the pointer.
    grab_offset: Vec2,
    /// A point off every surface slides on the plane facing the view through
    /// here: where the handle was at the press.
    plane_point: Vec3,
    moved: bool,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) enum Drag {
    #[default]
    None,
    /// A press that was the tool's but drags nothing: a tube selected, a
    /// point deleted, the second click of a double click.
    Idle,
    Point(PointDrag),
    /// A new stroke.
    Draw,
    /// S held: sideways travel sizes a point, or the tube's radius.
    Resize { tube: Option<usize>, point: Option<usize>, start_value: f32, start_x: f32 },
    /// The radius handle of a point, dragged away from or toward the point.
    Ring { tube: usize, point: usize, start_size: f32, centre: Vec2, start_distance: f32 },
}

/// The stroke being drawn, as it came off the pointer.
#[derive(Default)]
pub(crate) struct Stroke {
    anchor: Option<ObjectId>,
    on_surface: bool,
    /// Samples off the surface land on the plane facing the view through
    /// here: the surface where the stroke started (so a limb grows from
    /// where it was drawn), or the last surface point of a surface stroke
    /// that ran off the model.
    plane_point: Vec3,
    last_px: Vec2,
    pub world: Vec<Vec3>,
    normal: Vec<Vec3>,
    /// Where each sample was on screen.
    px: Vec<Vec2>,
    /// A muscle stroke: it is pulled onto the model from just off it, and its floating points face
    /// the view it was drawn in.
    muscle: bool,
    toward_viewer: Vec3,
    /// A muscle drawn free: the place nearest the viewer of everything the stroke has passed over. The
    /// muscle is drawn on the plane facing the view just in front of it, so none of it is inside what
    /// it was drawn over.
    over: Option<Vec3>,
}

#[derive(Default)]
pub(crate) struct Pointer {
    pub drag: Drag,
    pub hover_handle: Option<HandleHit>,
    pub hover_curve: Option<CurveHit>,
    pub hover_ring: bool,
    /// The last press on a point, for the double click.
    last_click: Option<(HandleHit, f64)>,
    /// Where the pointer is and the world point under it, for the radius
    /// ring of the next stroke.
    pub cursor: Option<(Vec2, Vec3)>,
    /// The tube the stroke in progress would make.
    pub draft: Option<LiveTube>,
    pub stroke: Stroke,
    /// Where the pointer was last seen, for S pressed with nothing held.
    last_px: Vec2,
}

impl Pointer {
    pub fn clear_hover(&mut self) {
        self.hover_handle = None;
        self.hover_curve = None;
        self.hover_ring = false;
        self.cursor = None;
    }
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> (f32, f32) {
    let ab = b - a;
    let length_squared = ab.length_squared();
    let t = if length_squared > 1e-6 { ((p - a).dot(ab) / length_squared).clamp(0.0, 1.0) } else { 0.0 };
    ((a + ab * t).distance(p), t)
}

/// The middle of the selected object: the depth a stroke started in mid-air
/// is drawn at.
fn default_depth_point(cx: &Ctx) -> Vec3 {
    let scene = cx.scene();
    let bounds = scene.bounds(scene.active());
    if bounds.is_empty() { Vec3::ZERO } else { bounds.center() }
}

/// Where a ray meets a tube's anchor, if the anchor can be seen.
fn raycast_anchor(scene: &dyn Scene, anchor: Option<ObjectId>, ray: &Ray) -> Option<Hit> {
    let anchor = anchor?;
    if !scene.info(anchor)?.visible {
        return None;
    }
    scene.raycast_object(anchor, ray)
}

/// The model's surface nearest a pixel that is just off the model: the place a stroke that started
/// beside the outline, or strayed over it, is pulled back to. Rings of rays outward from the pixel
/// find the model, then each that did is walked back to the outline, and the nearest one wins. The
/// same pixel on the same model always gives the same place.
fn snap_to_model(cx: &Ctx, anchor: Option<ObjectId>, px: Vec2) -> Option<Hit> {
    let scene = cx.scene();
    let cast = |at: Vec2| match anchor {
        Some(_) => raycast_anchor(scene, anchor, &cx.view.ray(at)),
        None => cx.pick(at),
    };
    let scale = cx.view.pixels_per_point;
    let mut radius = SNAP_STEP * scale;
    while radius <= SNAP_REACH * scale {
        let mut best: Option<(f32, Hit)> = None;
        for k in 0..SNAP_DIRECTIONS {
            let angle = k as f32 / SNAP_DIRECTIONS as f32 * std::f32::consts::TAU;
            let toward = Vec2::new(angle.cos(), angle.sin());
            let Some(mut hit) = cast(px + toward * radius) else { continue };
            let (mut off, mut on) = (0.0, radius);
            for _ in 0..6 {
                let middle = 0.5 * (off + on);
                match cast(px + toward * middle) {
                    Some(found) => (on, hit) = (middle, found),
                    None => off = middle,
                }
            }
            // A step inside the outline, where the surface is not seen edge on.
            let inside = cast(px + toward * (on + 2.0 * scale));
            if best.is_none_or(|(nearest, _)| on < nearest) {
                best = Some((on, inside.unwrap_or(hit)));
            }
        }
        if let Some((_, hit)) = best {
            return Some(hit);
        }
        radius += SNAP_STEP * scale;
    }
    None
}

impl TubeTool {
    // ---- picking -------------------------------------------------------------

    /// The handle nearest `px`, and how far away it is in pixels.
    fn pick_handle(&self, view: &View, px: Vec2) -> Option<(HandleHit, f32)> {
        let mut best = None;
        let mut best_score = HANDLE_PICK * view.pixels_per_point;
        for (t, tube) in self.tubes.iter().enumerate() {
            for (k, copy) in tube.copies.iter().enumerate() {
                for (i, world) in copy.handle_world.iter().enumerate().take(tube.state.points.len()) {
                    let Some(s) = view.project(*world) else { continue };
                    let distance = s.distance(px);
                    // A little preference for the selected tube, and for the
                    // tube itself over its mirror images, where handles
                    // overlap.
                    let score = distance + if Some(t) == self.active { 0.0 } else { 1.0 } + if k == 0 { 0.0 } else { 0.5 };
                    if score < best_score {
                        best_score = score;
                        best = Some((HandleHit { tube: t, copy: k, point: i }, distance));
                    }
                }
            }
        }
        best
    }

    /// The nearest point on any tube's centre line, within the tube's own
    /// thickness on screen (and never less than the pick distance), so a
    /// press anywhere on a tube's body hits it.
    fn pick_curve(&self, view: &View, px: Vec2) -> Option<CurveHit> {
        let mut best = None;
        let mut best_score = f32::MAX;
        for (t, tube) in self.tubes.iter().enumerate() {
            for (k, copy) in tube.copies.iter().enumerate() {
                let screen: Vec<Option<Vec2>> = copy.curve_world.iter().map(|p| view.project(*p)).collect();
                for i in 1..screen.len() {
                    let (Some(a), Some(b)) = (screen[i - 1], screen[i]) else { continue };
                    let (distance, f) = distance_to_segment(px, a, b);
                    let thickness = pixels_for(view, copy.curve_world[i], copy.samples[i].radius);
                    let reach = thickness.max(CURVE_PICK * view.pixels_per_point);
                    // Relative to the reach, so a thin tube in front of a fat
                    // one still wins where the pointer is over it.
                    let score = distance / reach;
                    if distance > reach || score >= best_score {
                        continue;
                    }
                    best_score = score;
                    let (sa, sb) = (&copy.samples[i - 1], &copy.samples[i]);
                    let near = if f < 0.5 { sa } else { sb };
                    let t_on_segment =
                        if sa.segment == sb.segment { sa.segment_t + (sb.segment_t - sa.segment_t) * f } else { near.segment_t };
                    best = Some(CurveHit {
                        tube: t,
                        copy: k,
                        segment: near.segment,
                        t: t_on_segment,
                        world: copy.curve_world[i - 1].lerp(copy.curve_world[i], f),
                    });
                }
            }
        }
        best
    }

    /// Where the radius handle of the selected point is drawn: out from the
    /// point by the tube's radius there, across the tube as it is seen, so
    /// it sits on the tube's outline.
    pub(crate) fn ring_handle(&self, view: &View) -> Option<RingHandle> {
        let (tube, point) = self.active_tube().zip(self.selected_point())?;
        let copy = self.tubes[tube].copies.first()?;
        let world = *copy.handle_world.get(point)?;
        let centre = view.project(world)?;
        let radius = pixels_for(view, world, copy.controls.get(point)?.radius);
        let neighbour = |i: Option<usize>| i.and_then(|i| copy.handle_world.get(i)).and_then(|p| view.project(*p));
        let before = neighbour(point.checked_sub(1)).unwrap_or(centre);
        let after = neighbour(Some(point + 1)).unwrap_or(centre);
        let along = (after - before).normalize_or_zero();
        let mut across = if along == Vec2::ZERO { Vec2::X } else { Vec2::new(-along.y, along.x) };
        // Up the screen (y runs down), or to the right for an upright tube.
        if across.y > 0.0 || (across.y == 0.0 && across.x < 0.0) {
            across = -across;
        }
        let reach = radius.max(RING_HANDLE_MIN * view.pixels_per_point);
        Some(RingHandle { tube, point, centre, at: centre + across * reach })
    }

    /// The radius handle, if `px` is on it and no point's own handle is
    /// nearer.
    fn pick_ring(&self, view: &View, px: Vec2, handle_distance: Option<f32>) -> Option<RingHandle> {
        let ring = self.ring_handle(view)?;
        let distance = ring.at.distance(px);
        (distance < HANDLE_PICK * view.pixels_per_point && handle_distance.is_none_or(|d| distance <= d)).then_some(ring)
    }

    /// What is under a free pointer: lights handles, shows where a click on
    /// the curve would add a point, and sizes the next stroke's ring.
    pub(crate) fn update_hover(&mut self, cx: &Ctx) {
        self.pointer.clear_hover();
        self.pointer.last_px = cx.pointer;
        if !cx.hovering || !matches!(self.pointer.drag, Drag::None) || self.finish.is_busy() {
            return;
        }
        let (view, px) = (cx.view, cx.pointer);
        let handle = self.pick_handle(&view, px);
        if self.pick_ring(&view, px, handle.map(|(_, d)| d)).is_some() {
            self.pointer.hover_ring = true;
            return;
        }
        self.pointer.hover_handle = handle.map(|(hit, _)| hit);
        if handle.is_none() {
            self.pointer.hover_curve = self.pick_curve(&view, px);
        }
        let under = cx.pick(px).map(|hit| hit.position).or_else(|| view.on_facing_plane(px, default_depth_point(cx)));
        self.pointer.cursor = under.map(|p| (px, p));
    }

    // ---- press, drag, release ------------------------------------------------

    /// A left press at `px`. Grabs the radius handle or the point under it,
    /// adds a point where the selected tube's curve is pressed, selects
    /// another tube, or starts a stroke on the model (anywhere, with
    /// `mid_air`). False when there was nothing there to take: the camera
    /// then orbits.
    pub(crate) fn press(&mut self, cx: &Ctx, px: Vec2, extend: bool, mid_air: bool) -> bool {
        self.pointer.last_px = px;
        if self.finish.is_busy() {
            return false;
        }
        if !matches!(self.pointer.drag, Drag::None) {
            return true;
        }
        self.refresh(cx, false);
        let view = cx.view;
        let handle = self.pick_handle(&view, px);
        if let Some(ring) = self.pick_ring(&view, px, handle.map(|(_, d)| d)) {
            self.begin_ring_drag(ring, px);
            return true;
        }
        if let Some((hit, _)) = handle {
            self.press_handle(cx, hit, px, extend);
            return true;
        }
        self.pointer.last_click = None;
        if let Some(hit) = self.pick_curve(&view, px) {
            if Some(hit.tube) != self.active {
                self.select_tube(hit.tube, None);
                let title = self.kind().title();
                self.say(format!("{title} selected. Click its curve to add a point, drag a point to move it."));
                self.pointer.drag = Drag::Idle;
            } else {
                self.insert_point(cx, hit, px);
            }
            return true;
        }
        self.begin_draw(cx, px, mid_air)
    }

    fn press_handle(&mut self, cx: &Ctx, hit: HandleHit, px: Vec2, extend: bool) {
        let again = self.pointer.last_click.is_some_and(|(last, at)| {
            last.tube == hit.tube && last.point == hit.point && self.now - at < DOUBLE_CLICK_SECONDS
        });
        self.pointer.last_click = Some((hit, self.now));
        self.select_tube(hit.tube, Some(hit.point));
        if again {
            self.pointer.last_click = None;
            self.toggle_sharp(hit.tube, hit.point);
            self.pointer.drag = Drag::Idle;
            return;
        }
        let count = self.point_count(hit.tube);
        if extend && (hit.point == 0 || hit.point + 1 == count) {
            // A second point on top of the end, dragged away from it.
            let label = self.label("Extend Tube", "Extend Muscle");
            self.begin_edit(label, 0.0);
            let index = if hit.point == 0 { 0 } else { count };
            let end = self.tubes[hit.tube].state.points[hit.point];
            self.tubes[hit.tube].state.points.insert(index, end);
            self.touch();
            self.refresh(cx, false);
            self.select_tube(hit.tube, Some(index));
            self.begin_point_drag(cx, hit.tube, hit.copy, index, px, label);
            return;
        }
        self.begin_point_drag(cx, hit.tube, hit.copy, hit.point, px, "Move Point");
    }

    /// A right press at `px`: deletes the point under it. False if there is
    /// none.
    pub(crate) fn right_press(&mut self, cx: &Ctx, px: Vec2) -> bool {
        if self.finish.is_busy() || !matches!(self.pointer.drag, Drag::None) {
            return false;
        }
        self.refresh(cx, false);
        let Some((hit, _)) = self.pick_handle(&cx.view, px) else { return false };
        self.delete_point(hit.tube, hit.point);
        self.pointer.drag = Drag::Idle;
        true
    }

    /// The pointer at `px` with the button held.
    pub(crate) fn drag_to(&mut self, cx: &Ctx, px: Vec2) {
        self.pointer.last_px = px;
        match self.pointer.drag {
            Drag::Point(_) => self.update_point_drag(cx, px),
            Drag::Draw => self.update_draw(cx, px),
            Drag::Ring { tube, point, start_size, centre, start_distance } => {
                let factor = centre.distance(px) / start_distance;
                self.set_point_size(tube, point, start_size * factor);
            }
            Drag::None | Drag::Idle | Drag::Resize { .. } => {}
        }
    }

    /// The button let go.
    pub(crate) fn release(&mut self, cx: &Ctx) {
        match self.pointer.drag {
            Drag::Draw => self.finish_draw(cx),
            // S is still held: the button was never part of that gesture.
            Drag::Resize { .. } => {}
            _ => self.end_gesture(),
        }
    }

    // ---- point drag ----------------------------------------------------------

    fn begin_point_drag(&mut self, cx: &Ctx, tube: usize, copy: usize, point: usize, px: Vec2, label: &'static str) {
        self.begin_edit(label, 0.0);
        let live = &self.tubes[tube];
        let copy = copy.min(live.copies.len().saturating_sub(1));
        let Some(&plane_point) = live.copies.get(copy).and_then(|c| c.handle_world.get(point)) else { return };
        let grab_offset = cx.view.project(plane_point).map_or(Vec2::ZERO, |s| s - px);
        let start = live.state.points[point];
        self.pointer.drag = Drag::Point(PointDrag { tube, copy, point, start, press: px, grab_offset, plane_point, moved: false });
    }

    fn update_point_drag(&mut self, cx: &Ctx, px: Vec2) {
        let Drag::Point(mut drag) = self.pointer.drag else { return };
        let Some(live) = self.tubes.get(drag.tube) else { return };
        if drag.point >= live.state.points.len() || drag.copy >= live.copies.len() {
            return;
        }
        if !drag.moved && px.distance(drag.press) < DRAG_THRESHOLD * cx.view.pixels_per_point {
            return;
        }
        drag.moved = true;
        self.pointer.drag = Drag::Point(drag);
        // A press that became a drag is not the first half of a double click.
        self.pointer.last_click = None;

        let op = live.copies[drag.copy].op;
        let at = px + drag.grab_offset;
        // A tube's point on the surface stays on it wherever points are being placed. A muscle's goes
        // by the placement: with Free it lifts off and floats, which is how a muscle on the model is
        // made to bridge a hollow or is taken off the model altogether.
        let stays = drag.start.is_on_surface() && self.kind() == Kind::Tube;
        let on_model = if stays || self.defaults.on_surface {
            raycast_anchor(cx.scene(), live.state.anchor, &cx.view.ray(at))
        } else {
            None
        };
        let (world, normal) = match on_model {
            Some(hit) => (hit.position, hit.normal),
            // Off the model (or floating): slide on the plane facing the view
            // through where the handle was at the press. The handle is the
            // lifted centre, so the point becomes a free one at that centre.
            None => match cx.view.on_facing_plane(at, drag.plane_point) {
                Some(on_plane) => (on_plane, Vec3::ZERO),
                None => return,
            },
        };
        let mut placed = live.frame.to_tube_point(&op, world, normal, drag.start.size, drag.start.sharp);
        if !placed.is_on_surface() {
            // Off the surface a muscle stays turned the way it was.
            placed.facing = drag.start.up();
        }
        self.tubes[drag.tube].state.points[drag.point] = placed;
        self.touch();
    }

    // ---- insert point --------------------------------------------------------

    /// A new point on the curve where it was pressed: its position the
    /// curve's own at that spot (so the tube does not move), then dragged
    /// like any other.
    fn insert_point(&mut self, cx: &Ctx, hit: CurveHit, px: Vec2) {
        let live = &self.tubes[hit.tube];
        let copy = &live.copies[hit.copy];
        let points = &live.state.points;
        if points.len() < 2 || copy.controls.len() != points.len() {
            return;
        }
        let segment = hit.segment.clamp(0, points.len() - 2);
        let (a, b) = (points[segment], points[segment + 1]);

        // The centre line there, in the copy's frame, taken back to the
        // tube's own space.
        let tension = curve::tension_for(live.state.settings.smoothness);
        let mut world = live.frame.frame_to_world().transform_point3(curve::evaluate(&copy.controls, tension, segment, hit.t));
        let mut world_normal = Vec3::ZERO;
        let size = a.size + (b.size - a.size) * hit.t;

        let muscle = live.state.settings.kind == Kind::Muscle;
        let mut facing = Vec3::ZERO;
        if muscle {
            // A muscle's points are on the surface themselves, and so is the place pressed: the
            // station there, with its lift taken off again.
            let nearest = copy.samples.iter().min_by(|x, y| {
                let d = |s: &crate::types::Sample| (s.segment as f32 + s.segment_t - segment as f32 - hit.t).abs();
                d(x).total_cmp(&d(y))
            });
            if let Some(station) = nearest {
                let settings = &live.state.settings;
                let ground = station.center - station.up * crate::muscle::lift(settings, station.radius, station.flat);
                let frame_to_world = live.frame.frame_to_world();
                world = frame_to_world.transform_point3(ground);
                if a.is_on_surface() && b.is_on_surface() {
                    world_normal = frame_to_world.transform_vector3(station.up).normalize_or_zero();
                } else {
                    facing = a.up().lerp(b.up(), hit.t).normalize_or_zero();
                }
            }
        } else if a.is_on_surface() && b.is_on_surface() {
            // Between two surface points the new one is dropped back onto
            // the surface (un-lifted along the neighbours' blended normal),
            // so it stays a surface point; and snapped onto the model when
            // the view ray through it finds the model close by.
            let radius = curve::evaluate_radius(&copy.controls, segment, hit.t);
            let normal_to_world = Mat3::from_mat4(live.frame.local_to_world().inverse()).transpose();
            let blended = normal_to_world * copy.op.apply(a.surface_normal.lerp(b.surface_normal, hit.t));
            if blended.length_squared() > 1e-12 {
                world_normal = blended.normalize();
                world -= world_normal * (radius * SURFACE_LIFT);
                let through = cx.view.project(world).map(|at| cx.view.ray(at));
                let found = through.and_then(|ray| raycast_anchor(cx.scene(), live.state.anchor, &ray));
                if let Some(found) = found.filter(|found| found.position.distance(world) < radius) {
                    world = found.position;
                    world_normal = found.normal;
                }
            }
        }

        let mut inserted = live.frame.to_tube_point(&copy.op, world, world_normal, size, false);
        inserted.facing = facing;
        self.begin_edit("Add Point", 0.0);
        self.tubes[hit.tube].state.points.insert(segment + 1, inserted);
        self.touch();
        self.refresh(cx, false);
        self.select_tube(hit.tube, Some(segment + 1));
        self.begin_point_drag(cx, hit.tube, hit.copy, segment + 1, px, "Add Point");
    }

    // ---- sizing --------------------------------------------------------------

    fn set_point_size(&mut self, tube: usize, point: usize, size: f32) {
        let Some(p) = self.tubes.get_mut(tube).and_then(|t| t.state.points.get_mut(point)) else { return };
        let size = size.clamp(TubeSettings::MIN_SIZE, TubeSettings::MAX_SIZE);
        if size.is_finite() && size != p.size {
            p.size = size;
            self.touch();
        }
    }

    /// The wheel over a point: thicker or thinner there. False when the
    /// pointer is on no point, which leaves the wheel to the camera.
    pub(crate) fn wheel(&mut self, cx: &Ctx, px: Vec2, notches: f32) -> bool {
        if self.finish.is_busy() || !matches!(self.pointer.drag, Drag::None) || notches == 0.0 {
            return false;
        }
        self.refresh(cx, false);
        let Some((hit, _)) = self.pick_handle(&cx.view, px) else { return false };
        self.select_tube(hit.tube, Some(hit.point));
        self.begin_edit("Point Size", COALESCE_SECONDS);
        let size = self.tubes[hit.tube].state.points[hit.point].size * WHEEL_SIZE_STEP.powf(notches.signum());
        self.set_point_size(hit.tube, hit.point, size);
        self.say(format!("Point size {:.2}x", self.tubes[hit.tube].state.points[hit.point].size));
        true
    }

    fn begin_ring_drag(&mut self, ring: RingHandle, px: Vec2) {
        self.begin_edit("Point Size", 0.0);
        self.pointer.drag = Drag::Ring {
            tube: ring.tube,
            point: ring.point,
            start_size: self.tubes[ring.tube].state.points[ring.point].size,
            centre: ring.centre,
            start_distance: ring.centre.distance(px).max(1.0),
        };
    }

    /// S went down: sideways travel now sizes the point under the pointer
    /// (or the selected one) if there is one; otherwise the selected tube's
    /// radius; otherwise the radius the next stroke will use.
    fn begin_resize(&mut self, cx: &Ctx) {
        if self.finish.is_busy() || !matches!(self.pointer.drag, Drag::None) {
            return;
        }
        self.refresh(cx, false);
        let px = cx.pointer;
        let hovered = self.pick_handle(&cx.view, px).filter(|_| cx.hovering);
        let (tube, point) = match hovered {
            Some((hit, _)) => {
                self.select_tube(hit.tube, Some(hit.point));
                (Some(hit.tube), Some(hit.point))
            }
            None => (self.active_tube(), self.selected_point()),
        };
        let start_value = match tube.zip(point) {
            Some((t, p)) => {
                self.begin_edit("Point Size", 0.0);
                self.tubes[t].state.points[p].size
            }
            None => self.settings().radius,
        };
        self.pointer.drag = Drag::Resize { tube, point, start_value, start_x: px.x };
    }

    /// While S is held: the pointer wherever it has got to. Doubles every
    /// `RESIZE_PER_DOUBLING` points of travel to the right.
    pub(crate) fn follow_resize(&mut self, px: Vec2) {
        let Drag::Resize { tube, point, start_value, start_x } = self.pointer.drag else { return };
        let factor = 2f32.powf((px.x - start_x) / RESIZE_PER_DOUBLING);
        match tube.zip(point) {
            Some((t, p)) => self.set_point_size(t, p, start_value * factor),
            None => self.set_radius(start_value * factor),
        }
    }

    // ---- drawing -------------------------------------------------------------

    /// Starts a stroke at `px`. It is anchored to the object under the
    /// pointer; with `mid_air` it may also start where there is none, at the
    /// depth of the selected object, and is then anchored to that.
    fn begin_draw(&mut self, cx: &Ctx, px: Vec2, mid_air: bool) -> bool {
        let scene = cx.scene();
        let muscle = self.defaults.kind == Kind::Muscle;
        // A muscle that starts just off the model starts on it.
        let hit = cx.pick(px).or_else(|| snap_to_model(cx, None, px).filter(|_| muscle && self.defaults.on_surface && !mid_air));
        if hit.is_none() && !mid_air {
            return false;
        }
        let selected = scene.active_info().filter(|info| info.visible).map(|info| info.id);
        let anchor = hit.map(|h| h.object).or(selected);
        self.pointer.stroke = Stroke {
            anchor,
            on_surface: self.defaults.on_surface && anchor.is_some(),
            plane_point: hit.map_or_else(|| default_depth_point(cx), |h| h.position),
            last_px: px,
            world: Vec::new(),
            normal: Vec::new(),
            px: Vec::new(),
            muscle,
            toward_viewer: -cx.view.forward,
            over: None,
        };
        self.pointer.drag = Drag::Draw;
        self.add_stroke_sample(cx, px);
        self.pointer.draft = Some(self.new_tube(anchor));
        self.active = None;
        self.selected_point = None;
        true
    }

    fn update_draw(&mut self, cx: &Ctx, px: Vec2) {
        if px.distance(self.pointer.stroke.last_px) < STROKE_SPACING * cx.view.pixels_per_point {
            return;
        }
        self.pointer.stroke.last_px = px;
        if self.add_stroke_sample(cx, px) {
            let kept = if self.pointer.stroke.muscle {
                self.simplify_muscle_stroke(cx, MAX_STROKE_POINTS)
            } else {
                self.simplify_stroke(self.defaults.radius * 0.12, 400)
            };
            self.fill_draft(cx, &kept);
        }
    }

    /// One pointer sample of the stroke: on the model if placing on the
    /// surface and the pointer is over it; otherwise on the plane facing the
    /// view at the stroke's depth.
    fn add_stroke_sample(&mut self, cx: &Ctx, px: Vec2) -> bool {
        let stroke = &mut self.pointer.stroke;
        let mut on_model = if stroke.on_surface { raycast_anchor(cx.scene(), stroke.anchor, &cx.view.ray(px)) } else { None };
        if on_model.is_none() && stroke.on_surface && stroke.muscle {
            on_model = snap_to_model(cx, stroke.anchor, px);
        }
        if let Some(hit) = on_model {
            stroke.world.push(hit.position);
            stroke.normal.push(hit.normal);
            stroke.px.push(px);
            stroke.plane_point = hit.position;
            return true;
        }
        let Some(on_plane) = cx.view.on_facing_plane(px, stroke.plane_point) else { return false };
        if stroke.muscle && !stroke.on_surface {
            let under = cx.pick(px).map(|hit| hit.position);
            let nearer = |p: Vec3| stroke.over.is_none_or(|over| cx.view.depth_of(p) < cx.view.depth_of(over));
            stroke.over = under.filter(|&p| nearer(p)).or(stroke.over);
        }
        stroke.world.push(on_plane);
        stroke.normal.push(Vec3::ZERO);
        stroke.px.push(px);
        true
    }

    fn simplify_stroke(&self, mut tolerance: f32, max_points: usize) -> Vec<usize> {
        let world = &self.pointer.stroke.world;
        let mut kept = curve::simplify(world, tolerance);
        while kept.len() > max_points {
            tolerance = tolerance.max(1e-6) * 1.4;
            kept = curve::simplify(world, tolerance);
        }
        kept
    }

    /// A muscle's points from its stroke. The line is judged as it was drawn, on screen, not in space:
    /// a muscle is laid on the surface between its points whatever they are, so a straight line over
    /// a shoulder is two points, where a tube's stroke needs a point for every turn of the surface
    /// under it. The tolerance is half the muscle's radius, which takes a shaky line as a clean one.
    fn simplify_muscle_stroke(&self, cx: &Ctx, max_points: usize) -> Vec<usize> {
        let stroke = &self.pointer.stroke;
        let flat: Vec<Vec3> = stroke.px.iter().map(|p| p.extend(0.0)).collect();
        let Some(&middle) = stroke.world.get(stroke.world.len() / 2) else { return Vec::new() };
        let mut tolerance = (pixels_for(&cx.view, middle, self.defaults.radius) * 0.5).max(2.0);
        let mut kept = curve::simplify(&flat, tolerance);
        while kept.len() > max_points {
            tolerance *= 1.4;
            kept = curve::simplify(&flat, tolerance);
        }
        kept
    }

    /// The draft tube's points from the stroke samples worth keeping.
    fn fill_draft(&mut self, cx: &Ctx, kept: &[usize]) {
        let Some(draft) = &mut self.pointer.draft else { return };
        let info = draft.state.anchor.and_then(|a| cx.scene().info(a));
        let frame = info.map_or(crate::live::Frame::WORLD, |info| crate::live::Frame { placement: info.placement });
        let stroke = &self.pointer.stroke;
        let facing = if stroke.muscle { frame.to_tube_direction(&SymmetryOp::IDENTITY, stroke.toward_viewer) } else { Vec3::ZERO };
        // A tube drawn free grows out of the spot it was started on. A muscle drawn free floats: on
        // the plane facing the view that clears everything it was drawn over by its own depth. On the
        // plane through where it started it would be half inside the model there, and out of sight
        // wherever the model bulges toward the viewer.
        let clear = draft.state.settings.thin_radius() * 1.25;
        let afloat = stroke.over.map(|over| over + stroke.toward_viewer * clear);
        let place = |i: usize| afloat.and_then(|plane| cx.view.on_facing_plane(stroke.px[i], plane)).unwrap_or(stroke.world[i]);
        draft.state.points = kept
            .iter()
            .map(|&i| {
                let mut point = frame.to_tube_point(&SymmetryOp::IDENTITY, place(i), stroke.normal[i], 1.0, false);
                if !point.is_on_surface() {
                    point.facing = facing;
                }
                point
            })
            .collect();
    }

    /// The stroke becomes a tube: simplified to a handful of points (few
    /// enough to refine by hand, and the curve through them follows the
    /// stroke), or nothing, for a click.
    fn finish_draw(&mut self, cx: &Ctx) {
        self.pointer.drag = Drag::None;
        if self.pointer.draft.is_none() {
            return;
        }
        let world = &self.pointer.stroke.world;
        let length: f32 = world.windows(2).map(|w| w[0].distance(w[1])).sum();
        let radius = self.defaults.radius;
        if world.len() < 2 || length < radius * 0.75 {
            self.pointer.draft = None;
            self.pointer.stroke = Stroke::default();
            self.say(format!("Drag further to draw a {}.", self.kind().one()));
            return;
        }
        // Tolerance in proportion to the tube's thickness: a wobble a
        // fraction of the radius wide is not something the tube can show
        // anyway.
        let kept = if self.pointer.stroke.muscle {
            self.simplify_muscle_stroke(cx, MAX_STROKE_POINTS)
        } else {
            self.simplify_stroke((radius * 0.3).max(length * 0.004), MAX_STROKE_POINTS)
        };
        self.fill_draft(cx, &kept);
        self.pointer.stroke = Stroke::default();
        let Some(draft) = self.pointer.draft.take() else { return };

        let on = draft.state.anchor.and_then(|a| cx.scene().info(a)).map(|info| info.name.to_owned());
        let points = draft.state.points.len();
        self.begin_edit(self.label("Draw Tube", "Draw Muscle"), 0.0);
        self.tubes.push(draft);
        self.touch();
        self.commit_edit();
        self.select_tube(self.tubes.len() - 1, None);
        let title = self.kind().title();
        self.say(match on {
            Some(name) => format!("{title} drawn on {name}: {points} points. Drag them to refine; Enter merges."),
            None => format!("{title} drawn in space: {points} points. Keep Separate makes it an object."),
        });
    }

    // ---- ending a gesture ----------------------------------------------------

    /// Ends any gesture, keeping what it did.
    pub(crate) fn end_gesture(&mut self) {
        let drag = std::mem::take(&mut self.pointer.drag);
        match drag {
            Drag::Draw => {
                self.pointer.draft = None;
                self.pointer.stroke = Stroke::default();
            }
            Drag::Point(_) | Drag::Ring { .. } | Drag::Resize { .. } => self.commit_edit(),
            Drag::None | Drag::Idle => {}
        }
    }

    /// Esc, or anything that interrupts a gesture: puts back what it
    /// changed. False if there was no gesture.
    pub(crate) fn cancel_gesture(&mut self) -> bool {
        let drag = std::mem::take(&mut self.pointer.drag);
        self.pointer.draft = None;
        self.pointer.stroke = Stroke::default();
        match drag {
            Drag::None => false,
            Drag::Point(_) | Drag::Ring { .. } => {
                self.cancel_edit();
                true
            }
            Drag::Resize { point, start_value, .. } => {
                match point {
                    Some(_) => self.cancel_edit(),
                    None => self.set_radius(start_value),
                }
                true
            }
            Drag::Draw | Drag::Idle => true,
        }
    }

    // ---- keys ----------------------------------------------------------------

    pub(crate) fn key_down(&mut self, cx: &mut Ctx, key: Key) -> bool {
        if key == Key::Escape {
            if self.cancel_merge() {
                return true;
            }
            if self.cancel_gesture() {
                self.say("Cancelled.");
                return true;
            }
            // Then the point, then the tube; with nothing selected Esc is
            // the app's, which puts the tool away.
            return self.selected_point.take().is_some() || self.active.take().is_some();
        }
        // S is held for as long as it sizes; its repeats are part of that.
        if key == Key::S {
            self.begin_resize(cx);
            return true;
        }
        if !matches!(self.pointer.drag, Drag::None) {
            return false;
        }
        match key {
            // Always the tool's while it is up, with or without a selection:
            // left to the app, Delete would delete the selected object.
            Key::Delete | Key::Backspace => {
                if self.finish.is_busy() {
                } else if let Some((tube, point)) = self.active_tube().zip(self.selected_point()) {
                    self.delete_point(tube, point);
                } else if self.active_tube().is_some() {
                    self.delete_active_tube();
                }
                true
            }
            Key::Enter => {
                self.merge_all(cx);
                true
            }
            _ => false,
        }
    }

    pub(crate) fn key_up(&mut self, key: Key) {
        if key == Key::S && matches!(self.pointer.drag, Drag::Resize { .. }) {
            self.end_gesture();
        }
    }

    // ---- scripted gestures ---------------------------------------------------

    /// Draws a tube along `path` exactly as a stroke of the pointer would.
    /// True if a tube was made.
    pub fn draw_stroke(&mut self, cx: &Ctx, path: &[Vec2], mid_air: bool) -> bool {
        let Some((&first, rest)) = path.split_first() else { return false };
        self.cancel_gesture();
        let before = self.tubes.len();
        if !self.press(cx, first, false, mid_air) {
            return false;
        }
        for &px in rest {
            self.drag_to(cx, px);
        }
        self.release(cx);
        self.refresh(cx, false);
        self.tubes.len() > before
    }
}
