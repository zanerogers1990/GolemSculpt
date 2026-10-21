//! `--demo-feel <brush>[:rough|:fin|:back]`: a picture of how a brush
//! feels, for comparing two builds by eye (the numbers are `--brush-feel`).
//!
//! A ball seen face on and four strokes across it, one under another: the
//! brush at 10%, 50% and 100% Strength at full pen pressure, and at its own
//! Strength with the pen pressing in and lifting off again (a taper from no
//! pressure to full and back). `<brush>` is any brush of the library by its
//! name (a base brush, or one that ships, like Clay Strips). `:rough` lays a
//! fine ripple over the ball first, for the brushes that smooth; `:fin` is
//! a slab thinner than the brush instead of the ball, and `:back` the same
//! slab seen from behind once the strokes are made on its front.

use glam::{Vec2, Vec3};

use super::Running;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::PointerSample;

impl Running {
    pub(super) fn feel_demo(&mut self, spec: &str) {
        let (name, what) = spec.split_once(':').unwrap_or((spec, ""));
        let slab = what == "fin" || what == "back";
        let mesh = if slab {
            let cube = primitives::build(Primitive::Cube, 400_000, 0.5);
            let thin = cube.positions.iter().map(|p| Vec3::new(p.x, p.y, p.z * 0.015)).collect();
            Mesh::new(thin, cube.indices.clone(), None)
        } else if what == "rough" {
            let ball = primitives::build(Primitive::Sphere, 400_000, 0.5);
            let bumpy = ball.positions.iter().map(|&p| p * (1.0 + 0.004 * (p.x * 260.0).sin() * (p.y * 260.0).sin())).collect();
            Mesh::new(bumpy, ball.indices.clone(), None)
        } else {
            primitives::build(Primitive::Sphere, 400_000, 0.5)
        };
        self.replace_mesh(mesh, false);
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 0.0);
        // Close enough that the strokes are worth looking at.
        self.doc.camera.distance /= 1.8;
        self.sync_view();

        let sculptor = &mut self.doc.sculptor;
        if !sculptor.library.select_named(name, &mut sculptor.settings) {
            eprintln!("demo-feel: there is no brush called {name:?}");
            return;
        }
        let radius = 0.03 * self.view.size.y;
        let settings = &mut sculptor.settings;
        (settings.mask_mode, settings.screen_space_size, settings.radius_px) = (false, true, radius);
        let own = settings.current().strength;
        let middle = self.doc.scene.active().matrix().transform_point3(Vec3::Z * if slab { 0.0075 } else { 0.5 });
        let Some(middle) = self.view.project(middle) else { return };
        let rows: [(f32, bool); 4] = [(0.1, false), (0.5, false), (1.0, false), (own, true)];
        for (row, (strength, taper)) in rows.into_iter().enumerate() {
            self.doc.sculptor.settings.current_mut().strength = strength;
            let y = middle.y + (row as f32 - 1.5) * 3.0 * radius;
            let (from, to) = (Vec2::new(middle.x - 4.0 * radius, y), Vec2::new(middle.x + 4.0 * radius, y));
            if !self.feel_stroke(from, to, taper) {
                eprintln!("demo-feel: the stroke at {strength} missed the model");
            }
        }
        self.doc.sculptor.settings.current_mut().strength = own;
        if what == "back" {
            self.doc.camera.yaw = 180.0;
        }
        self.input.inside = false;
        self.sync_view();
        println!("demo-feel: {name}, Strength 10% / 50% / 100% at full pressure, then {own:.2} tapering with the pen");
    }

    /// One stroke from `a` to `b` on the screen, with the pen: at full
    /// pressure, or with `taper` rising from none to full and back.
    fn feel_stroke(&mut self, a: Vec2, b: Vec2, taper: bool) -> bool {
        let steps = 96;
        let sample = |i: usize| {
            let t = i as f32 / steps as f32;
            let pressure = if taper { (t * std::f32::consts::PI).sin() } else { 1.0 };
            PointerSample { pos: a.lerp(b, t), pressure: Some(pressure), time: i as f64 / 240.0 }
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
}
