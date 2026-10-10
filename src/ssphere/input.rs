//! Picking and gestures.
//!
//! The ground rules that make this predictable:
//!   - every drag is computed absolutely from state captured at the press
//!     (start positions, start radii, the grab point), never by adding up
//!     per-frame deltas, so nothing drifts or creeps however long or jittery
//!     the drag is, and Esc can put it all back;
//!   - a drag moves the point you grabbed, not the sphere's centre, so
//!     nothing jumps to the cursor on the first frame;
//!   - grabbing a reflection edits the node it reflects, with the cursor
//!     mapped back through the mirror: you drag the right arm and the right
//!     arm follows your hand;
//!   - clicks mean the same thing in every mode (select, right-click delete,
//!     wheel resize); the mode only changes what a drag does;
//!   - posing never stretches a bone;
//!   - a new sphere never sinks into what it is placed on: it rests on the
//!     sphere it grows from, touching the surface under the cursor;
//!   - every gesture is worked out in the rig's own space. The app hands
//!     over the camera and the model in world space, the few entry points
//!     (hover, press, drag, release) move them into the rig's space once
//!     (see `in_rig`), and nothing below them knows the rig may be standing
//!     on an object that has been moved, turned or resized.

use glam::{Quat, Vec2, Vec3};

use super::rig::{self, LinkInstance, Node, SphereInstance};
use super::{Ctx, EditMode, Frame, Hit, MAX_NODE_RADIUS, MIN_NODE_RADIUS, Press, SSpheres, pose};

const DRAG_THRESHOLD_PX: f32 = 3.0;
/// How far a press on empty space may wander and still be a click rather
/// than the start of an orbit. Wider than the drag threshold: a pen tip
/// skids a little as it lands.
const TAP_SLOP_PX: f32 = 6.0;
/// Forgiveness around every sphere and link, in screen pixels, so a
/// fingertip-sized sphere on a zoomed-out view is still grabbable. Only
/// consulted when nothing is hit exactly.
const PICK_SLOP_PX: f32 = 7.0;
/// Links are drawn (and picked) thinner than the skin they describe. At full
/// radius a tube swallows its own end spheres and the armature reads as one
/// sausage with no way to see where the grabbable joints are.
pub(super) const LINK_SCALE: f32 = 0.6;
const WHEEL_RESIZE_PER_NOTCH: f32 = 0.08;
const SCALE_DRAG_PER_PX: f32 = 0.006;
const TWIST_DEG_PER_PX: f32 = 0.5;
/// Hold-S sizing of the next sphere: sideways travel doubles or halves it
/// every this many pixels.
const GAUGE_PX_PER_DOUBLING: f32 = 160.0;

type Ray = (Vec3, Vec3);

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub(super) enum DragKind {
    #[default]
    None,
    /// Pressed on a sphere in Draw mode; a child appears once the pointer
    /// has really moved.
    PendingDraw,
    Draw,
    SizeRoot,
    Move,
    Scale,
    Rotate,
    Twist,
    Pose,
    /// A handle of the transform gizmo on the selected sphere (see gizmo.rs).
    /// The gizmo owns the pointer; the drag only holds what the undo step
    /// goes back to.
    Gizmo,
}

/// What each captured node does while a limb is posed.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PoseRole {
    /// The middle joint itself.
    Joint,
    /// Hangs off the joint but not below the grabbed bone: turns rigidly
    /// with the upper bone.
    WithUpper,
    /// The grabbed bone's child and everything below it: turns rigidly with
    /// the lower bone.
    WithLower,
}

#[derive(Default)]
pub(super) struct Drag {
    pub(super) kind: DragKind,
    pub(super) node: Option<usize>,
    parent: Option<usize>,
    /// The grab was on a reflection; cursor points are mapped back through
    /// the mirror before they reach the rig.
    mirrored: bool,
    start_mouse: Vec2,
    /// Drag planes face the camera and pass through this point.
    plane_anchor: Vec3,
    grab: Vec3,
    pivot: Vec3,
    /// Rotate/Twist: the axis the branch turns about.
    axis: Vec3,
    /// Sizing a root that rests on a model: where it touches, and the
    /// surface normal there. It grows away from the contact.
    resting: Option<(Vec3, Vec3)>,
    nodes: Vec<usize>,
    pub(super) start_positions: Vec<Vec3>,
    start_radii: Vec<f32>,
    // Pose: the two-bone chain root -> joint -> end (end = the grabbed
    // bone's child).
    roles: Vec<PoseRole>,
    pose_root: Option<usize>,
    pose_joint: Option<usize>,
    bend_normal: Vec3,
    bend_side: f32,
}

#[derive(Clone, Copy, Default)]
struct Pick {
    node: Option<usize>,
    /// A picked link, named by its child node.
    link: Option<usize>,
    mirrored: bool,
    /// Where the picked instance is drawn: a node's centre, or the point on
    /// a link's centre line. Drag planes are anchored here.
    point: Vec3,
    /// Parameter along a picked link, 0 at the parent end.
    t: f32,
}

/// Nearest non-negative ray parameter at which a unit-direction ray meets a
/// sphere.
fn ray_sphere(ray: Ray, centre: Vec3, radius: f32) -> Option<f32> {
    let oc = ray.0 - centre;
    let b = oc.dot(ray.1);
    let disc = b * b - (oc.dot(oc) - radius * radius);
    if disc < 0.0 {
        return None;
    }
    let root = disc.sqrt();
    let t = if -b - root >= 0.0 { -b - root } else { -b + root };
    (t >= 0.0).then_some(t)
}

