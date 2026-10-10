//! The app's side of the Pose brush: laying its preview out over the
//! viewport every frame, and a scripted figure to pose for visual checks.
//!
//! The brush itself is `crate::sculpt::pose`; a Pose drag is an ordinary
//! stroke and needs nothing here.

use glam::{Quat, Vec2, Vec3};
use serde_json::json;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

use super::{Drag, Gauge, GaugeKind, Running};
use crate::doc::{Action, Tool};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::{Brush, PoseMode};

impl Running {
    /// Keeps the Pose brush's preview current: the chain a press would make
    /// where the pointer rests, or the chain of the drag under way. Runs
    /// every frame, once the point of the model under the pointer is known.
    pub(super) fn pose_frame(&mut self) {
        let doc = &mut self.doc;
        let shown = doc.tool == Tool::Sculpt && !doc.opts.clean_view;
        let free = shown && self.input.drag == Drag::None && self.ui.radial.is_none();
        let in_hand = doc.sculptor.settings.brush == Brush::Pose;
        let under = match self.input.gauge {
            _ if !free || !in_hand => None,
            None => self.input.hover,
            // S held: the ring stays where the key went down while a drag
            // sizes it, and the size is what places the joint. So the chain
            // is shown there, moving along the limb as the ring grows.
            Some(gauge) if gauge.kind == GaugeKind::Size => {
                let (origin, dir) = self.local.ray(gauge.anchor);
                doc.mesh.raycast(origin, dir).map(|hit| hit.pos)
            }
            Some(_) => None,
        };
        // The brush's radius on the model there, as a press works it out.
        let s = &doc.sculptor.settings;
        let radius = |at: Vec3| match s.screen_space_size {
            true => s.radius_px * self.local.world_per_pixel(self.local.depth_of(at)),
            false => s.radius_world,
        };
        let under = under.map(|at| (at, radius(at)));
        doc.sculptor.pose_hover(&doc.mesh, under, self.input.mods.shift_key());
        let per_point = self.egui_ctx.pixels_per_point();
        // A chain that runs behind the camera is left out whole.
        let on_screen = |chain: &[Vec3]| {
            let at = |&p: &Vec3| self.local.project(p).map(|px| egui::pos2(px.x / per_point, px.y / per_point));
            chain.iter().map(at).collect::<Option<Vec<egui::Pos2>>>()
        };
        let chains: Vec<Vec<egui::Pos2>> =
            if shown { doc.sculptor.pose_chains().into_iter().filter_map(on_screen).collect() } else { Vec::new() };
        self.ui.pose = chains;
    }

    // ---- for visual checks ---------------------------------------------------

