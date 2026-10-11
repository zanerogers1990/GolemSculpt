//! Each object's own symmetry, on the app's side. The settings and their
//! rules are the scene's (`scene/symmetry.rs`); here is where the running app
//! meets them: a stroke begins under the selected object's symmetry and no
//! other, and the scripted checks.
//!
//! The sculptor keeps a working copy of the symmetry it strokes under
//! (`sculpt::Settings`). Nothing in the app writes that copy but
//! `Scene::hand_symmetry_to`, and every stroke the app begins goes through
//! [`Doc::begin_stroke`], which hands it over first. So whichever object was
//! selected a moment ago, and however the selection changed (a click, a
//! tool, undo, a file), a stroke is made under the symmetry of the object it
//! lands on.

use glam::{Quat, Vec2, Vec3};
use sculpt_geom::{MeshAdjacency, MeshEdges};
use sculpt_symmetry::Axis;

use super::Running;
use crate::camera::View;
use crate::doc::{Action, Doc};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::{Space, Symmetry};
use crate::sculpt::{Brush, PointerSample, Radial, RadialAxis};

impl Doc {
    /// Starts a stroke on the active object, under that object's own
    /// symmetry. Returns whether the pointer was over the mesh.
    pub(crate) fn begin_stroke(&mut self, view: &View, sample: PointerSample, invert: bool, smooth_held: bool) -> bool {
        self.scene.hand_symmetry_to(&mut self.sculptor.settings);
        self.sculptor.begin(&mut self.mesh, view, sample, invert, smooth_held)
    }

    /// Changes the selected object's symmetry (and with it that of the
    /// objects sharing its mesh). Not an undo step, as in the Unity app.
    pub(crate) fn edit_symmetry(&mut self, edit: impl FnOnce(&mut Symmetry)) {
        self.scene.edit_symmetry(self.scene.active_id(), edit);
        self.scene.hand_symmetry_to(&mut self.sculptor.settings);
    }
}

impl Running {
    /// For scripted runs: gives the selected object these mirror axes (`None`
    /// leaves them), then takes the words of `SCULPT_DEMO_BRUSH`, the
    /// object's own about its symmetry and the brushes' about the rest.
    pub(super) fn demo_symmetry(&mut self, mirror: Option<[bool; 3]>) {
        self.doc.edit_symmetry(|symmetry| {
            symmetry.mirror = mirror.unwrap_or(symmetry.mirror);
            symmetry.demo_setup();
        });
        self.doc.sculptor.settings.demo_setup(false);
    }