/// Closest approach between a unit-direction ray and the segment a-b: the
/// ray parameter, the segment parameter, the segment point, and the gap.
fn ray_segment(ray: Ray, a: Vec3, b: Vec3) -> (f32, f32, Vec3, f32) {
    let (origin, dir) = ray;
    let ab = b - a;
    let ab2 = ab.dot(ab);
    let ao = origin - a;
    let (t, u);
    if ab2 < 1e-12 {
        u = 0.0;
        t = (-ao.dot(dir)).max(0.0);
    } else {
        let dab = dir.dot(ab);
        let dao = dir.dot(ao);
        let abao = ab.dot(ao);
        let denom = ab2 - dab * dab;
        let along = if denom > 1e-10 { ((abao - dao * dab) / denom).clamp(0.0, 1.0) } else { 0.0 };
        let reach = along * dab - dao;
        if reach < 0.0 {
            t = 0.0;
            u = (abao / ab2).clamp(0.0, 1.0);
        } else {
            t = reach;
            u = ((origin + dir * t - a).dot(ab) / ab2).clamp(0.0, 1.0);
        }
    }
    let on_segment = a + ab * u;
    (t, u, on_segment, (origin + dir * t).distance(on_segment))
}

/// Where a ray meets the camera-facing plane through `anchor`: the distance
/// along the ray and the point.
fn plane_hit(cx: &Ctx, anchor: Vec3, ray: Ray) -> Option<(f32, Vec3)> {
    let denom = ray.1.dot(cx.view.forward);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (anchor - ray.0).dot(cx.view.forward) / denom;
    (t > 0.0).then(|| (t, ray.0 + ray.1 * t))
}

/// The camera-facing plane through the orbit pivot: where the artist is
/// already looking, rather than the world origin, which may be off screen.
fn view_plane_point(cx: &Ctx, ray: Ray) -> Vec3 {
    plane_hit(cx, cx.pivot, ray).map_or(cx.pivot, |(_, p)| p)
}

/// World size of one screen pixel at `point`.
fn pixel_size(cx: &Ctx, point: Vec3) -> f32 {
    cx.view.world_per_pixel(cx.view.depth_of(point))
}

fn signed_angle(from: Vec3, to: Vec3, axis: Vec3) -> f32 {
    let angle = from.angle_between(to);
    if axis.dot(from.cross(to)) < 0.0 { -angle } else { angle }
}

fn turn(from: Vec3, to: Vec3) -> Quat {
    Quat::from_rotation_arc(from.normalize(), to.normalize())
}

impl SSpheres {
    pub fn is_dragging(&self) -> bool {
        self.drag.kind != DragKind::None
    }

    /// The hovered link (named by its child), when a click there would act
    /// on the link itself.
    pub(super) fn hovered_link(&self) -> Option<usize> {
        if self.is_dragging() { None } else { self.hovered_link }
    }

    /// The sphere a click would place, if a click here would place one.
    pub(super) fn ghost(&self) -> Option<(Vec3, f32)> {
        self.ghost.filter(|_| !self.preview)
    }

    // ---- picking ----------------------------------------------------------

    /// What is under the cursor, tested analytically against exactly the
    /// spheres and links that are drawn. Precedence: a sphere hit squarely,
    /// then a link hit squarely, then anything within the pixel slop, so the
    /// forgiveness that makes tiny spheres grabbable never steals a click
    /// from something the cursor is genuinely on.
    fn pick(&self, cx: &Ctx, ray: Ray) -> Pick {
        if self.rig.is_empty() {
            return Pick::default();
        }
        let (spheres, links) = self.rig.geometry(self.symmetry);
        let node_pick = |s: &SphereInstance| Pick {
            node: Some(s.node),
            link: None,
            mirrored: s.mirrored,
            point: s.centre,
            t: 0.0,
        };
        let mut hard: Option<(f32, Pick)> = None;
        let mut soft_sphere: Option<(f32, Pick)> = None;
        for s in &spheres {
            if let Some(t) = ray_sphere(ray, s.centre, s.radius) {
                if hard.is_none_or(|(best, _)| t < best) {
                    hard = Some((t, node_pick(s)));
                }
            } else if let Some(t) = ray_sphere(ray, s.centre, s.radius + PICK_SLOP_PX * pixel_size(cx, s.centre))
                && soft_sphere.is_none_or(|(best, _)| t < best)
            {
                soft_sphere = Some((t, node_pick(s)));
            }
        }
        if let Some((_, pick)) = hard {
            return pick;
        }

        let mut soft_link: Option<(f32, Pick)> = None;
        for l in &links {
            let LinkInstance { a, b, radius_a, radius_b, .. } = *l;
            let (t, u, on_axis, distance) = ray_segment(ray, a, b);
            let radius = (radius_a + (radius_b - radius_a) * u) * LINK_SCALE;
            let pick = Pick { node: None, link: Some(l.child), mirrored: l.mirrored, point: on_axis, t: u };
            if distance <= radius {
                if hard.is_none_or(|(best, _)| t < best) {
                    hard = Some((t, pick));
                }
            } else if distance <= radius + PICK_SLOP_PX * pixel_size(cx, on_axis)
                && soft_link.is_none_or(|(best, _)| t < best)
            {
                soft_link = Some((t, pick));
            }
        }
        hard.or(soft_sphere).or(soft_link).map_or(Pick::default(), |(_, pick)| pick)
    }

    /// Where a ray meets the model the first sphere may rest on, and the
    /// surface normal there, turned to face the camera. Only while the rig is
    /// empty: once a sphere exists nothing is bound to the model's surface
    /// any more, and later spheres are placed and moved freely.
    fn surface_hit(&self, cx: &Ctx, ray: Ray) -> Option<Hit> {
        if !self.rig.is_empty() || !self.snap_to_surface {
            return None;
        }
        let hit = (cx.surface)(ray.0, ray.1)?;
        // A face hit from behind (an open shell, a hole) must still lift the
        // sphere toward the viewer, not push it through to the far side.
        let normal = if hit.normal.dot(ray.1) > 0.0 { -hit.normal } else { hit.normal };
        Some(Hit { normal: normal.try_normalize().unwrap_or(-ray.1), ..hit })
    }