    /// Stands a small figure in the scene and poses it; used with
    /// `--screenshot` to check the brush by eye. `what` picks the pose:
    ///
    /// - `hover`: the pointer resting on a forearm, so the preview shows
    ///   where it would bend
    /// - `bend`: that forearm part of the way through a drag, mirrored
    /// - `chain`: an arm reaching down with three joints, mirrored
    /// - `twist`: the chest twisted (Ctrl), which carries the head and arms
    ///   round with it
    /// - `resize`: the head made bigger; `slide`: the head slid to one side
    /// - `stretch`: a leg made longer and thinner
    /// - `done`: the bend let go, so the preview shows what the next press
    ///   on the bent arm would do
    /// - `wheel`: the brush wheel held open
    /// - `play`: works the brush through the real pointer and undo routines
    ///   and prints what came of each step
    ///
    /// Words after a colon change the set-up (`bend:close:soft`): `close`
    /// moves the camera in on the part being posed, `soft` sets Rigidity
    /// low, and `offset` sets Joint Offset to half a brush size.
    pub(super) fn pose_demo(&mut self, what: &str) {
        let mut words = what.split(':');
        let what = words.next().unwrap_or_default();
        let options: Vec<&str> = words.collect();
        let close = options.contains(&"close");
        self.replace_mesh(figure(), false);
        self.doc.scene.active_mut().name = "Figure".into();
        self.scene_changed();
        self.sync_view();
        self.frame_view();
        self.doc.camera.update(1.0);
        if close {
            let camera = &mut self.doc.camera;
            camera.pivot = match what {
                "resize" | "slide" => Vec3::new(0.0, 0.95, 0.0),
                "stretch" => Vec3::new(0.0, -0.5, 0.0),
                "twist" => Vec3::new(0.0, 0.6, 0.0),
                _ => Vec3::new(0.55, 0.55, 0.0),
            };
            camera.distance *= if what == "twist" { 0.7 } else { 0.45 };
        }
        self.sync_view();
        self.input.inside = true;
        let s = &mut self.doc.sculptor.settings;
        s.brush = Brush::Pose;
        s.mask_mode = false;
        s.screen_space_size = false;
        s.radius_world = 0.3;
        s.symmetry = [true, false, false];
        if options.contains(&"soft") {
            s.pose.rigidity = 0.15;
        }
        if options.contains(&"offset") {
            s.pose.offset = 0.5;
        }

        let forearm = Vec3::new(0.72, 0.68, 0.068);
        match what {
            "bend" | "done" => self.pose_demo_drag(forearm, Vec3::new(0.6, 0.36, 0.068), what == "done"),
            "chain" => {
                let s = &mut self.doc.sculptor.settings;
                (s.pose.joints, s.radius_world) = (3, 0.23);
                self.pose_demo_drag(Vec3::new(0.84, 0.68, 0.068), Vec3::new(0.52, 0.2, 0.068), false);
            }
            "twist" => {
                // A limb on the mirror plane cannot twist and stay
                // symmetric, so the mirror is off for this one.
                let s = &mut self.doc.sculptor.settings;
                (s.symmetry, s.radius_world) = ([false; 3], 0.45);
                self.input.mods = ModifiersState::CONTROL;
                let chest = Vec3::new(0.08, 0.5, 0.185);
                self.pose_demo_drag(chest, chest + Vec3::new(0.16, 0.0, 0.0), false);
            }
            "resize" | "slide" => {
                let s = &mut self.doc.sculptor.settings;
                (s.pose.mode, s.radius_world) = (PoseMode::Resize, 0.42);
                let crown = Vec3::new(0.0, 1.14, 0.1);
                if what == "slide" {
                    // With the mirror on, a head on the mirror plane only
                    // slides within the plane.
                    self.doc.sculptor.settings.symmetry = [false; 3];
                    self.input.mods = ModifiersState::CONTROL;
                    self.pose_demo_drag(crown, crown + Vec3::new(0.2, 0.0, 0.0), false);
                } else {
                    self.pose_demo_drag(crown, crown + Vec3::new(0.0, 0.2, 0.0), false);
                }
            }
            "stretch" => {
                let s = &mut self.doc.sculptor.settings;
                (s.pose.mode, s.radius_world) = (PoseMode::Stretch, 0.4);
                let shin = Vec3::new(0.135, -0.62, 0.088);
                self.pose_demo_drag(shin, shin + Vec3::new(0.02, -0.25, 0.0), false);
            }
            "wheel" => {
                // In the interface's units, which no frame has set up yet.
                let per_point = self.window.scale_factor() as f32 * self.doc.opts.ui_scale;
                let middle = (self.view.origin + self.view.size * 0.5) / per_point;
                self.ui.radial = Some(egui::pos2(middle.x, middle.y));
            }
            "play" => self.pose_play(),
            _ => {
                let at = self.pose_demo_pixel(forearm);
                self.pointer_moved(at, None);
            }
        }
    }

    /// Where a point of the model is in the window.
    fn pose_demo_pixel(&self, at: Vec3) -> Vec2 {
        self.view.project(at).unwrap_or(self.view.size * 0.5)
    }

