//! The app's side of the transform gizmo: what it sits on (the selected
//! objects while the Transform tool is up, or the target a hosted tool
//! offers while that tool is up), routing the pointer to its handles, and
//! turning each drag into one undo step.
//!
//! The gizmo itself (`crate::gizmo`) only knows a list of targets. This file
//! decides what they are and what a drag of them means: whole objects are
//! placed, a lone object with a mask has the unmasked part of its surface
//! reshaped instead (`masked.rs`), a tool's target is the tool's to move, so
//! it is told where the drag has put it, and the selected SSphere is moved
//! by the spheres themselves (`ssphere/gizmo.rs`).

use glam::{Quat, Vec2, Vec3};
use sculpt_toolkit as kit;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

use super::masked::{Deform, Mirror};
use super::mirror_copies::{place_shown, shown_target};
use super::{Drag, Running};
use crate::doc::{Action, Tool};
use crate::gizmo::{self, Frame, Handles, Orientation, Placement, Target};
use crate::history::History;
use crate::mesh::Mesh;
use crate::mesh::primitives::Primitive;
use crate::scene::{Scene, SceneState, Space};

/// What a handle drag is moving.
enum Dragged {
    /// Whole objects, in the order of the drag's targets, with the object
    /// list as it was at the press for the undo step.
    Objects { ids: Vec<u64>, before: SceneState },
    /// The unmasked part of the active object's surface. `stale` is set
    /// when the pointer has moved since the surface was last reshaped: that
    /// is done once a frame, however many pointer samples arrive in it.
    Surface { deform: Deform, stale: bool },
    /// The target of hosted tool `index`.
    Tool { index: usize },
    /// The selected sphere of the SSphere rig, and its branch.
    Spheres,
}

/// A handle drag under way.
pub(super) struct Dragging {
    drag: gizmo::Drag,
    what: Dragged,
}

#[derive(Default)]
pub(super) struct State {
    dragging: Option<Dragging>,
    /// The Deform card's deformation, while a deformer is picked (app/deform.rs).
    pub(super) deform: super::deform::State,
    /// Whether the pointer rests on a handle of the gizmo a tool asked for (a
    /// hosted tool, or the SSpheres). A press there is the gizmo's, so the
    /// tool is told the pointer is not free (`Env::hovering`) and keeps its
    /// ghosts and highlights back.
    pub(super) on_tool_handle: bool,
}

/// What the gizmo is on.
enum Subject {
    /// These scene objects, in the order of the targets.
    Objects(Vec<u64>),
    /// The target this hosted tool offers.
    Tool(usize),
    /// The selected sphere of the SSphere rig.
    Spheres,
}

/// The gizmo as it stands with nothing being dragged.
struct Setup {
    subject: Subject,
    targets: Vec<Target>,
    handles: Handles,
    orientation: Orientation,
}

/// The selected objects as the gizmo's targets, and their ids in the same
/// order. Only those that can be seen, and the active one first: Local
/// orientation takes its axes from the first target.
pub(super) fn selection_targets(scene: &Scene, slot: &Mesh) -> (Vec<u64>, Vec<Target>) {
    let active = scene.active_id();
    let ids = std::iter::once(active).chain(scene.selected().iter().copied().filter(|&id| id != active));
    ids.filter_map(|id| scene.get(id))
        .filter(|o| o.visible)
        .map(|o| {
            let bounds = scene.bounds_of(o, slot);
            let radius = if bounds.is_empty() { 0.5 } else { bounds.size().length() * 0.5 };
            (o.id, shown_target(scene, o, radius))
        })
        .unzip()
}

/// Puts each object where a drag has it. Ids that are gone are passed over.
pub(super) fn place(scene: &mut Scene, ids: &[u64], placements: &[Placement]) {
    for (&id, placement) in ids.iter().zip(placements) {
        place_shown(scene, id, placement);
    }
}

/// Leaves one undo step for a drag of objects, however many it moved;
/// `before` is the object list at the press. A click that picked a handle
/// without moving it is not an edit, and leaves none. Returns whether a step
/// was recorded.
pub(super) fn record(scene: &Scene, history: &mut History, before: SceneState) -> bool {
    if before.objects == scene.objects {
        return false;
    }
    history.push_scene(before);
    true
}

/// What a hosted tool offers, as the gizmo takes it.
pub(super) fn tool_target(offer: &kit::GizmoTarget) -> (Target, Handles) {
    let p = offer.placement;
    let target = Target {
        placement: Placement { position: p.position, rotation: p.rotation, scale: p.scale },
        radius: offer.radius.max(0.0),
        turns: offer.turns,
        scales: offer.scales,
    };
    (target, Handles { moves: offer.moves, turns: offer.turns, scales: offer.scales })
}

/// What a tool is told about a drag of its target: where the target belongs
/// now, worked out from where it was at the press.
pub(super) fn tool_event(drag: &gizmo::Drag, phase: kit::GizmoPhase) -> kit::Event {
    let p = drag.placements()[0];
    kit::Event::Gizmo(kit::Placement { position: p.position, rotation: p.rotation, scale: p.scale }, phase)
}

/// Points along the green (Y) ring of a gizmo for a scripted drag: `sweep`
/// radians round it, starting half way between the blue and red arms, where
/// no arrow or cube shares the pixel with the ring.
fn green_ring(frame: &Frame, sweep: f32) -> Vec<Vec3> {
    let (u, v) = (frame.axis(2) * frame.arm * 0.63, frame.axis(0) * frame.arm * 0.63);
    (0..=8)
        .map(|k| {
            let a = std::f32::consts::FRAC_PI_4 + k as f32 / 8.0 * sweep;
            frame.pivot + u * a.cos() + v * a.sin()
        })
        .collect()
}

impl Running {
    pub(super) fn transform_active(&self) -> bool {
        self.doc.tool == Tool::Transform
    }