    // ---- the rig's own space ----------------------------------------------

    /// Runs a gesture in the rig's own space: the camera, the pivot and the
    /// model surface as the rig sees them. The rig's frame is a move, a turn
    /// and an even resize, which keeps spheres round and camera-facing
    /// planes facing the camera, so the same maths serves unchanged wherever
    /// the rig stands.
    fn in_rig<R>(&mut self, cx: &Ctx, gesture: impl FnOnce(&mut Self, &Ctx) -> R) -> R {
        let frame = self.frame;
        if frame.is_identity() {
            return gesture(self, cx);
        }
        let view = cx.view.local_to(frame.matrix());
        let surface = |origin: Vec3, direction: Vec3| {
            let hit = (cx.surface)(frame.to_world(origin), frame.direction_to_world(direction))?;
            Some(Hit { point: frame.to_rig(hit.point), normal: frame.direction_to_rig(hit.normal), ..hit })
        };
        gesture(self, &Ctx { view: &view, pivot: frame.to_rig(cx.pivot), surface: &surface, body: cx.body })
    }

    /// Stands an empty rig where its first sphere will belong, before the
    /// ghost is shown or the sphere placed: on the object it is attached to,
    /// else on the model under the cursor, else on the selected object. The
    /// mirror plane the artist already knows is that object's, so the rig
    /// has to share it; standing anywhere else would give the scene a
    /// second, competing centre line. Takes the world-space context.
    fn seat(&mut self, cx: &Ctx, pos: Vec2) {
        if !self.rig.is_empty() {
            return;
        }
        let under_cursor = || {
            let ray = cx.view.ray(pos);
            self.snap_to_surface.then(|| (cx.surface)(ray.0, ray.1)).flatten().map(|hit| hit.body)
        };
        let body = self.attached.or_else(under_cursor).or(cx.body);
        if let Some(body) = body {
            self.seat_empty(&body);
        }
    }

    /// Starts a blockout from nothing: one sphere where the camera is
    /// looking, selected, ready to have the next one pulled out of it. The
    /// rig stands on the object it is attached to, else on the selected one.
    /// The sphere is placed much as a click would place it, so with a model
    /// in the way it rests on the model (and attaches to it) instead of
    /// being buried at the model's centre, where it could be neither seen
    /// nor grabbed. Under symmetry it goes on the mirror plane: the first
    /// sphere of a figure is the one both sides share, not one of a pair.
    ///
    /// Never clears a rig that is already there: a button that reads as "add
    /// one" must not throw a blockout away. False (having only switched to
    /// Draw) when there are spheres already. Takes the world-space context.
    pub fn start_new_rig(&mut self, cx: &Ctx) -> bool {
        self.set_mode(EditMode::Draw);
        if !self.rig.is_empty() {
            return false;
        }
        self.end_drag();
        self.commit_edit();
        if let Some(body) = self.attached.or(cx.body) {
            self.seat_empty(&body);
        }
        // Opened here so the step carries this name; placing the sphere
        // extends it.
        self.begin_edit("New SSphere Rig");
        self.in_rig(cx, |tool, cx| {
            let at = cx.view.project(cx.pivot).unwrap_or(cx.view.origin + cx.view.size * 0.5);
            let mut ray = cx.view.ray(at);
            let onto_plane = |p: Vec3| Vec3::new(0.0, p.y, p.z);
            if tool.symmetry {
                // Toward the same point from the camera's side and as far
                // off, but along the mirror plane, so what the ray lands on
                // is where the plane crosses the front of the model. Seen
                // square from the side there is no such direction, and the
                // camera's own ray stands.
                let target = onto_plane(cx.pivot);
                if let Some(direction) = (target - onto_plane(ray.0)).try_normalize() {
                    ray = (target - direction * ray.0.distance(cx.pivot), direction);
                }
            }
            tool.drag = Drag { start_mouse: at, ..Drag::default() };
            tool.place_root(cx, ray);
            // A surface that leans sideways there would rest the sphere a
            // little off the plane, and give it a twin.
            if tool.symmetry
                && let Some(node) = tool.drag.node
                && let Some(position) = tool.rig.get(node).map(|n| n.position)
            {
                tool.rig.set_position(node, onto_plane(position));
            }
        });
        // A click leaves the press open to size the sphere by dragging; a
        // button has no drag to follow.
        self.end_drag();
        true
    }

    // ---- hover ------------------------------------------------------------

    /// Whether the pointer is on a sphere's body (a reflection too), which
    /// then keeps the press even where an arrow of the gizmo passes over it.
    /// The arrows start at the selected sphere's centre, so without this
    /// every grab of that sphere would be a one-axis drag: the sphere is the
    /// sphere, and the arrows are the arrows only where they stick out past
    /// it. Takes the world-space context.
    pub fn claims_pointer(&mut self, cx: &Ctx, pos: Vec2) -> bool {
        self.in_rig(cx, |tool, cx| tool.pick(cx, cx.view.ray(pos)).node.is_some())
    }