    /// Presses on the model at `from` and drags the pointer to `to`, by way
    /// of the routines the window feeds. With `release` false the drag is
    /// left under way, so the frames that follow draw its preview.
    fn pose_demo_drag(&mut self, from: Vec3, to: Vec3, release: bool) {
        let (from, to) = (self.pose_demo_pixel(from), self.pose_demo_pixel(to));
        self.pointer_moved(from, None);
        self.pointer_pressed(MouseButton::Left, None);
        for i in 1..=12 {
            self.pointer_moved(from.lerp(to, i as f32 / 12.0), None);
        }
        if release {
            self.pointer_released(MouseButton::Left);
        }
    }

    /// Works the Pose brush the way a person would, through the real pointer
    /// and undo routines, and prints what came of each step: a check of the
    /// wiring that needs no one at the tablet.
    fn pose_play(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let steps = |app: &Running| app.doc.history.steps().0;
        let preview = |app: &mut Running| {
            app.pose_frame();
            app.ui.pose.clone()
        };
        // What `frame` does for the pointer each time round.
        let look = |app: &mut Running| {
            app.sync_view();
            let (origin, dir) = app.local.ray(app.input.pointer);
            app.input.hover = app.doc.mesh.raycast(origin, dir).map(|hit| hit.pos);
        };
        let forearm = Vec3::new(0.72, 0.68, 0.068);
        let at = self.pose_demo_pixel(forearm);

        self.doc.sculptor.settings.brush = Brush::Clay;
        self.pointer_moved(at, None);
        look(self);
        say("another brush shows no chain", preview(self).is_empty());
        self.doc.sculptor.settings.brush = Brush::Pose;
        let hover = preview(self);
        say("resting on a forearm shows a chain for each arm", hover.len() == 2 && hover.iter().all(|c| c.len() == 2));
        self.input.mods = ModifiersState::SHIFT;
        say("but not with Shift held, which smooths", preview(self).is_empty());
        self.input.mods = ModifiersState::empty();
        self.doc.sculptor.settings.pose.joints = 3;
        self.doc.sculptor.settings.radius_world = 0.2;
        say("three joints show as three", preview(self).iter().all(|c| c.len() == 4));
        self.doc.sculptor.settings.pose.joints = 1;
        self.doc.sculptor.settings.radius_world = 0.3;

        // Sizing the brush with S held: the ring stays where the key went
        // down, and so does the chain, its joint following the size.
        self.input.gauge = Some(Gauge { kind: GaugeKind::Size, anchor: at, start: 0.3 });
        self.input.pointer = at + Vec2::new(0.0, -300.0);
        look(self);
        self.doc.sculptor.settings.radius_world = 0.45;
        let sized = preview(self);
        say(
            "with S held the chain stays under the ring and follows its size",
            sized.len() == 2 && sized[0][0] == hover[0][0] && sized[0][1].distance(hover[0][1]) > 20.0,
        );
        self.input.gauge = Some(Gauge { kind: GaugeKind::Strength, anchor: at, start: 0.1 });
        say("F and D show none", preview(self).is_empty());
        self.input.gauge = None;
        self.doc.sculptor.settings.radius_world = 0.3;
        self.pointer_moved(at, None);
        look(self);

        let rest = self.doc.mesh.positions.clone();
        let count = steps(self);
        self.pose_demo_drag(forearm, Vec3::new(0.6, 0.36, 0.068), false);
        say("a press on the forearm begins a stroke", self.input.drag == Drag::Sculpt);
        self.act(Action::Undo);
        say("Z during the drag does nothing", steps(self) == count && self.doc.sculptor.is_active());
        look(self);
        let doc = &mut self.doc;
        for sample in self.input.samples.drain(..) {
            doc.sculptor.sample(&mut doc.mesh, &self.local, sample);
        }
        doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
        let during = preview(self);
        say("the preview follows the drag", during.len() == 2 && during != hover);
        self.pointer_released(MouseButton::Left);
        let moved = self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).count();
        println!("     {moved} of {} vertices moved", rest.len());
        say("the drag bent the arms", moved > 500);
        say("as one undo step", steps(self) == count + 1);
        let sides = self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).fold((0, 0), |(l, r), (_, b)| {
            if b.x < 0.0 { (l + 1, r) } else { (l, r + 1) }
        });
        say("on both sides alike", sides.0 > 200 && sides.0 == sides.1);
        self.act(Action::Undo);
        say("which undo takes back exactly", self.doc.mesh.positions == rest);

        // A click that does not move is not an edit.
        self.pointer_moved(at, None);
        self.pointer_pressed(MouseButton::Left, None);
        self.pointer_released(MouseButton::Left);
        say("a click without a drag leaves no undo step", steps(self) == count && self.doc.mesh.positions == rest);

        // Ctrl twists; the chain stays where it is.
        self.input.mods = ModifiersState::CONTROL;
        self.pose_demo_drag(forearm, forearm + Vec3::new(0.0, -0.1, 0.0), true);
        self.input.mods = ModifiersState::empty();
        let twisted = self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).count();
        say("Ctrl + drag twists instead", twisted > 500 && steps(self) == count + 1);
        self.act(Action::Undo);

        // The settings come back from the settings file.
        let s = &mut self.doc.sculptor.settings;
        (s.pose.mode, s.pose.joints, s.pose.offset, s.pose.rigidity) = (PoseMode::Stretch, 4, 0.5, 0.25);
        let kept = crate::prefs::BrushPrefs::of(s);
        s.pose = Default::default();
        s.brush = Brush::Clay;
        kept.apply(s);
        let kept = (s.pose.mode, s.pose.joints, s.pose.offset, s.pose.rigidity) == (PoseMode::Stretch, 4, 0.5, 0.25);
        say("the Pose settings are remembered", s.brush == Brush::Pose && kept);
        self.doc.sculptor.settings.pose = Default::default();

        // The Claude bridge: the same brush through its tools.
        let wanted = json!({
            "brush": "pose", "pose_mode": "bend", "pose_joints": 2, "pose_offset": 0.25, "pose_rigidity": 0.8,
            "radius": 0.08,
        });
        let set = self.call("set_brush", &wanted);
        let s = &self.doc.sculptor.settings;
        say(
            "the bridge picks the Pose brush and sets it up",
            set.is_ok() && s.brush == Brush::Pose && (s.pose.joints, s.pose.offset, s.pose.rigidity) == (2, 0.25, 0.8),
        );
        let reported = set.is_ok_and(|state| state.contains("\"pose\"") && state.contains("\"joints\":2"));
        say("and reports the settings back", reported);
        say("it turns down a mode there is not", self.call("set_brush", &json!({ "pose_mode": "wobble" })).is_err());
        self.doc.sculptor.settings.screen_space_size = true;
        let size = self.view.size;
        let (from, to) = (at / size, self.pose_demo_pixel(Vec3::new(0.6, 0.36, 0.068)) / size);
        let path = json!([[from.x, from.y], [to.x, to.y]]);
        let drawn = self.call("stroke", &json!({ "points": path }));
        println!("     {drawn:?}");
        let bent = self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).count();
        say("a bridge stroke bends the arms, as one undo step", drawn.is_ok() && bent > 500 && steps(self) == count + 1);
        self.act(Action::Undo);
        let drawn = self.call("stroke", &json!({ "points": path, "invert": true }));
        let twisted = self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).count();
        say("and an inverted one twists them", drawn.is_ok() && twisted > 500 && steps(self) == count + 1);
        self.act(Action::Undo);
        say("all of it undone", self.doc.mesh.positions == rest);
        self.doc.sculptor.settings.pose = Default::default();
        self.doc.sculptor.settings.screen_space_size = false;
        self.doc.sculptor.settings.radius_world = 0.3;
        look(self);
    }
}