    fn pixels_per_point(&self) -> f32 {
        self.egui_ctx.pixels_per_point()
    }

    /// What the gizmo is on right now, if it is up at all.
    fn gizmo_setup(&self) -> Option<Setup> {
        // Put away, and so not there to be grabbed either, while the panels
        // are hidden.
        if self.doc.opts.clean_view {
            return None;
        }
        if self.transform_active() {
            // A deformer's handles take the arrows' place (app/deform.rs).
            if self.deform_on() {
                return None;
            }
            let (ids, targets) = selection_targets(&self.doc.scene, &self.doc.mesh);
            let orientation = self.doc.opts.gizmo_axes;
            return (!targets.is_empty()).then(|| Setup {
                subject: Subject::Objects(ids),
                targets,
                handles: Handles::ALL,
                orientation,
            });
        }
        // A tool's target brings its own axes; the orientation setting is
        // for the scene selection. The SSpheres are the app's own tool, with
        // arrows on the selected sphere (see ssphere/gizmo.rs).
        if self.spheres_active() {
            let (target, handles) = self.spheres_gizmo()?;
            let orientation = Orientation::Auto;
            return Some(Setup { subject: Subject::Spheres, targets: vec![target], handles, orientation });
        }
        let (index, offer) = self.doc.tools.gizmo(self.doc.tool)?;
        let (target, handles) = tool_target(&offer);
        Some(Setup { subject: Subject::Tool(index), targets: vec![target], handles, orientation: Orientation::Auto })
    }

    fn gizmo_frame(&self, setup: &Setup) -> Option<Frame> {
        let ppp = self.pixels_per_point();
        let frame = Frame::on(&setup.targets, setup.orientation, setup.handles, &self.view, ppp)?;
        Some(match setup.subject {
            // The middle of a tool's own target stays the tool's to press.
            Subject::Tool(_) | Subject::Spheres => frame.keeping_clear(setup.targets[0].radius, &self.view, ppp),
            Subject::Objects(_) => frame,
        })
    }

    /// The left button went down on the viewport. If it is on a handle, a
    /// drag begins and this returns true. A handle wins over the model
    /// whatever is in front: the gizmo is drawn on top, and what you can see
    /// is what you can grab.
    pub(super) fn gizmo_pressed(&mut self) -> bool {
        self.sync_view();
        let pos = self.input.pointer;
        let Some(setup) = self.gizmo_setup() else { return false };
        let Some(frame) = self.gizmo_frame(&setup) else { return false };
        let Some(handle) = frame.pick(&self.view, pos, self.pixels_per_point()) else { return false };
        // Where an arrow passes over a sphere, the sphere keeps the press.
        if matches!(setup.subject, Subject::Spheres) && self.spheres_claims_pointer(pos) {
            return false;
        }
        let drag = gizmo::Drag::on(handle, frame, &setup.targets, &self.view, pos);
        let what = match setup.subject {
            Subject::Objects(ids) => {
                let doc = &mut self.doc;
                // With part of a lone object masked, the drag reshapes the
                // rest of its surface around the mask instead of moving the
                // object. Only ever for one object: a mask lives in one
                // mesh, and that mesh is the active object's.
                let alone = ids == [doc.scene.active_id()];
                // Under the object's mirror symmetry each half moves as the
                // other's mirror image (masked.rs).
                let symmetry = doc.scene.active_symmetry();
                let mirror = Mirror {
                    axes: symmetry.mirror,
                    world: (symmetry.space == Space::World).then(|| doc.scene.active().matrix()),
                };
                match alone.then(|| Deform::begin(&mut doc.mesh, mirror)).flatten() {
                    Some(deform) => {
                        // The shape Reset Shape goes back to is the one
                        // before this drag, as before a brush stroke.
                        doc.scene.keep_baseline(&doc.mesh);
                        Dragged::Surface { deform, stale: false }
                    }
                    None => Dragged::Objects { ids, before: doc.scene.state() },
                }
            }
            Subject::Tool(index) => {
                // Before anything has moved, so the tool can keep the state
                // its own undo step goes back to.
                self.tools_gizmo_event(index, tool_event(&drag, kit::GizmoPhase::Begin));
                Dragged::Tool { index }
            }
            Subject::Spheres => {
                if !self.doc.spheres.begin_gizmo_drag() {
                    return false;
                }
                Dragged::Spheres
            }
        };
        self.transform.dragging = Some(Dragging { drag, what });
        true
    }

    /// Whether a handle drag is under way. Nothing else may edit the scene
    /// until it ends: the drag holds the state its undo step goes back to.
    pub(super) fn transform_dragging(&self) -> bool {
        self.transform.dragging.is_some() || self.transform.deform.grabbing()
    }

    /// Whether a drag is reshaping the active mesh, which is then inside a
    /// stroke and must be left alone, as during a brush stroke.
    pub(super) fn transform_holds_mesh(&self) -> bool {
        matches!(&self.transform.dragging, Some(Dragging { what: Dragged::Surface { .. }, .. }))
    }

    /// A button went down on the viewport while the Transform tool is up.
    pub(super) fn transform_pressed(&mut self, button: MouseButton) -> Drag {
        if let Some(drag) = self.deform_pressed(button) {
            return drag;
        }
        if button != MouseButton::Left {
            return Drag::Ui;
        }
        if self.gizmo_pressed() {
            return Drag::Gizmo;
        }
        // Off the handles a click on another object selects it (with Shift or
        // Ctrl it joins or leaves the selection); a click on nothing leaves
        // the selection alone, since that is far more often a missed handle
        // than a wish to deselect. Dragging orbits.
        if let Some(id) = self.pick(self.input.pointer) {
            self.transform_click(id);
            self.mirror_click_side();
        }
        Drag::Orbit
    }