    /// Tracks what the cursor is over between gestures. `blocked` means the
    /// pointer is not the viewport's to read (it is over a panel, or the
    /// camera has it).
    pub fn hover(&mut self, cx: &Ctx, pos: Vec2, blocked: bool) {
        if self.is_dragging() {
            self.ghost = None;
            return;
        }
        if !blocked {
            // So the ghost and the mirror plane show where a click would
            // really put the first sphere.
            self.seat(cx, self.gauge.map_or(pos, |(anchor, _)| anchor));
        }
        self.in_rig(cx, |tool, cx| tool.hover_in_rig(cx, pos, blocked));
    }

    fn hover_in_rig(&mut self, cx: &Ctx, pos: Vec2, blocked: bool) {
        if let Some((anchor, _)) = self.gauge {
            // The ghost freezes where S went down and grows in place.
            self.hovered = None;
            self.hovered_link = None;
            let ray = cx.view.ray(anchor);
            let pick = self.pick(cx, ray);
            self.place_ghost(cx, false, &pick, ray, anchor);
            if self.ghost.is_none() {
                // Somewhere a click would not place (over a sphere, in Move
                // mode...): still show the size being dialled in, on the
                // view plane through the selection.
                let through = self.selected.and_then(|s| self.rig.get(s)).map_or(cx.pivot, |n| n.position);
                let centre = plane_hit(cx, through, ray).map_or(through, |(_, p)| p);
                self.ghost = Some((centre, self.next_radius()));
            }
            return;
        }
        let ray = cx.view.ray(pos);
        let pick = if blocked { Pick::default() } else { self.pick(cx, ray) };
        self.hovered = pick.node;
        self.hovered_link = pick.link.filter(|_| self.mode == EditMode::Draw);
        self.place_ghost(cx, blocked, &pick, ray, pos);
    }

    /// A ghost of the sphere a click would place, shown only where a click
    /// would place one: anywhere with no rig yet, or on empty space in Draw
    /// mode with a sphere selected (the next sphere of its chain).
    fn place_ghost(&mut self, cx: &Ctx, blocked: bool, pick: &Pick, ray: Ray, pos: Vec2) {
        self.ghost = None;
        if blocked || pick.node.is_some() || pick.link.is_some() || self.preview {
            return;
        }
        let radius = self.next_radius();
        if self.rig.is_empty() {
            // Resting on the model, exactly where the root will be put.
            let centre = match self.surface_hit(cx, ray) {
                Some(hit) => hit.point + hit.normal * radius,
                None => view_plane_point(cx, ray),
            };
            self.ghost = Some((self.snap_if_symmetric(centre, radius), radius));
        } else if self.mode == EditMode::Draw
            && let Some(parent) = self.selected
            && let Some((mirrored, _, point)) = self.child_placement(cx, parent, ray, pos)
        {
            // Drawn where it will appear, so the canonical point goes back
            // through the mirror.
            let point = self.snap_if_symmetric(point, radius);
            self.ghost = Some((if mirrored { rig::mirror(point) } else { point }, radius));
        }
    }

    // ---- press / release --------------------------------------------------

    /// A button went down over the viewport. `delete` is the right button,
    /// or Ctrl with the left.
    pub fn press(&mut self, cx: &Ctx, pos: Vec2, shift: bool, delete: bool) -> Press {
        self.tap = None;
        if self.is_dragging() || self.gauge.is_some() {
            return Press::Done;
        }
        if !delete {
            // A pen can land without having hovered first.
            self.seat(cx, pos);
        }
        self.in_rig(cx, |tool, cx| tool.press_in_rig(cx, pos, shift, delete))
    }

    fn press_in_rig(&mut self, cx: &Ctx, pos: Vec2, shift: bool, delete: bool) -> Press {
        let ray = cx.view.ray(pos);
        let pick = self.pick(cx, ray);
        if delete {
            if let Some(node) = pick.node {
                self.delete_node(node);
            }
            return Press::Done;
        }

        self.drag = Drag { start_mouse: pos, mirrored: pick.mirrored, plane_anchor: pick.point, ..Drag::default() };
        if let Some(node) = pick.node.or(pick.link.filter(|_| self.mode != EditMode::Draw)) {
            // A link grabbed outside Draw mode is a handle on the bone's end
            // (the child it runs into), which is what "move, scale or rotate
            // this limb" means.
            let on_link = pick.node.is_none();
            self.selected = Some(node);
            // Dragging a bone in Move mode poses the limb. Shift keeps the
            // plain slide of the branch.
            if !(on_link && self.mode == EditMode::Move && !shift && self.begin_pose(cx, node, ray)) {
                match self.mode {
                    EditMode::Draw => {
                        // Opened now so the snapshot predates the child; the
                        // commit throws it away again if the press turns out
                        // to be a plain selection click.
                        self.begin_edit("Draw Sphere");
                        self.drag.parent = Some(node);
                        self.drag.kind = DragKind::PendingDraw;
                    }
                    // Shift means "just this sphere" on a sphere, but on a
                    // bone it means slide the branch rather than pose it.
                    EditMode::Move => self.begin_move(cx, node, shift && !on_link, ray, "Move Sphere"),
                    EditMode::Scale => {
                        self.begin_edit("Scale Sphere");
                        self.capture(node, shift);
                        self.drag.node = Some(node);
                        self.drag.kind = DragKind::Scale;
                    }
                    EditMode::Rotate => self.begin_rotate(cx, node, ray, shift),
                }
            }
        } else if pick.link.is_some() {
            self.insert_on_link(cx, &pick, ray);
        } else if self.rig.is_empty() {
            self.place_root(cx, ray);
        } else {
            // Empty space beside a rig. The camera gets the drag; if it
            // turns out to be a click, `release` acts on it.
            self.tap = Some(pos);
            return Press::Empty;
        }
        if self.is_dragging() { Press::Drag } else { Press::Done }
    }