/// A small figure to pose: a body, a head and four limbs, fused into one
/// surface, mirror-exact from left to right.
fn figure() -> Mesh {
    let mut positions: Vec<Vec3> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();
    // A rounded limb of this thickness from one point to another.
    let mut limb = |from: Vec3, to: Vec3, radius: f32| {
        let part = primitives::build(Primitive::Capsule, 6_000, 0.5);
        let turn = Quat::from_rotation_arc(Vec3::Y, (to - from).try_normalize().unwrap_or(Vec3::Y));
        // The capsule as built is 1 across and 2 tall.
        let scale = Vec3::new(radius / 0.5, from.distance(to) * 0.5 + radius, radius / 0.5);
        let base = positions.len() as u32;
        positions.extend(part.positions.iter().map(|&p| (from + to) * 0.5 + turn * (p * scale)));
        indices.extend(part.indices.iter().map(|&i| i + base));
    };
    let v = Vec3::new;
    limb(v(0.0, 0.05, 0.0), v(0.0, 0.62, 0.0), 0.2);
    limb(v(0.0, 0.7, 0.0), v(0.0, 0.95, 0.0), 0.075);
    limb(v(0.0, 1.0, 0.0), v(0.0, 1.06, 0.0), 0.17);
    for side in [-1.0, 1.0] {
        limb(v(0.2 * side, 0.68, 0.0), v(0.95 * side, 0.68, 0.0), 0.07);
        limb(v(0.11 * side, 0.0, 0.0), v(0.14 * side, -0.85, 0.0), 0.09);
    }
    let joined = Mesh::new(positions, indices, None);
    crate::remesh::voxel_remesh(&joined, 192, [true, false, false]).unwrap_or(joined)
}