    /// `--demo-symmetry`: two objects with a symmetry each, for a look at
    /// symmetry belonging to objects. A ball on the left mirrored across its
    /// own X, and a cube on the right, moved and turned, with five repeats
    /// about its own Y; a stroke is laid on each while it is selected, and
    /// the cards above the Symmetry card are shut so that it shows.
    /// `SCULPT_DEMO_SYMMETRY` picks which is left selected: `ball` (the
    /// default) or `cube`. `world` stands a second ball in the cube's place,
    /// turned and astride the world's X plane, with Mirror X fixed in the
    /// world, and leaves that selected. `select:<name>` builds nothing: it
    /// selects the object of that name in the scene as it is (a file opened
    /// on the command line), for a look at that object's own symmetry. `cut`
    /// and `corner` are different checks altogether: see
    /// [`Self::cut_and_mirror_demo`] and [`Self::corner_demo`].
    pub(super) fn symmetry_demo(&mut self) {
        let what = std::env::var("SCULPT_DEMO_SYMMETRY").unwrap_or_default();
        if let Some(rest) = what.strip_prefix("cut") {
            return self.cut_and_mirror_demo(rest == ":alone");
        }
        if let Some(view) = what.strip_prefix("corner") {
            return self.corner_demo(view == ":front");
        }
        let shut = ["Brushes", "Clay Settings", "Move Settings"].map(|card| (card.to_owned(), false));
        crate::prefs::cards::restore(&self.egui_ctx, &shut.into_iter().collect());
        if let Some(name) = what.strip_prefix("select:") {
            match self.doc.scene.objects.iter().find(|o| o.name == name).map(|o| o.id) {
                Some(id) => self.act(Action::SelectObject(id)),
                None => eprintln!("demo-symmetry: no object is called {name:?}"),
            }
            let scene = &self.doc.scene;
            for o in &scene.objects {
                println!("symmetry of {:?}: {}", o.name, scene.symmetry(o.id).summary());
            }
            return;
        }
        let world = what == "world";
        let ball = self.doc.scene.active_id();
        self.doc.scene.active_mut().position = Vec3::new(if world { -1.3 } else { -0.75 }, 0.0, 0.0);
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, false, false]);

        self.act(Action::New(if world { Primitive::Sphere } else { Primitive::Cube }));
        let second = self.doc.scene.active_id();
        let object = self.doc.scene.active_mut();
        if world {
            // Its middle on the world's plane, and nothing else about it square to the world.
            object.position = Vec3::new(0.0, 0.2, 0.15);
            object.rotation = Quat::from_rotation_z(0.7) * Quat::from_rotation_y(0.5);
        } else {
            object.position = Vec3::new(0.7, 0.05, 0.0);
            object.rotation = Quat::from_rotation_y(0.5) * Quat::from_rotation_x(-0.35);
        }
        self.doc.edit_symmetry(|symmetry| {
            *symmetry = if world {
                Symmetry { mirror: [true, false, false], space: Space::World, ..Symmetry::default() }
            } else {
                let radial = Radial { on: true, count: 5, axis: RadialAxis::Y, ..Radial::default() };
                Symmetry { radial, ..Symmetry::default() }
            };
        });

        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (if world { 0.0 } else { 20.0 }, 32.0);
        // Room round them: the panels cover the sides of the picture.
        self.doc.camera.distance *= 1.2;

        // A stroke on each, selected in turn: each under its own symmetry.
        // On the ball, up and to the right of its middle on the side facing
        // the camera; on the cube, across its top, which its own Y goes through.
        let on_the_ball = (Vec3::new(0.2, 0.3, 0.5), Vec3::new(0.34, 0.02, 0.5));
        let on_the_cube = (Vec3::new(0.1, 0.45, 0.22), Vec3::new(0.27, 0.45, 0.05));
        for (id, (from, to)) in [(ball, on_the_ball), (second, if world { on_the_ball } else { on_the_cube })] {
            self.act(Action::SelectObject(id));
            // The ball's are measured from its middle along the world's axes.
            let object = self.doc.scene.active();
            let own = id == second && !world;
            let place = |at: Vec3| if own { object.matrix().transform_point3(at) } else { object.position + at };
            let (from, to) = (place(from), place(to));
            self.symmetry_demo_stroke(from, to);
        }
        let keep = if what == "cube" || world { second } else { ball };
        self.act(Action::SelectObject(keep));
        // The brush ring is where the last stroke ended; leave it out of the picture.
        self.input.inside = false;
        self.sync_view();
    }

    /// `SCULPT_DEMO_SYMMETRY=cut`: Cut & Mirror, by the routine its buttons
    /// run (app/cut_mirror.rs), on a lumpy model, for a look at the seam it
    /// leaves. The model as it was stands on the left and a copy of it, cut,
    /// on the right, its +X side kept and mirrored onto -X, both in wireframe
    /// and seen from the front, so the seam runs straight down the middle of
    /// the right one. A band of mask across the model shows the mask coming
    /// through. The cut is then undone and redone. `cut:alone` shows the
    /// result by itself, larger. Prints the status line the cut left and a
    /// line per check (ok / FAIL).
    fn cut_and_mirror_demo(&mut self, alone: bool) {
        // The Symmetry card, with the buttons at its foot, in the picture.
        let shut = ["Brushes", "Clay Settings", "Move Settings"].map(|card| (card.to_owned(), false));
        crate::prefs::cards::restore(&self.egui_ctx, &shut.into_iter().collect());
        // Coarse, so each triangle along the seam can be seen, and moved off
        // centre, so the plane runs through the sides of lumps at a slant.
        let ball = primitives::build(Primitive::Sphere, 6_000, 0.5);
        let lumpy = |p: Vec3| {
            let dir = p.normalize();
            let lumps = (dir.y * 7.0 + 1.0).sin() * (dir.z * 6.0 + 0.5).sin() + 0.5 * (dir.x * 9.0 + dir.y * 5.0).sin();
            Quat::from_rotation_y(0.4) * (dir * (0.5 + 0.11 * lumps)) + Vec3::new(0.09, 0.0, 0.0)
        };
        let positions: Vec<Vec3> = ball.positions.iter().map(|&p| lumpy(p)).collect();
        let band = |p: &Vec3| ((0.12 - (p.y - 0.1 + 0.5 * p.x).abs()) * 30.0).clamp(0.0, 1.0);
        let mask: Vec<f32> = positions.iter().map(band).collect();

        self.replace_mesh(Mesh::new(positions, ball.indices.clone(), Some(mask)), false);
        let before = self.doc.scene.active_id();
        self.doc.scene.active_mut().position = Vec3::new(-0.75, 0.0, 0.0);
        // A copy to the right of it, and the button pressed on that.
        self.act(Action::CloneObject);
        let object = self.doc.scene.active_mut();
        (object.name, object.position) = ("Cut and Mirrored".to_owned(), Vec3::new(0.75, 0.0, 0.0));
        let whole = (self.doc.mesh.vertex_count(), self.doc.mesh.triangle_count());
        let steps = self.doc.history.steps().0;
        self.act(Action::CutAndMirror { axis: Axis::X, from_positive: true });
        println!("{}", self.ui.status.0);

        let mesh = &self.doc.mesh;
        let cut = (mesh.vertex_count(), mesh.triangle_count());
        let built = sculpt_geom::TriMesh::new(mesh.positions.clone(), mesh.indices.clone());
        let adjacency = MeshAdjacency::of(&built);
        let edges = MeshEdges::of(&built, &adjacency);
        let on_plane = built.positions.iter().filter(|p| p.x == 0.0).count();
        let places: std::collections::HashSet<[u32; 3]> =
            built.positions.iter().map(|&p| (p + Vec3::ZERO).to_array().map(f32::to_bits)).collect();
        let mirrored = |p: &Vec3| places.contains(&(Vec3::new(-p.x, p.y, p.z) + Vec3::ZERO).to_array().map(f32::to_bits));
        let masked = |side: f32| mesh.positions.iter().zip(&mesh.mask).any(|(p, &m)| p.x * side > 0.1 && m > 0.9);
        let mut checks = vec![
            ("closed: every edge has two triangles", edges.is_closed()),
            ("no edge has more than two triangles", edges.non_manifold_edges().is_empty()),
            ("every triangle faces the same way as its neighbours", edges.is_consistently_wound(&built.indices)),
            ("every vertex has its mirror image, exactly", built.positions.iter().all(mirrored)),
            ("the seam is a row of vertices on the plane", on_plane > 20),
            ("the mask came across, on both sides", masked(1.0) && masked(-1.0)),
            ("the cut is one undo step", self.doc.history.steps().0 == steps + 1),
            ("the status line is good news", self.ui.status.1 == crate::ui::theme::OK),
        ];
        self.act(Action::Undo);
        let undone = (self.doc.mesh.vertex_count(), self.doc.mesh.triangle_count());
        checks.push(("one Undo brings the whole model back", undone == whole && self.doc.history.steps().0 == steps));
        self.act(Action::Redo);
        let redone = (self.doc.mesh.vertex_count(), self.doc.mesh.triangle_count());
        checks.push(("and Redo cuts it again", redone == cut && cut != whole));
        for (what, ok) in checks {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }

        if alone {
            self.act(Action::ToggleVisible(before));
        }
        self.doc.opts.wireframe = true;
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 12.0);
        if alone {
            self.doc.camera.distance *= 0.55;
        }
        self.input.inside = false;
        self.sync_view();
    }

    /// One scripted Clay stroke on the active object between two places in
    /// the world (each is pulled onto the object's surface along the view).
    pub(super) fn symmetry_demo_stroke(&mut self, from: Vec3, to: Vec3) {
        self.sync_view();
        let settings = &mut self.doc.sculptor.settings;
        (settings.brush, settings.mask_mode) = (Brush::Clay, false);
        settings.radius_px = 0.03 * self.view.size.y;
        settings.per_brush[Brush::Clay.index()].strength = 0.6;
        let on_screen = |at: Vec3| self.view.project(at);
        let (Some(a), Some(b)) = (on_screen(from), on_screen(to)) else { return };
        if !self.demo_stroke_between(a, b) {
            eprintln!("demo-symmetry: the stroke from {from} missed \"{}\"", self.doc.scene.active().name);
        }
    }

    /// One scripted stroke on the active object with the brush as it is set,
    /// from one place on screen to another: a press and a release, one dab,
    /// when they are the same place. Returns whether it landed on the object.
    fn demo_stroke_between(&mut self, a: Vec2, b: Vec2) -> bool {
        let steps = if a == b { 0 } else { 40 };
        let sample = |i: usize| PointerSample {
            pos: a.lerp(b, i as f32 / steps.max(1) as f32),
            pressure: None,
            time: i as f64 / 240.0,
        };
        let doc = &mut self.doc;
        if !doc.begin_stroke(&self.local, sample(0), false, false) {
            return false;
        }
        for i in 1..=steps {
            doc.sculptor.sample(&mut doc.mesh, &self.local, sample(i));
            if i % 4 == 0 {
                doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
            }
        }
        if let Some(delta) = doc.sculptor.end(&mut doc.mesh, &self.local) {
            doc.history.push_stroke(delta);
        }
        true
    }

    /// `SCULPT_DEMO_SYMMETRY=corner`: Layer where two mirror planes cross.
    /// The ball with Mirror X and Y on, and the Layer brush at full strength
    /// pressed once a quarter of its radius from both planes, where its four
    /// copies all but cover each other. A second press on the X plane, well
    /// clear of the corner, and its image across Y are there to measure it
    /// against: each is one copy's worth, the height the brush is set to.
    ///
    /// The camera then goes round to the side, where all three stand on the
    /// outline of the ball. They should be the same height with a flat top;
    /// the one in the corner standing taller is the four copies piling up.
    /// `corner:front` leaves the camera where the dabs were made, looking
    /// down the line the planes cross along.
    fn corner_demo(&mut self, from_the_front: bool) {
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, true, false]);
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 0.0);
        self.sync_view();

        let radius = 0.07 * self.view.size.y;
        let settings = &mut self.doc.sculptor.settings;
        (settings.brush, settings.mask_mode, settings.radius_px) = (Brush::Layer, false, radius);
        // A tall step, so that a third more of it is plain to see.
        settings.layer_height = 0.5;
        settings.per_brush[Brush::Layer.index()].strength = 1.0;
        // The planes cross in the middle of the picture, where the ball's Z comes out.
        let crossing = self.doc.scene.active().matrix().transform_point3(Vec3::Z * 0.5);
        let Some(corner) = self.view.project(crossing) else { return };
        for radii in [Vec2::new(0.25, -0.25), Vec2::new(0.0, -2.4)] {
            let at = corner + radii * radius;
            if !self.demo_stroke_between(at, at) {
                eprintln!("demo-symmetry: the Layer dab at {at} missed the ball");
            }
        }
        if !from_the_front {
            // Close up on the corner: the steps are small beside the ball.
            let camera = &mut self.doc.camera;
            (camera.yaw, camera.pitch, camera.pivot) = (-90.0, 0.0, crossing);
            camera.distance /= 2.4;
        }
        // The brush ring is where the last dab was; leave it out of the picture.
        self.input.inside = false;
        self.sync_view();
    }
}