    /// The pointer moved with a button held. Returns false once a press on
    /// empty space has moved far enough to be the camera's drag, not a click.
    pub fn drag_to(&mut self, cx: &Ctx, pos: Vec2) -> bool {
        if let Some(start) = self.tap {
            if pos.distance(start) < TAP_SLOP_PX {
                return true;
            }
            self.tap = None;
            return false;
        }
        if self.is_dragging() {
            self.in_rig(cx, |tool, cx| tool.update_drag(cx, pos));
            return true;
        }
        false
    }

    /// The button came up: closes the gesture as one undo step, or acts on a
    /// click on empty space.
    pub fn release(&mut self, cx: &Ctx) {
        if let Some(pos) = self.tap.take() {
            self.in_rig(cx, |tool, cx| tool.empty_click(cx, pos));
        }
        self.end_drag();
    }

    /// A click on empty space. In Draw mode with a sphere selected it adds
    /// the next sphere of the chain, so click after click lays one down.
    /// Anything else deselects, and never spawns a stray disconnected sphere.
    fn empty_click(&mut self, cx: &Ctx, pos: Vec2) {
        let parent = self.selected.filter(|&s| self.mode == EditMode::Draw && self.rig.is_alive(s));
        let Some(parent) = parent else {
            self.selected = None;
            return;
        };
        let Some((_, _, point)) = self.child_placement(cx, parent, cx.view.ray(pos), pos) else { return };
        self.begin_edit("Place Sphere");
        let radius = self.next_radius();
        if let Some(child) = self.rig.add_child(parent, self.snap_if_symmetric(point, radius), radius) {
            self.last_radius = radius;
            self.selected = Some(child);
        }
        self.commit_edit();
    }

    pub(super) fn end_drag(&mut self) {
        if !self.is_dragging() {
            return;
        }
        self.drag = Drag::default();
        // One gesture, one undo step: closed here rather than per frame.
        self.commit_edit();
    }

    /// Esc: mid-drag, puts the rig back exactly as it was at the press;
    /// otherwise lets go of the selection, so a click in empty space means
    /// nothing again. Returns whether there was anything to do.
    pub fn escape(&mut self) -> bool {
        if self.is_dragging() {
            self.drag = Drag::default();
            self.cancel_edit();
            return true;
        }
        self.selected.take().is_some()
    }

    // ---- wheel and the size gauge -----------------------------------------

    /// The wheel resizes the sphere under the cursor, or the one being
    /// dragged (so a limb can be drawn out and sized in one motion). Returns
    /// false when there is no sphere for it to act on.
    pub fn wheel(&mut self, lines: f32) -> bool {
        let factor = 1.0 + lines.signum() * WHEEL_RESIZE_PER_NOTCH;
        if self.is_dragging() {
            let sized = !matches!(self.drag.kind, DragKind::Scale | DragKind::PendingDraw | DragKind::Gizmo);
            if let Some(node) = self.drag.node.filter(|_| sized)
                && let Some(radius) = self.rig.get(node).map(|n| n.radius)
            {
                self.set_node_radius(node, radius * factor);
            }
            return true;
        }
        let Some(node) = self.hovered else { return false };
        let Some(radius) = self.rig.get(node).map(|n| n.radius) else { return false };
        // Streamed: a resize is a run of notches, and one undo step per
        // notch would take a dozen presses to walk back.
        self.begin_stream_edit("Resize Sphere");
        self.set_node_radius(node, radius * factor);
        true
    }

    /// S went down: start sizing the next sphere by dragging sideways.
    pub fn gauge_begin(&mut self, pos: Vec2) {
        if !self.is_dragging() {
            self.gauge = Some((pos, self.next_radius()));
        }
    }

    /// Right grows, left shrinks. Returns whether the gauge is open.
    pub fn gauge_move(&mut self, pos: Vec2) -> bool {
        let Some((anchor, start)) = self.gauge else { return false };
        self.set_next_radius(start * 2f32.powf((pos.x - anchor.x) / GAUGE_PX_PER_DOUBLING));
        true
    }

    pub fn gauge_end(&mut self) {
        self.gauge = None;
    }

    // ---- drags ------------------------------------------------------------