    /// The pointer moved. True when a handle drag used the movement.
    pub(super) fn transform_moved(&mut self, pos: Vec2) -> bool {
        if self.input.drag != Drag::Gizmo {
            return false;
        }
        if self.deform_moved(pos) {
            return true;
        }
        let Some(mut d) = self.transform.dragging.take() else { return true };
        d.drag.update(&self.view, pos);
        match &mut d.what {
            Dragged::Objects { ids, .. } => place(&mut self.doc.scene, ids, d.drag.placements()),
            Dragged::Surface { stale, .. } => *stale = true,
            Dragged::Tool { index } => self.tools_gizmo_event(*index, tool_event(&d.drag, kit::GizmoPhase::Move)),
            Dragged::Spheres => self.doc.spheres.gizmo_drag_to(d.drag.placements()[0].position),
        }
        self.transform.dragging = Some(d);
        true
    }

    /// Reshapes the surface a masked drag is working on, if the pointer has
    /// moved since the last time. Runs once a frame, before the mesh is sent
    /// to be drawn: a pen reports many times a frame, and each pass goes
    /// over every unmasked vertex.
    pub(super) fn transform_apply(&mut self) {
        let Some(Dragging { drag, what: Dragged::Surface { deform, stale } }) = &mut self.transform.dragging else {
            return;
        };
        if std::mem::take(stale) {
            let doc = &mut self.doc;
            deform.apply(&mut doc.mesh, doc.scene.shown_matrix(), drag.world_delta());
        }
    }

    /// Ends a handle drag, leaving one undo step if it changed anything.
    pub(super) fn transform_released(&mut self) {
        if self.deform_released() || !self.transform_dragging() {
            return;
        }
        // Whatever the pointer did since the last frame still counts.
        self.transform_apply();
        let Some(d) = self.transform.dragging.take() else { return };
        let name = d.drag.handle.name();
        // The drag's undo step is called by its handle, as its toast is.
        self.doc.history.name_next(Some(name.to_owned()));
        match d.what {
            Dragged::Objects { before, .. } => {
                if record(&self.doc.scene, &mut self.doc.history, before) {
                    self.toast(name);
                }
                self.scene_changed();
            }
            Dragged::Surface { deform, .. } => {
                let doc = &mut self.doc;
                let mirrored = deform.mirrored();
                if let Some(delta) = deform.end(&mut doc.mesh) {
                    doc.history.push_stroke(delta);
                    // The surface moved out from under the last brush dab.
                    doc.sculptor.last_hit = None;
                    let object = doc.scene.active().name.clone();
                    self.toast(name);
                    let how = match mirrored {
                        Some(planes) => format!(", each side the mirror image of the other across {planes}"),
                        None => String::new(),
                    };
                    self.status(
                        format!(
                            "Reshaped the unmasked part of \"{object}\"{how}; the masked part stayed where it was. Undo (Ctrl+Z) puts it back."
                        ),
                        true,
                    );
                }
            }
            Dragged::Tool { index } => self.tools_gizmo_event(index, tool_event(&d.drag, kit::GizmoPhase::End)),
            // The spheres keep their own history: the drag is one step there.
            Dragged::Spheres => self.doc.spheres.end_gizmo_drag(),
        }
        self.doc.history.name_next(None);
        self.redraw = true;
    }

    /// Esc during a drag of the handles: the objects, or the reshaped surface,
    /// go back to where they were at the press, and no undo step is left.
    /// False for a drag that is not the app's own to take back (a hosted
    /// tool's target, the spheres: they see Esc first and cancel their own).
    /// The button stays down until it is released, but moves nothing.
    pub(super) fn transform_escape(&mut self) -> bool {
        let ours = matches!(
            &self.transform.dragging,
            Some(Dragging { what: Dragged::Objects { .. } | Dragged::Surface { .. }, .. })
        );
        if !ours {
            return false;
        }
        let Some(d) = self.transform.dragging.take() else { return false };
        match d.what {
            Dragged::Objects { before, .. } => {
                let doc = &mut self.doc;
                doc.scene.restore(before, &mut doc.mesh);
                self.scene_changed();
            }
            Dragged::Surface { deform, .. } => {
                let doc = &mut self.doc;
                if let Some(delta) = deform.end(&mut doc.mesh) {
                    doc.mesh.apply_delta(&delta, true);
                }
                doc.sculptor.last_hit = None;
            }
            Dragged::Tool { .. } | Dragged::Spheres => {}
        }
        self.toast("Move Cancelled");
        self.redraw = true;
        true
    }

    /// Lets go of a sphere drag the spheres have already cancelled (Esc put
    /// them back). The button stays down until it is released, but moves
    /// nothing.
    pub(super) fn transform_cancelled(&mut self) {
        if matches!(&self.transform.dragging, Some(Dragging { what: Dragged::Spheres, .. })) {
            self.transform.dragging = None;
        }
    }

    /// Drops a drag without finishing it (the tool was put away mid-drag).
    pub(super) fn transform_leave(&mut self) {
        self.transform_released();
        // A deformation on the model is kept, as any other command would keep it.
        self.deform_commit();
        self.transform.on_tool_handle = false;
        self.ui.gizmo.clear();
    }