#[cfg(test)]
mod tests {
    use glam::{Vec2, Vec3};
    use winit::keyboard::KeyCode;

    use super::super::brush_key;
    use super::figure;
    use crate::camera::Camera;
    use crate::mesh::Mesh;
    use crate::sculpt::{Brush, PointerSample, Sculptor};

    const SIZE: Vec2 = Vec2::new(1600.0, 900.0);

    /// A sculptor holding the Pose brush at a fixed size on the model.
    fn poser(radius: f32, mirror: bool) -> Sculptor {
        let mut sculptor = Sculptor::default();
        let s = &mut sculptor.settings;
        s.brush = Brush::Pose;
        s.screen_space_size = false;
        s.radius_world = radius;
        s.symmetry = [mirror, false, false];
        sculptor
    }

    /// The joints a press at `at` would make, one per chain.
    fn joints(mesh: &Mesh, sculptor: &mut Sculptor, at: Vec3) -> Vec<Vec3> {
        sculptor.pose_hover(mesh, Some((at, sculptor.settings.radius_world)), false);
        sculptor.pose_chains().iter().map(|chain| chain[1]).collect()
    }

    /// Presses at `from`, drags to `to` (seen from the front) and lets go.
    /// Returns how far every vertex moved.
    fn drag(mesh: &mut Mesh, sculptor: &mut Sculptor, from: Vec3, to: Vec3) -> Vec<f32> {
        let before = mesh.positions.clone();
        let mut camera = Camera::default();
        camera.frame(mesh.bounds(), SIZE.x / SIZE.y, false);
        let view = camera.view(Vec2::ZERO, SIZE, camera.distance, 1.0);
        let sample = |at: Vec3, step: usize| PointerSample {
            pos: view.project(at).unwrap(),
            pressure: None,
            time: step as f64 / 240.0,
        };
        assert!(sculptor.begin(mesh, &view, sample(from, 0), false, false), "the press missed the figure");
        for step in 1..=8 {
            sculptor.sample(mesh, &view, sample(from.lerp(to, step as f32 / 8.0), step));
        }
        sculptor.end(mesh, &view);
        mesh.positions.iter().zip(&before).map(|(a, b)| a.distance(*b)).collect()
    }