    fn update_drag(&mut self, cx: &Ctx, pos: Vec2) {
        let ray = cx.view.ray(pos);
        let travel = pos - self.drag.start_mouse;
        let past_threshold = travel.length() >= DRAG_THRESHOLD_PX;
        match self.drag.kind {
            DragKind::None => {}
            DragKind::PendingDraw => {
                if !past_threshold {
                    return;
                }
                let Some(parent) = self.drag.parent.filter(|&p| self.rig.is_alive(p)) else {
                    return self.end_drag();
                };
                // Born resting on the sphere it was dragged off (the cursor
                // is still over it) rather than centred inside it.
                let radius = self.next_radius();
                let Some(point) = self.draw_point(cx, ray, Some(parent), radius) else { return };
                self.drag.node = self.rig.add_child(parent, self.snap_if_symmetric(point, radius), radius);
                self.last_radius = radius;
                self.selected = self.drag.node;
                self.drag.kind = DragKind::Draw;
            }
            DragKind::Draw => {
                let Some((node, radius)) = self.drag_node() else { return self.end_drag() };
                // Slides over its parent instead of through it.
                if let Some(point) = self.draw_point(cx, ray, self.rig.parent_of(node), radius) {
                    self.rig.set_position(node, self.snap_if_symmetric(point, radius));
                }
            }
            DragKind::SizeRoot => {
                // Until the cursor actually moves, the sphere keeps the
                // ghost's size, so a plain click gives exactly what the
                // ghost promised.
                if !past_threshold {
                    return;
                }
                let Some((_, point)) = plane_hit(cx, self.drag.plane_anchor, ray) else { return };
                let Some(node) = self.drag.node else { return };
                self.set_node_radius(node, point.distance(self.drag.plane_anchor));
                // On a model it grows away from where it touches, so it
                // stays resting on it.
                if let (Some((contact, normal)), Some(radius)) =
                    (self.drag.resting, self.rig.get(node).map(|n| n.radius))
                {
                    self.rig.set_position(node, self.snap_if_symmetric(contact + normal * radius, radius));
                }
            }
            DragKind::Move => {
                let Some((_, radius)) = self.drag_node() else { return self.end_drag() };
                let Some(&start) = self.drag.start_positions.first() else { return self.end_drag() };
                let Some(point) = self.canonical_drag_point(cx, ray) else { return };
                let target = self.snap_if_symmetric(start + (point - self.drag.grab), radius);
                self.set_captured(|_, p| p + (target - start));
            }
            DragKind::Scale => {
                // Exponential, so the same hand movement is the same
                // proportional change at any size, and a radius can never be
                // dragged through zero. Right or up grows.
                let factor = ((travel.x - travel.y) * SCALE_DRAG_PER_PX).exp();
                for i in 0..self.drag.nodes.len() {
                    let radius = (self.drag.start_radii[i] * factor).clamp(MIN_NODE_RADIUS, MAX_NODE_RADIUS);
                    self.rig.set_radius(self.drag.nodes[i], radius);
                }
                if let Some((_, radius)) = self.drag_node() {
                    self.last_radius = radius;
                }
            }
            DragKind::Rotate => {
                // Both points lie in the camera-facing plane through the
                // pivot, so the turn is purely about the view axis: exactly
                // the angle the cursor swept around the joint on screen.
                let Some(point) = self.canonical_drag_point(cx, ray) else { return };
                let (pivot, axis) = (self.drag.pivot, self.drag.axis);
                let (from, to) = (self.drag.grab - pivot, point - pivot);
                if from.length_squared() < 1e-10 || to.length_squared() < 1e-10 {
                    return;
                }
                let rotation = Quat::from_axis_angle(axis, signed_angle(from, to, axis));
                self.set_captured(|_, p| pivot + rotation * (p - pivot));
            }
            DragKind::Twist => {
                // Sideways travel rolls the branch around its own bone.
                // Mirrored grabs turn the other way in canonical space, so
                // the reflection on screen rolls the way the hand moved.
                let side = if self.drag.mirrored { -1.0 } else { 1.0 };
                let pivot = self.drag.pivot;
                let rotation = Quat::from_axis_angle(self.drag.axis, (travel.x * TWIST_DEG_PER_PX * side).to_radians());
                self.set_captured(|_, p| pivot + rotation * (p - pivot));
            }
            DragKind::Pose => self.update_pose(cx, ray),
            // Worked out from the gizmo's own placements, not the pointer.
            DragKind::Gizmo => {}
        }
    }

    /// The dragged node and its radius, if it is still alive.
    pub(super) fn drag_node(&self) -> Option<(usize, f32)> {
        let node = self.drag.node?;
        Some((node, self.rig.get(node)?.radius))
    }

    /// Records the nodes a drag will move (a node, or it and everything
    /// below it) with their positions and radii at the press.
    pub(super) fn capture(&mut self, node: usize, subtree: bool) {
        self.drag.nodes.clear();
        if subtree {
            self.rig.collect_subtree(node, &mut self.drag.nodes);
        } else if self.rig.is_alive(node) {
            self.drag.nodes.push(node);
        }
        let nodes: &[Node] = self.rig.nodes();
        self.drag.start_positions = self.drag.nodes.iter().map(|&n| nodes[n].position).collect();
        self.drag.start_radii = self.drag.nodes.iter().map(|&n| nodes[n].radius).collect();
    }

    /// Rewrites every captured node's position as a function of its slot and
    /// its position at the press.
    pub(super) fn set_captured(&mut self, f: impl Fn(usize, Vec3) -> Vec3) {
        for i in 0..self.drag.nodes.len() {
            self.rig.set_position(self.drag.nodes[i], f(i, self.drag.start_positions[i]));
        }
    }

    fn canonical(&self, point: Vec3) -> Vec3 {
        if self.drag.mirrored { rig::mirror(point) } else { point }
    }

    /// The cursor on the camera-facing plane through the grabbed point,
    /// mapped back through the mirror when the grab was on a reflection.
    fn canonical_drag_point(&self, cx: &Ctx, ray: Ray) -> Option<Vec3> {
        plane_hit(cx, self.drag.plane_anchor, ray).map(|(_, p)| self.canonical(p))
    }

    /// The camera's viewing direction in canonical space: reflected when the
    /// drag is on a reflection, so rotations computed in canonical space turn
    /// the reflection the way the cursor moved.
    fn canonical_view_axis(&self, cx: &Ctx) -> Vec3 {
        self.canonical(cx.view.forward)
    }

    fn begin_move(&mut self, cx: &Ctx, node: usize, single: bool, ray: Ray, label: &'static str) {
        let Some(position) = self.rig.get(node).map(|n| n.position) else { return };
        self.begin_edit(label);
        self.capture(node, !single);
        self.drag.node = Some(node);
        self.drag.kind = DragKind::Move;
        self.drag.grab = self.canonical_drag_point(cx, ray).unwrap_or(position);
    }

