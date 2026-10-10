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

use glam::{Quat, Vec3};

use super::Running;
use crate::camera::View;
use crate::doc::{Action, Doc};
use crate::mesh::primitives::Primitive;
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
    /// on the command line), for a look at that object's own symmetry.
    pub(super) fn symmetry_demo(&mut self) {
        let what = std::env::var("SCULPT_DEMO_SYMMETRY").unwrap_or_default();
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

    /// One scripted Clay stroke on the active object between two places in
    /// the world (each is pulled onto the object's surface along the view).
    fn symmetry_demo_stroke(&mut self, from: Vec3, to: Vec3) {
        self.sync_view();
        let settings = &mut self.doc.sculptor.settings;
        (settings.brush, settings.mask_mode) = (Brush::Clay, false);
        settings.radius_px = 0.03 * self.view.size.y;
        settings.per_brush[Brush::Clay.index()].strength = 0.6;
        let on_screen = |at: Vec3| self.view.project(at);
        let (Some(a), Some(b)) = (on_screen(from), on_screen(to)) else { return };
        let steps = 40;
        let sample = |i: usize| PointerSample {
            pos: a.lerp(b, i as f32 / steps as f32),
            pressure: None,
            time: i as f64 / 240.0,
        };
        let doc = &mut self.doc;
        if !doc.begin_stroke(&self.local, sample(0), false, false) {
            eprintln!("demo-symmetry: the stroke from {from} missed \"{}\"", doc.scene.active().name);
            return;
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
    }
}