    /// The figure the demos pose is a body with five things sticking out of
    /// it, on a voxel mesh: the brush has to tell limb from body by itself.
    #[test]
    fn on_a_figure_the_joint_is_always_on_the_side_of_the_body() {
        let mesh = figure();
        // The forearm: an elbow for each arm, a brush size in from the press.
        let elbows = joints(&mesh, &mut poser(0.3, true), Vec3::new(0.72, 0.68, 0.068));
        assert_eq!(elbows.len(), 2);
        for (elbow, side) in elbows.iter().zip([1.0, -1.0]) {
            assert!(elbow.distance(Vec3::new(0.43 * side, 0.68, 0.0)) < 0.03, "elbow at {elbow}");
        }
        // Half-way along the arm with a smaller brush: the brush ring cuts
        // the arm twice, and the joint is the cut nearer the shoulder.
        let upper = joints(&mesh, &mut poser(0.2, false), Vec3::new(0.5, 0.68, 0.068));
        assert!(upper[0].distance(Vec3::new(0.32, 0.68, 0.0)) < 0.03, "joint at {}", upper[0]);
        // A brush too small to reach round the arm takes hold of a patch of
        // skin, not of the arm: there is no cut across the limb to put a
        // joint in, and the patch turns about a point just under itself.
        // (Blender's does the same; the preview shows it before the press.)
        let patch = joints(&mesh, &mut poser(0.1, false), Vec3::new(0.5, 0.68, 0.068));
        assert!(patch[0].distance(Vec3::new(0.5, 0.68, 0.03)) < 0.04, "joint at {}", patch[0]);
        // The chest, with the head and both arms poking out of the ring: the
        // joint is the waist, where the ring cuts the trunk above the legs.
        let waist = joints(&mesh, &mut poser(0.42, false), Vec3::new(0.0, 0.5, 0.195));
        assert!(waist[0].x.abs() < 0.02 && (waist[0].y - 0.13).abs() < 0.08, "joint at {}", waist[0]);
    }

    /// What pokes out of the brush ring on the limb's side goes with the
    /// limb: the hand with the arm, the head and arms with the chest.
    #[test]
    fn on_a_figure_what_lies_past_the_brush_ring_goes_with_the_limb() {
        let rest = figure();
        let near = |at: Vec3| {
            let far = |v: &usize| rest.positions[*v].distance(at);
            (0..rest.vertex_count()).min_by(|a, b| far(a).total_cmp(&far(b))).unwrap()
        };
        let (hand, other_hand) = (near(Vec3::new(1.02, 0.68, 0.0)), near(Vec3::new(-1.02, 0.68, 0.0)));
        let (crown, foot) = (near(Vec3::new(0.0, 1.23, 0.0)), near(Vec3::new(0.14, -0.94, 0.0)));

        // The arm, pressed with a brush that stops short of the hand.
        let mut mesh = figure();
        let from = Vec3::new(0.5, 0.68, 0.068);
        let moved = drag(&mut mesh, &mut poser(0.2, false), from, from + Vec3::new(-0.02, -0.2, 0.0));
        assert!(moved[hand] > 0.5, "the hand moved {}", moved[hand]);
        assert_eq!((moved[other_hand], moved[crown], moved[foot]), (0.0, 0.0, 0.0), "the rest of the figure moved");

        // The chest, turned to one side: the head and both hands go with it,
        // and the feet stay where they are.
        let mut mesh = figure();
        let from = Vec3::new(0.0, 0.5, 0.195);
        let moved = drag(&mut mesh, &mut poser(0.42, false), from, from + Vec3::new(0.25, 0.0, 0.0));
        let carried = [moved[crown], moved[hand], moved[other_hand]];
        assert!(carried[0] > 0.3 && carried[1] > 0.2 && carried[2] > 0.2, "{carried:?}");
        assert_eq!(moved[foot], 0.0, "a foot moved");
    }

    /// 7 is Pose, as in the Unity app, and no two brushes share a key: an
    /// unknown key falls back to 0, which would hand Snake Hook's key to
    /// whichever brush came first.
    #[test]
    fn every_brush_has_a_number_key_of_its_own() {
        assert_eq!(brush_key(Brush::Pose), KeyCode::Digit7);
        assert_eq!(brush_key(Brush::SnakeHook), KeyCode::Digit0);
        for (i, a) in Brush::ALL.into_iter().enumerate() {
            for b in &Brush::ALL[..i] {
                assert_ne!(brush_key(a), brush_key(*b), "{a:?} and {b:?}");
            }
        }
    }
}