    fn begin_rotate(&mut self, cx: &Ctx, node: usize, ray: Ray, twist: bool) {
        let Some(position) = self.rig.get(node).map(|n| n.position) else { return };
        // A root has no joint to swing about, so rotating it moves the whole
        // rig instead: the only useful meaning, and it saves a trip to Move.
        let Some(parent) = self.rig.parent_of(node) else {
            return self.begin_move(cx, node, false, ray, "Move Rig");
        };
        let pivot = self.rig.nodes()[parent].position;
        self.drag.node = Some(node);
        self.drag.pivot = pivot;
        if twist {
            let Some(bone) = (position - pivot).try_normalize() else { return };
            self.begin_edit("Twist Branch");
            self.capture(node, true);
            self.drag.axis = bone;
            self.drag.kind = DragKind::Twist;
            return;
        }
        self.begin_edit("Rotate Branch");
        self.capture(node, true);
        self.drag.axis = self.canonical_view_axis(cx);
        // The drag plane goes through the pivot as drawn (the reflection's
        // pivot for a mirrored grab), so cursor points land in the pivot's
        // view plane.
        self.drag.plane_anchor = self.canonical(pivot);
        self.drag.grab = self.canonical_drag_point(cx, ray).unwrap_or(position);
        self.drag.kind = DragKind::Rotate;
    }

    /// Starts posing a limb from a grab on the bone running into `end`: the
    /// bone's parent is the middle joint, its parent the fixed root, and
    /// dragging bends the chain at the joint with both bone lengths kept.
    /// Drag a forearm and the elbow bends while the shoulder stays put.
    /// Everything below the bone rides along rigidly with it, and anything
    /// else hanging off the joint turns with the upper bone. Without a
    /// grandparent there is only one bone, which swings about its parent.
    /// False when the grab is not a bone.
    fn begin_pose(&mut self, cx: &Ctx, end: usize, ray: Ray) -> bool {
        let Some(joint) = self.rig.parent_of(end) else { return false };
        let root = self.rig.parent_of(joint);
        let position = |i: usize| self.rig.nodes()[i].position;
        let (end_position, joint_position) = (position(end), position(joint));

        self.begin_edit("Pose Limb");
        self.drag.node = Some(end);
        self.drag.pose_joint = Some(joint);
        self.drag.pose_root = root;
        match root {
            None => {
                // One bone: rigid swing of the grabbed branch about its parent.
                self.capture(end, true);
                self.drag.roles = vec![PoseRole::WithLower; self.drag.nodes.len()];
            }
            Some(root) => {
                self.capture(joint, true);
                let mut lower = Vec::new();
                self.rig.collect_subtree(end, &mut lower);
                self.drag.roles = self
                    .drag
                    .nodes
                    .iter()
                    .map(|&n| {
                        if n == joint {
                            PoseRole::Joint
                        } else if lower.contains(&n) {
                            PoseRole::WithLower
                        } else {
                            PoseRole::WithUpper
                        }
                    })
                    .collect();
                let r = self.rig.nodes()[root].position;
                let view = self.canonical_view_axis(cx);
                self.drag.bend_normal = pose::bend_normal(r, joint_position, end_position, view);
                self.drag.bend_side = pose::bend_side(r, joint_position, end_position, self.drag.bend_normal);
            }
        }
        self.drag.grab = self.canonical_drag_point(cx, ray).unwrap_or(end_position);
        self.drag.kind = DragKind::Pose;
        true
    }

    fn update_pose(&mut self, cx: &Ctx, ray: Ray) {
        let Some(point) = self.canonical_drag_point(cx, ray) else { return };
        let slot = |node: Option<usize>| node.and_then(|n| self.drag.nodes.iter().position(|&x| x == n));
        let alive = |node: Option<usize>| node.is_some_and(|n| self.rig.is_alive(n));
        let (Some(end_slot), Some(joint)) = (slot(self.drag.node), self.drag.pose_joint.filter(|_| alive(self.drag.node)))
        else {
            return self.end_drag();
        };
        if !self.rig.is_alive(joint) {
            return self.end_drag();
        }
        let end0 = self.drag.start_positions[end_slot];
        let target = end0 + (point - self.drag.grab);

        let Some(root) = self.drag.pose_root else {
            // The joint is the pivot and does not move.
            let pivot = self.rig.nodes()[joint].position;
            let (from, to) = (end0 - pivot, target - pivot);
            if from.length_squared() < 1e-12 || to.length_squared() < 1e-12 {
                return;
            }
            let swing = turn(from, to);
            return self.set_captured(|_, p| pivot + swing * (p - pivot));
        };
        let (Some(joint_slot), true) = (slot(Some(joint)), self.rig.is_alive(root)) else {
            return self.end_drag();
        };
        let root = self.rig.nodes()[root].position;
        let joint0 = self.drag.start_positions[joint_slot];
        let (upper, lower) = ((joint0 - root).length(), (end0 - joint0).length());
        if upper < 1e-6 || lower < 1e-6 {
            return;
        }
        let (joint, end) =
            pose::solve_two_bone(root, upper, lower, target, self.drag.bend_normal, self.drag.bend_side, end0 - root);

        // Minimal-arc turns of each bone from its direction at the press.
        // Absolute from the press, like every other drag, so the pose depends
        // only on where the cursor is now.
        let upper_turn = turn(joint0 - root, joint - root);
        let lower_turn = turn(upper_turn * (end0 - joint0), end - joint) * upper_turn;
        let roles = std::mem::take(&mut self.drag.roles);
        self.set_captured(|i, p| match roles[i] {
            PoseRole::Joint => joint,
            PoseRole::WithLower => end + lower_turn * (p - end0),
            PoseRole::WithUpper => root + upper_turn * (p - root),
        });
        self.drag.roles = roles;
    }

    // ---- creation ---------------------------------------------------------