    /// Keeps the drawn gizmo current. Runs every frame.
    pub(super) fn transform_frame(&mut self) {
        let ppp = self.pixels_per_point();
        let (frame, hot) = match &self.transform.dragging {
            // Mid-drag the gizmo is the one that was pressed, with the
            // handles it had then; a move carries it along. A surface being
            // reshaped moves under an object that stays put, and the gizmo
            // stays with the object.
            Some(d) => {
                let frame = match d.what {
                    Dragged::Surface { .. } => d.drag.frame,
                    _ => d.drag.shown_frame(),
                };
                (Some(frame), Some(d.drag.handle))
            }
            None => {
                let setup = self.gizmo_setup();
                let frame = setup.as_ref().and_then(|s| self.gizmo_frame(s));
                let hovering = self.input.inside
                    && self.input.drag == Drag::None
                    && self.ui.radial.is_none()
                    && self.doc.tools.radial.is_none()
                    && !self.over_ui();
                let hot = frame.filter(|_| hovering).and_then(|f| f.pick(&self.view, self.input.pointer, ppp));
                let tools = matches!(setup, Some(Setup { subject: Subject::Tool(_) | Subject::Spheres, .. }));
                // A sphere under the pointer keeps it, whatever arrow is over it.
                let hot = hot.filter(|_| {
                    !matches!(setup, Some(Setup { subject: Subject::Spheres, .. }))
                        || !self.spheres_claims_pointer(self.input.pointer)
                });
                self.transform.on_tool_handle = tools && hot.is_some();
                (frame, hot)
            }
        };
        let mut shapes = frame.map_or(Vec::new(), |f| f.shapes(&self.view, hot, ppp));
        shapes.extend(self.deform_shapes());
        // The interface, which draws these, was built earlier in the frame:
        // a gizmo that has changed needs one more frame to be seen where it
        // now is.
        if shapes != self.ui.gizmo {
            self.ui.gizmo = shapes;
            self.redraw = true;
        }
    }

    /// Puts the active object's turn and size back to none, as one undo step.
    pub(super) fn reset_transform(&mut self) {
        self.edit_scene(|scene, _| {
            let object = scene.active_mut();
            let changed = object.rotation != Quat::IDENTITY || object.scale != Vec3::ONE;
            object.rotation = Quat::IDENTITY;
            object.scale = Vec3::ONE;
            changed
        });
    }

    // ---- for visual checks ---------------------------------------------------

    /// Brings the gizmo up in a state worth looking at; used with
    /// `--screenshot` to check it by eye. The environment variable
    /// `SCULPT_DEMO_GIZMO` picks which:
    ///
    /// - unset: one object, turned and stretched, the gizmo on its own axes
    /// - `global` / `local`: the same with that orientation chosen
    /// - `three`: three objects selected, the gizmo at their shared centre
    /// - `three-turn`: those three part of the way through a turn together
    /// - `masked`: a ball with its lower half masked, part of the way
    ///   through a pull upwards on the green arrow
    /// - `masked-mirror`: a ball with its middle masked under X symmetry,
    ///   part of the way through a pull out on the red arrow: both sides go out
    /// - `play`: works all of the above through the real pointer routines
    ///   and prints what came of each step
    pub(super) fn transform_demo(&mut self) {
        let what = std::env::var("SCULPT_DEMO_GIZMO").unwrap_or_default();
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        match what.as_str() {
            "three" => {
                self.demo_three();
            }
            "three-turn" => {
                self.demo_three();
                // Seen from a little above, so the green ring lies open.
                self.doc.camera.pitch = 35.0;
                self.sync_view();
                if let Some(frame) = self.demo_frame() {
                    self.demo_drag(&green_ring(&frame, std::f32::consts::FRAC_PI_3), false);
                }
            }
            "masked" => {
                self.demo_masked();
                if let Some(frame) = self.demo_frame() {
                    let grab = frame.pivot + frame.axis(1) * frame.arm * 1.1;
                    self.demo_drag(&[grab, grab + frame.axis(1) * 0.35], false);
                }
            }
            "masked-mirror" => {
                self.demo_mirror_masked();
                if let Some(frame) = self.demo_frame() {
                    let grab = frame.pivot + frame.axis(0) * frame.arm * 1.1;
                    self.demo_drag(&[grab, grab + frame.axis(0) * 0.3], false);
                }
            }
            "play" => self.transform_play(),
            _ => {
                self.demo_turned();
                self.doc.opts.gizmo_axes = match what.as_str() {
                    "global" => Orientation::Global,
                    "local" => Orientation::Local,
                    _ => Orientation::Auto,
                };
            }
        }
        self.sync_view();
    }

    /// One object, turned and stretched, with the gizmo up.
    fn demo_turned(&mut self) {
        let object = self.doc.scene.active_mut();
        object.rotation = Quat::from_rotation_z(0.4) * Quat::from_rotation_y(0.5);
        object.scale = Vec3::new(1.0, 1.4, 0.7);
        self.doc.tool = Tool::Transform;
        self.sync_view();
    }