    /// A Draw drag's point, canonical: resting on `parent` as drawn (its
    /// reflection for a mirrored grab), else on the drag plane.
    fn draw_point(&self, cx: &Ctx, ray: Ray, parent: Option<usize>, radius: f32) -> Option<Vec3> {
        let rest = parent.and_then(|p| self.rig.get(p)).map(|p| (self.canonical(p.position), p.radius));
        self.resting_point(cx, ray, self.drag.plane_anchor, rest, radius).map(|p| self.canonical(p))
    }

    /// Where a sphere of `radius` goes for the cursor ray, as drawn, resting
    /// on whatever it would otherwise sink into:
    ///   - the sphere it grows from (`rest`: centre and radius, as drawn).
    ///     The ray is cast against that sphere grown by `radius`: the hit is
    ///     a centre that is still under the cursor and exactly touches the
    ///     parent, so it slides over the parent's surface. Past the parent's
    ///     outline the ray misses and the point falls on the plane, which is
    ///     where the grown sphere's outline is, so leaving the parent is
    ///     seamless;
    ///   - the model, when it is in front (see [`Self::surface_hit`]);
    ///   - otherwise the camera-facing plane through `plane_anchor`.
    /// The nearest along the ray wins: what the cursor is visibly over is
    /// what the sphere rests on.
    fn resting_point(
        &self,
        cx: &Ctx,
        ray: Ray,
        plane_anchor: Vec3,
        rest: Option<(Vec3, f32)>,
        radius: f32,
    ) -> Option<Vec3> {
        let mut best = plane_hit(cx, plane_anchor, ray);
        let mut nearer = |t: f32, point: Vec3| {
            if best.is_none_or(|(b, _)| t < b) {
                best = Some((t, point));
            }
        };
        if let Some((centre, rest_radius)) = rest
            && let Some(t) = ray_sphere(ray, centre, rest_radius + radius)
        {
            nearer(t, ray.0 + ray.1 * t);
        }
        if let Some(hit) = self.surface_hit(cx, ray) {
            nearer((hit.point - ray.0).dot(ray.1), hit.point + hit.normal * radius);
        }
        best.map(|(_, point)| point)
    }

    /// Drops a root where the cursor is (on the model if it is over one) and
    /// leaves the press open as a sizing drag: click for the ghost's size,
    /// or drag out to size it by hand.
    fn place_root(&mut self, cx: &Ctx, ray: Ray) {
        let landed = self.surface_hit(cx, ray);
        let resting = landed.map(|hit| (hit.point, hit.normal));
        let radius = self.next_radius();
        let centre = match resting {
            Some((contact, normal)) => contact + normal * radius,
            None => view_plane_point(cx, ray),
        };
        self.begin_edit("Place Sphere");
        // Landed on a model with nothing attached: tie the rig to it, as
        // Attach to Selected would, so a blockout started on a body follows
        // that body. Only when the rig is standing on that very model (see
        // `seat`), which is what makes its mirror plane the model's.
        if let Some(hit) = landed
            && self.attached.is_none()
            && self.frame == Frame::IDENTITY.seated_on(&hit.body)
        {
            self.attached = Some(hit.body);
            self.say("The spheres are attached to the model the first one rests on, and follow it when it is moved.");
        }
        let node = self.rig.add_root(self.snap_if_symmetric(centre, radius), radius);
        self.last_radius = radius;
        self.selected = Some(node);
        self.drag = Drag {
            kind: DragKind::SizeRoot,
            node: Some(node),
            start_mouse: self.drag.start_mouse,
            resting,
            // Sized by distance from the contact on a model, from the centre
            // in the open.
            plane_anchor: resting.map_or(self.rig.nodes()[node].position, |(contact, _)| contact),
            ..Drag::default()
        };
    }

    /// Splices a joint into the clicked bone, sized to blend with its
    /// neighbours, and keeps the press going as a single-sphere move: a
    /// click adds a joint and a drag bends the limb at it, in one motion.
    fn insert_on_link(&mut self, cx: &Ctx, pick: &Pick, ray: Ray) {
        let Some(child) = pick.link else { return };
        let Some(parent) = self.rig.parent_of(child) else { return };
        let (a, b) = (&self.rig.nodes()[parent], &self.rig.nodes()[child]);
        // Interpolated between the canonical ends, which is the canonical
        // point whether the drawn link was the original or its reflection.
        let position = a.position.lerp(b.position, pick.t);
        let radius = a.radius + (b.radius - a.radius) * pick.t;

        self.begin_edit("Insert Sphere");
        let Some(inserted) = self.rig.insert_between(child, self.snap_if_symmetric(position, radius), radius)
        else {
            return self.commit_edit();
        };
        self.last_radius = radius;
        self.selected = Some(inserted);
        self.begin_move(cx, inserted, true, ray, "Insert Sphere");
    }

    /// Where a click-placed child of `parent` lands: on the camera-facing
    /// plane through the parent as drawn (its reflection when that is the
    /// one nearer the cursor on screen, so clicking beside the mirrored arm
    /// extends the mirrored arm), resting on the parent rather than inside
    /// it. Returns whether the reflection was used, the plane anchor, and the
    /// canonical point.
    fn child_placement(&self, cx: &Ctx, parent: usize, ray: Ray, pos: Vec2) -> Option<(bool, Vec3, Vec3)> {
        let p = self.rig.get(parent)?;
        let reflection = rig::mirror(p.position);
        let screen = |world: Vec3| cx.view.project(world).map_or(f32::INFINITY, |px| px.distance_squared(pos));
        let mirrored = self.symmetry && screen(reflection) < screen(p.position);
        let anchor = if mirrored { reflection } else { p.position };
        let drawn = self.resting_point(cx, ray, anchor, Some((anchor, p.radius)), self.next_radius())?;
        Some((mirrored, anchor, if mirrored { rig::mirror(drawn) } else { drawn }))
    }
}