    /// Three objects set round an empty middle, all selected, with the gizmo
    /// up: it sits in the gap between them, on none of the three.
    fn demo_three(&mut self) -> [u64; 3] {
        let first = self.doc.scene.active_id();
        let mut ids = [first; 3];
        self.doc.scene.active_mut().position = Vec3::new(-0.1, 1.2, 0.0);
        let places = [(Primitive::Cube, Vec3::new(1.5, -0.5, 0.0)), (Primitive::Cone, Vec3::new(-1.4, -0.7, 0.5))];
        for (i, (kind, at)) in places.into_iter().enumerate() {
            self.act(Action::New(kind));
            let object = self.doc.scene.active_mut();
            object.position = at;
            object.rotation = Quat::from_rotation_y(0.6 * (i as f32 + 1.0));
            ids[i + 1] = object.id;
        }
        self.act(Action::SelectObject(first));
        self.select_also(ids[1]);
        self.select_also(ids[2]);
        self.doc.tool = Tool::Transform;
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, self.view.size.x / self.view.size.y.max(1.0), false);
        self.sync_view();
        ids
    }

    /// The first object alone, the lower half of it masked with a soft
    /// edge, and the gizmo up.
    fn demo_masked(&mut self) {
        let half = self.doc.mesh.bounds().size().y * 0.5;
        let soft = move |y: f32| {
            let t = ((0.1 * half - y) / (0.3 * half)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        if let Some(delta) = self.doc.mesh.remap_mask(|mesh, v, _| soft(mesh.positions[v as usize].y)) {
            self.doc.history.push_stroke(delta);
        }
        self.doc.tool = Tool::Transform;
        self.sync_view();
    }

    /// The first object alone, its middle across X masked with a soft edge
    /// and both sides free, mirrored across X, and the gizmo up.
    fn demo_mirror_masked(&mut self) {
        let half = self.doc.mesh.bounds().size().x * 0.5;
        let soft = move |x: f32| {
            let t = ((0.45 * half - x.abs()) / (0.2 * half)).clamp(0.0, 1.0);
            t * t * (3.0 - 2.0 * t)
        };
        if let Some(delta) = self.doc.mesh.remap_mask(|mesh, v, _| soft(mesh.positions[v as usize].x)) {
            self.doc.history.push_stroke(delta);
        }
        let id = self.doc.scene.active_id();
        self.doc.scene.edit_symmetry(id, |s| s.mirror = [true, false, false]);
        self.doc.tool = Tool::Transform;
        self.sync_view();
    }

    /// The gizmo as it stands now.
    pub(super) fn demo_frame(&self) -> Option<Frame> {
        self.gizmo_frame(&self.gizmo_setup()?)
    }

    /// Presses at the first world point and drags through the rest, by way
    /// of the routines the window feeds. Returns whether the press took a
    /// handle. With `release` false the drag is left under way.
    pub(super) fn demo_drag(&mut self, path: &[Vec3], release: bool) -> bool {
        let px = |app: &Running, p: Vec3| app.view.project(p).unwrap_or(app.view.size * 0.5);
        let Some((&first, rest)) = path.split_first() else { return false };
        let from = px(self, first);
        self.pointer_moved(from, None);
        self.pointer_pressed(MouseButton::Left, None);
        let took = self.input.drag == Drag::Gizmo;
        let mut at = from;
        for &next in rest {
            let to = px(self, next);
            for i in 1..=4 {
                self.pointer_moved(at.lerp(to, i as f32 / 4.0), None);
            }
            at = to;
        }
        if release {
            self.pointer_released(MouseButton::Left);
        }
        took
    }

    /// Works the gizmo the way a person would, through the real pointer and
    /// undo routines, and prints what came of each step: a check of the
    /// wiring that needs no one at the tablet.
    fn transform_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let place = |app: &Running, id: u64| app.doc.scene.get(id).map(Placement::of);
        let steps = |app: &Running| app.doc.history.steps().0;

        // Three objects, moved and turned as one. Seen from a little above,
        // so the green ring lies open and is dragged round rather than across.
        let ids = self.demo_three();
        self.doc.camera.pitch = 35.0;
        self.sync_view();
        let before: Vec<Placement> = ids.iter().filter_map(|&id| place(self, id)).collect();
        let Some(frame) = self.demo_frame() else { return say("the gizmo is up on three objects", false) };
        let middle = before.iter().map(|p| p.position).sum::<Vec3>() / 3.0;
        say("the gizmo sits at the middle of the three, on the world's axes", {
            frame.pivot.distance(middle) < 1e-4 && frame.axis(0).distance(Vec3::X) < 1e-4
        });
        let count = steps(self);
        let grab = frame.pivot + frame.axis(0) * frame.arm * 1.1;
        say("a press on the red arrow takes the drag", self.demo_drag(&[grab, grab + Vec3::X * 0.5], false));
        // Z in the middle of a drag must not pull the scene out from under it.
        self.act(Action::Undo);
        say("Z during the drag does nothing", steps(self) == count && self.transform_dragging());
        self.pointer_released(MouseButton::Left);
        let after: Vec<Placement> = ids.iter().filter_map(|&id| place(self, id)).collect();
        let moved: Vec<Vec3> = after.iter().zip(&before).map(|(a, b)| a.position - b.position).collect();
        println!("     moved by {:?}", moved[0]);
        say("all three moved the same way", moved.iter().all(|m| m.distance(Vec3::X * 0.5) < 0.02));
        say("as one undo step", steps(self) == count + 1);
        self.act(Action::Undo);
        let back: Vec<Placement> = ids.iter().filter_map(|&id| place(self, id)).collect();
        say("which puts all three back, still selected", back == before && self.doc.scene.selected().len() == 3);
        self.act(Action::Redo);

        let Some(frame) = self.demo_frame() else { return say("the gizmo is still up", false) };
        let before: Vec<Placement> = ids.iter().filter_map(|&id| place(self, id)).collect();
        let quarter = green_ring(&frame, std::f32::consts::FRAC_PI_2);
        say("a press on the green ring takes the drag", self.demo_drag(&quarter, true));
        let spin = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let turned = ids.iter().zip(&before).all(|(&id, b)| {
            place(self, id).is_some_and(|a| {
                a.position.distance(frame.pivot + spin * (b.position - frame.pivot)) < 0.05
                    && a.rotation.angle_between(spin * b.rotation) < 0.05
            })
        });
        say("a quarter turn carries all three round their shared middle and turns each", turned);
        self.act(Action::Undo);

        // One turned object, Global against Local.
        self.act(Action::SelectObject(ids[1]));
        let own = place(self, ids[1]).map_or(Quat::IDENTITY, |p| p.rotation);
        self.doc.opts.gizmo_axes = Orientation::Local;
        say("Local puts the arrows on the object's own axes", {
            self.demo_frame().is_some_and(|f| f.axis(0).distance(own * Vec3::X) < 1e-4)
        });
        self.doc.opts.gizmo_axes = Orientation::Global;
        let frame = self.demo_frame();
        say("Global puts them on the world's, and leaves the cubes on the object's", {
            frame.is_some_and(|f| f.axis(0).distance(Vec3::X) < 1e-4 && f.scale_axis(0).distance(own * Vec3::X) < 1e-4)
        });
        if let (Some(frame), Some(before)) = (frame, place(self, ids[1])) {
            let grab = frame.pivot + Vec3::X * frame.arm * 1.1;
            self.demo_drag(&[grab, grab + Vec3::X * 0.4], true);
            let after = place(self, ids[1]).unwrap_or(before);
            say("a pull on the red arrow then moves along the world's X", {
                (after.position - before.position).distance(Vec3::X * 0.4) < 0.02
            });
            self.act(Action::Undo);
        }
        self.doc.opts.gizmo_axes = Orientation::Auto;

        // A masked object is reshaped, not moved.
        self.act(Action::SelectObject(ids[0]));
        self.demo_masked();
        let rest = self.doc.mesh.positions.clone();
        let mask = self.doc.mesh.mask.clone();
        let (count, at) = (steps(self), place(self, ids[0]));
        if let Some(frame) = self.demo_frame() {
            let grab = frame.pivot + frame.axis(1) * frame.arm * 1.1;
            say("a press on the green arrow of a masked object takes the drag", {
                self.demo_drag(&[grab, grab + frame.axis(1) * 0.3], false)
            });
            say("which holds the mesh for the length of the drag", self.transform_holds_mesh());
            self.pointer_released(MouseButton::Left);
        }
        let mesh = &self.doc.mesh;
        let lift = |v: usize| mesh.positions[v].y - rest[v].y;
        let pinned = (0..rest.len()).filter(|&v| mask[v] >= 1.0).all(|v| mesh.positions[v] == rest[v]);
        let lifted = (0..rest.len()).filter(|&v| mask[v] <= 0.0).all(|v| (lift(v) - 0.3).abs() < 0.02);
        say("the object itself stays where it was", place(self, ids[0]) == at);
        say("fully masked vertices are exactly where they were", pinned);
        say("unmasked vertices went the whole way", lifted);
        say("as one undo step", steps(self) == count + 1 && !self.transform_holds_mesh());
        self.act(Action::Undo);
        say("which puts every vertex back", self.doc.mesh.positions == rest);

        // Under X symmetry with the middle masked, a pull out on the red
        // arrow takes both sides out, as far as each other.
        let symmetry = self.doc.scene.symmetry(ids[0]);
        self.demo_mirror_masked();
        let rest = self.doc.mesh.positions.clone();
        let mask = self.doc.mesh.mask.clone();
        let count = steps(self);
        if let Some(frame) = self.demo_frame() {
            let grab = frame.pivot + frame.axis(0) * frame.arm * 1.1;
            say("under X symmetry a press on the red arrow of a masked object takes the drag", {
                self.demo_drag(&[grab, grab + frame.axis(0) * 0.3], true)
            });
        }
        let mesh = &self.doc.mesh;
        let out = |v: usize| (mesh.positions[v].x - rest[v].x) * rest[v].x.signum();
        let free: Vec<usize> = (0..rest.len()).filter(|&v| mask[v] <= 0.0).collect();
        let sides = [free.iter().filter(|&&v| rest[v].x > 0.0).count(), free.iter().filter(|&&v| rest[v].x < 0.0).count()];
        println!("     free vertices each side: {sides:?}");
        say("both sides went out the whole way", sides[0] > 0 && sides[1] > 0 && free.iter().all(|&v| (out(v) - 0.3).abs() < 0.02));
        let pinned = (0..rest.len()).filter(|&v| mask[v] >= 1.0).all(|v| mesh.positions[v] == rest[v]);
        say("the masked middle stayed where it was", pinned);
        say("as one undo step", steps(self) == count + 1);
        self.act(Action::Undo);
        say("which puts both sides back", self.doc.mesh.positions == rest);
        self.doc.scene.set_symmetry(ids[0], symmetry);

        // A hosted tool's own target: the Studs example puts the gizmo on
        // its last stud.
        self.tools_demo("studs");
        let offered = |app: &Running| app.doc.tools.gizmo(app.doc.tool).map(|(_, target)| target.placement);
        match (offered(self), self.demo_frame()) {
            (Some(before), Some(frame)) => {
                say("the gizmo is on the tool's target", frame.pivot.distance(before.position) < 1e-5);
                // Whichever arrow lies most across the screen.
                let across = |i: &usize| frame.axis(*i).dot(self.view.forward).abs();
                let arrow = (0..3).min_by(|a, b| across(a).total_cmp(&across(b))).unwrap_or(0);
                let pull = frame.axis(arrow) * 0.25;
                let grab = frame.pivot + frame.axis(arrow) * frame.arm * 1.1;
                let count = steps(self);
                say("a press on its arrow takes the drag", self.demo_drag(&[grab, grab + pull], true));
                let after = offered(self).unwrap_or(before);
                println!("     the stud went from {} to {}", before.position, after.position);
                // The example keeps a stud as a matrix, so its turn and size
                // come back through a round trip.
                say("and the tool moved its target along that arrow", {
                    (after.position - before.position).distance(pull) < 0.02
                        && after.rotation.angle_between(before.rotation) < 2e-3
                        && after.scale.distance(before.scale) < 1e-5
                });
                say("without a scene undo step: the stud is the tool's own", steps(self) == count);
            }
            _ => say("the Studs tool offers its last stud to the gizmo", false),
        }
        self.tools_leave();
        say("with the tool put away the gizmo is gone", self.demo_frame().is_none());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::{Camera, View};
    use crate::gizmo::Handle;
    use crate::mesh::primitives;

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    fn cube() -> Mesh {
        primitives::build(Primitive::Cube, 600, 0.5)
    }

    /// Looking down a little on whatever is at `pivot`.
    fn view(pivot: Vec3, distance: f32) -> View {
        let mut camera = Camera::default();
        (camera.pivot, camera.distance, camera.pitch) = (pivot, distance, 35.0);
        camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0)
    }

    /// Three cubes in a row along X, each turned its own way; returns their
    /// ids, top row first.
    fn three(mesh: &mut Mesh) -> (Scene, [u64; 3]) {
        let mut scene = Scene::new("Cube");
        let a = scene.active_id();
        let b = scene.add("Cube", cube(), mesh);
        let c = scene.add("Cube", cube(), mesh);
        for (i, id) in [a, b, c].into_iter().enumerate() {
            let object = scene.get_mut(id).unwrap();
            object.position = Vec3::new(i as f32 * 3.0, 0.5 * i as f32, 0.0);
            object.rotation = Quat::from_rotation_z(0.3 * i as f32);
        }
        scene.select(a, mesh);
        (scene, [a, b, c])
    }

    #[test]
    fn the_targets_are_the_shown_selection_with_the_active_object_first() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        assert_eq!(selection_targets(&scene, &mesh).0, [a]);
        scene.select_also(c);
        scene.select_also(b);
        let (ids, targets) = selection_targets(&scene, &mesh);
        assert_eq!(ids, [a, c, b]);
        assert_eq!(targets[1].placement, Placement::of(scene.get(c).unwrap()));
        // Half the diagonal of a unit cube, whichever mesh is the one in hand.
        assert!(targets.iter().all(|t| (t.radius - 0.866).abs() < 0.3 && t.turns && t.scales));

        // Taking the active one out promotes the next, which then leads.
        scene.toggle_selected(a, &mut mesh);
        scene.toggle_selected(a, &mut mesh);
        assert_eq!((scene.active_id(), selection_targets(&scene, &mesh).0), (c, vec![c, b, a]));

        // What cannot be seen is not moved.
        scene.get_mut(b).unwrap().visible = false;
        assert_eq!(selection_targets(&scene, &mesh).0, [c, a]);
        for id in [a, c] {
            scene.get_mut(id).unwrap().visible = false;
        }
        assert!(selection_targets(&scene, &mesh).0.is_empty());
    }

    #[test]
    fn a_drag_of_several_objects_keeps_them_together_and_is_one_undo_step() {
        let mut mesh = cube();
        let (mut scene, [a, b, c]) = three(&mut mesh);
        let mut history = History::default();
        scene.select_also(b);
        scene.select_also(c);
        let rest = scene.state();
        let middle = Vec3::new(3.0, 0.5, 0.0);
        let view = view(middle, 14.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let object = |scene: &Scene, id: u64| Placement::of(scene.get(id).unwrap());

        // A click on a handle that goes nowhere is not an edit.
        let (ids, targets) = selection_targets(&scene, &mesh);
        let frame = Frame::on(&targets, Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        assert!(frame.pivot.distance(middle) < 1e-5, "the gizmo sits at the shared centre");
        let grab = middle + Vec3::Y * frame.arm;
        let before = scene.state();
        let mut drag = gizmo::Drag::on(Handle::Move(1), frame, &targets, &view, px(grab));
        drag.update(&view, px(grab));
        place(&mut scene, &ids, drag.placements());
        assert!(!record(&scene, &mut history, before));
        assert!(!history.can_undo());

        // Up by one: every object goes up by one.
        let before = scene.state();
        drag.update(&view, px(grab + Vec3::Y));
        place(&mut scene, &ids, drag.placements());
        for (now, was) in scene.objects.iter().zip(&rest.objects) {
            assert!((now.position - was.position).distance(Vec3::Y) < 1e-3);
            assert_eq!((now.rotation, now.scale), (was.rotation, was.scale));
        }
        assert!(record(&scene, &mut history, before));
        assert_eq!(history.steps(), (1, 0), "three objects, one step");

        // A quarter turn about the shared centre: each object swings round
        // it and turns with it, so the set keeps its shape.
        let (ids, targets) = selection_targets(&scene, &mesh);
        let frame = Frame::on(&targets, Orientation::Auto, Handles::ALL, &view, 1.0).unwrap();
        let pivot = frame.pivot;
        let lifted = scene.state();
        let (u, v) = (Vec3::Z * frame.arm * 0.63, Vec3::X * frame.arm * 0.63);
        let mut drag = gizmo::Drag::on(Handle::Rotate(1), frame, &targets, &view, px(pivot + u));
        for k in 1..=30 {
            let angle = k as f32 / 30.0 * std::f32::consts::FRAC_PI_2;
            drag.update(&view, px(pivot + u * angle.cos() + v * angle.sin()));
            place(&mut scene, &ids, drag.placements());
        }
        let spin = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        for (now, was) in scene.objects.iter().zip(&lifted.objects) {
            assert!(now.position.distance(pivot + spin * (was.position - pivot)) < 0.02, "{}", now.position);
            assert!(now.rotation.angle_between(spin * was.rotation) < 0.02);
        }
        let apart = |scene: &[crate::scene::Object], i: usize, j: usize| scene[i].position.distance(scene[j].position);
        for (i, j) in [(0, 1), (1, 2), (0, 2)] {
            assert!((apart(&scene.objects, i, j) - apart(&lifted.objects, i, j)).abs() < 1e-3, "the set changed shape");
        }
        // Seen from any one of them, the others are where they were.
        let seen = |scene: &[crate::scene::Object]| scene[0].rotation.inverse() * (scene[2].position - scene[0].position);
        assert!(seen(&scene.objects).distance(seen(&lifted.objects)) < 0.03);
        assert!(record(&scene, &mut history, lifted.clone()));
        assert_eq!(history.steps(), (2, 0));

        // One undo per drag, each bringing all three back, selection and all.
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.objects, lifted.objects);
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.objects, rest.objects);
        assert_eq!((scene.selected(), scene.active_id()), (&[a, b, c][..], a));
        assert!(!history.can_undo());
        assert!(history.step_scene(false, &mut scene, &mut mesh));
        assert!((object(&scene, c).position - object(&scene, a).position).distance(Vec3::new(6.0, 1.0, 0.0)) < 1e-3);
    }

    /// A tool with one point of its own, which it puts the gizmo on.
    struct Pin {
        at: Vec3,
        heard: Vec<(kit::Placement, kit::GizmoPhase)>,
    }

    impl kit::Tool for Pin {
        fn info(&self) -> kit::Info {
            kit::Info { id: "pin", name: "Pin", icon: "", tip: "", hotkey: None }
        }

        fn gizmo(&self) -> Option<kit::GizmoTarget> {
            Some(kit::GizmoTarget::point(self.at))
        }

        fn input(&mut self, _cx: &mut kit::Ctx, event: kit::Event) -> bool {
            let kit::Event::Gizmo(placement, phase) = event else { return false };
            self.at = placement.position;
            self.heard.push((placement, phase));
            true
        }
    }

    #[test]
    fn a_tool_offered_target_receives_the_drag() {
        use kit::Tool as _;

        // The toolkit's bench stands in for the app's tool calls; the drag
        // itself is the app's own.
        let mut bench = kit::testing::Bench::new();
        let start = Vec3::new(0.4, 0.2, -0.3);
        let mut pin = Pin { at: start, heard: Vec::new() };
        let view = view(Vec3::ZERO, 6.0);
        let px = |p: Vec3| view.project(p).unwrap();

        let offer = pin.gizmo().expect("the pin offers its point");
        let (target, handles) = tool_target(&offer);
        assert_eq!(handles, Handles { moves: true, turns: false, scales: false });
        let frame = Frame::on(&[target], Orientation::Auto, handles, &view, 1.0).unwrap().keeping_clear(target.radius, &view, 1.0);
        assert!(frame.pivot.distance(start) < 1e-6, "the gizmo sits on the point");
        // The point itself is still the tool's to press; its arrows are the gizmo's.
        assert_eq!(frame.pick(&view, px(start), 1.0), None);
        let grab = start + Vec3::X * frame.arm * 0.8;
        assert_eq!(frame.pick(&view, px(grab), 1.0), Some(Handle::Move(0)));

        let mut drag = gizmo::Drag::on(Handle::Move(0), frame, &[target], &view, px(grab));
        bench.send(&mut pin, tool_event(&drag, kit::GizmoPhase::Begin));
        assert_eq!(pin.at, start, "nothing has moved at the press");
        for k in 1..=5 {
            // The pointer wanders upward as it goes; only the travel along
            // the arrow counts (give or take the perspective on the wander).
            drag.update(&view, px(grab + Vec3::new(0.1 * k as f32, 0.04 * k as f32, 0.0)));
            bench.send(&mut pin, tool_event(&drag, kit::GizmoPhase::Move));
        }
        bench.send(&mut pin, tool_event(&drag, kit::GizmoPhase::End));

        assert!(pin.at.distance(start + Vec3::X * 0.5) < 0.05, "{}", pin.at);
        assert_eq!((pin.at.y, pin.at.z), (start.y, start.z));
        let phases: Vec<kit::GizmoPhase> = pin.heard.iter().map(|h| h.1).collect();
        assert_eq!(phases.len(), 7);
        assert_eq!((phases[0], phases[6]), (kit::GizmoPhase::Begin, kit::GizmoPhase::End));
        assert!(phases[1..6].iter().all(|&p| p == kit::GizmoPhase::Move));
        assert_eq!(pin.heard[0].0, offer.placement, "Begin says where the target was");
        assert_eq!(pin.heard[6].0, pin.heard[5].0, "End repeats the last place");
        assert!(pin.heard.iter().all(|h| h.0.rotation == Quat::IDENTITY && h.0.scale == Vec3::ONE));
    }

    #[test]
    fn a_tool_offered_body_is_turned_and_resized_through_the_same_events() {
        use kit::Tool as _;

        // The toolkit's own example puts the gizmo on the stud placed last.
        let mut bench = kit::testing::Bench::new();
        bench.add("Cube", kit::testing::cube(0.5));
        let mut studs = kit::example::Studs::default();
        bench.enter(&mut studs);
        bench.click(&mut studs, bench.px(Vec3::new(0.0, 0.0, 0.5)), kit::Button::Left);
        let offer = studs.gizmo().expect("a stud was placed");
        let (target, handles) = tool_target(&offer);
        assert_eq!(handles, Handles::ALL);
        let start = target.placement;

        let view = view(start.position, 3.0);
        let px = |p: Vec3| view.project(p).unwrap();
        let frame = Frame::on(&[target], Orientation::Auto, handles, &view, 1.0).unwrap().keeping_clear(target.radius, &view, 1.0);
        // The white cube in the middle is the one handle inside the body.
        assert_eq!(frame.pick(&view, px(start.position), 1.0), Some(Handle::Uniform));

        // Twice the size along its own X.
        let own = frame.scale_axis(0);
        let grab = start.position + own * frame.arm * 0.82;
        assert_eq!(frame.pick(&view, px(grab), 1.0), Some(Handle::Scale(0)));
        let mut drag = gizmo::Drag::on(Handle::Scale(0), frame, &[target], &view, px(grab));
        bench.send(&mut studs, tool_event(&drag, kit::GizmoPhase::Begin));
        drag.update(&view, px(start.position + own * frame.arm * 1.64));
        bench.send(&mut studs, tool_event(&drag, kit::GizmoPhase::Move));
        bench.send(&mut studs, tool_event(&drag, kit::GizmoPhase::End));
        let now = studs.gizmo().unwrap().placement;
        assert!((now.scale - start.scale * Vec3::new(2.0, 1.0, 1.0)).length() < 1e-3, "{:?}", now.scale);
        assert!(now.position.distance(start.position) < 1e-6 && now.rotation.angle_between(start.rotation) < 2e-3);
    }
}
