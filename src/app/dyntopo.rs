//! The scripted run that shows dynamic topology at work, for a look at the
//! triangles it leaves: `sculpt.exe --screenshot out.png --demo-dyntopo`.
//!
//! A long shape is pulled out of a coarse ball with Snake Hook, which
//! stretches the triangles along it into slivers (Snake Hook does not refine,
//! as in the Unity app). An Inflate stroke then runs along the shape with
//! dynamic topology on, and the wireframe, which the run switches on, shows
//! what that makes of the stretched triangles. `SCULPT_DEMO_DYNTOPO` picks a
//! variant:
//!
//!   - `off`: the same strokes with dynamic topology left off, to compare.
//!   - `dense`: the same again on a ball that is fine all over to begin with,
//!     which is what the shape looks like when triangles are no object.
//!   - `instance`: an instance of the ball stands beside it from the start,
//!     and shows the same refined mesh.
//!   - `mirror`: the ball stands off the middle with its live mirror copy
//!     beside it, and the refining strokes are drawn on the copy, through the
//!     window's own pointer routines. Both sides have to show the detail:
//!     where the strokes were drawn on the copy, and at the reflected place
//!     on the ball.
//!   - `undo`: the refining stroke is undone again, which has to give back
//!     the stretched shape as it was.
//!   - `live`: the refining stroke is fed to the app a few samples a frame,
//!     as a hand would draw it, instead of all before the first frame. The
//!     picture has to come out the same: this is the one variant in which
//!     the renderer follows the mesh as it grows, rather than being handed
//!     the finished mesh whole.

use std::cell::RefCell;
use std::collections::VecDeque;

use glam::{Vec2, Vec3};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

use super::Running;
use crate::doc::Action;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::MirrorCommand;
use crate::sculpt::{Brush, PointerSample};

/// Triangles of the ball the run starts from: few enough that each one can be
/// seen in the wireframe, and that a brush has detail to add.
const BALL_TRIANGLES: usize = 5_000;
/// The same ball fine all over, for the `dense` variant.
const DENSE_BALL_TRIANGLES: usize = 400_000;
/// Pointer samples in each scripted stroke, and how many of them the `live`
/// variant hands over per frame: the stroke is done well inside the dozen
/// frames a screenshot run draws.
const STROKE_SAMPLES: usize = 96;
const LIVE_SAMPLES_PER_FRAME: usize = 12;

thread_local! {
    /// The samples the `live` variant has still to feed to its stroke.
    static LIVE: RefCell<Option<VecDeque<PointerSample>>> = const { RefCell::new(None) };
}

/// The pointer samples of a straight stroke across the screen.
fn stroke_samples(from: Vec2, to: Vec2) -> impl Iterator<Item = PointerSample> {
    (0..=STROKE_SAMPLES).map(move |i| PointerSample {
        pos: from.lerp(to, i as f32 / STROKE_SAMPLES as f32),
        pressure: None,
        time: i as f64 / 240.0,
    })
}

impl Running {
    pub(super) fn dyntopo_demo(&mut self) {
        let what = std::env::var("SCULPT_DEMO_DYNTOPO").unwrap_or_default();
        let triangles = if what == "dense" { DENSE_BALL_TRIANGLES } else { BALL_TRIANGLES };
        self.replace_mesh(primitives::build(Primitive::Sphere, triangles, 0.5), false);
        self.doc.opts.wireframe = true;
        // `SCULPT_DEMO_BRUSH=radial=6` and the like, for a look at detail
        // being added under every copy of a stroke.
        self.demo_symmetry(None);
        if what == "instance" {
            let original = self.doc.scene.active_id();
            self.act(Action::InstanceObject);
            self.doc.scene.active_mut().position = Vec3::new(0.0, -1.3, 0.0);
            self.act(Action::SelectObject(original));
        }
        if what == "mirror" {
            return self.dyntopo_demo_on_a_copy();
        }
        self.sync_view();

        // The ball's centre and radius on screen: the strokes are laid out
        // against those, so they land the same in a window of any size.
        let Some(centre) = self.local.project(Vec3::ZERO) else { return };
        let ball = 0.5 / self.local.world_per_pixel(self.local.depth_of(Vec3::ZERO).max(1e-6));
        let from = centre + Vec2::new(0.55, -0.1) * ball;
        let to = centre + Vec2::new(2.3, -0.75) * ball;

        self.dyntopo_demo_brush(Brush::SnakeHook, 0.4 * ball, 1.0);
        self.dyntopo_demo_stroke(stroke_samples(from, to));
        self.doc.sculptor.dyntopo.enabled = what != "off" && what != "dense";
        // From just short of the root to just short of the tip, where there
        // is still surface under the pointer.
        self.dyntopo_demo_brush(Brush::Inflate, 0.3 * ball, 0.12);
        let mut along = stroke_samples(from.lerp(to, -0.1), from.lerp(to, 0.93));
        if what == "live" {
            // Only the press now; the frames to come draw the rest.
            let (doc, press) = (&mut self.doc, along.next().expect("a stroke has a first sample"));
            if doc.begin_stroke(&self.local, press, false, false) {
                LIVE.set(Some(along.collect()));
            }
            return;
        }
        self.dyntopo_demo_stroke(along);
        if what == "undo" {
            self.act(Action::Undo);
        }
        self.dyntopo_demo_frame_shape();
    }

    /// The `mirror` variant: the ball to one side, its live mirror copy on
    /// the other, and two refining strokes pressed and dragged on the copy
    /// the way the window would feed them.
    fn dyntopo_demo_on_a_copy(&mut self) {
        self.doc.scene.active_mut().position = Vec3::new(0.7, 0.0, 0.0);
        self.act(Action::Mirror(MirrorCommand::Set(1)));
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, self.view.size.x / self.view.size.y.max(1.0), false);
        self.sync_view();
        self.doc.sculptor.dyntopo.enabled = true;
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();

        // The copy's middle and radius on screen.
        let copy = self.doc.scene.active().placed_matrix(1);
        let on_screen = |p: Vec3| self.view.project(copy.transform_point3(p));
        let (Some(centre), Some(top)) = (on_screen(Vec3::ZERO), on_screen(Vec3::Y * 0.5)) else { return };
        let ball = centre.distance(top);
        let strokes = [
            (Brush::Inflate, 0.12, Vec2::new(-0.5, -0.35), Vec2::new(0.35, -0.45)),
            (Brush::Clay, 0.6, Vec2::new(-0.45, 0.3), Vec2::new(0.4, 0.2)),
        ];
        for (brush, strength, from, to) in strokes {
            self.dyntopo_demo_brush(brush, 0.22 * ball, strength);
            let (from, to) = (centre + from * ball, centre + to * ball);
            self.pointer_moved(from, None);
            self.sync_view();
            self.pointer_pressed(MouseButton::Left, None);
            if !self.doc.sculptor.is_active() {
                eprintln!("demo-dyntopo: the {} stroke began off the copy at {from}", brush.name());
            }
            for i in 1..=STROKE_SAMPLES {
                self.pointer_moved(from.lerp(to, i as f32 / STROKE_SAMPLES as f32), None);
                if i % 4 == 0 {
                    self.dyntopo_demo_tick();
                }
            }
            self.pointer_released(MouseButton::Left);
        }
        // The brush ring would be wherever the real pointer happens to be;
        // leave it out of the picture.
        self.input.inside = false;
        self.sync_view();
    }

    /// What a frame does for a stroke under way, without the drawing: the
    /// pointer samples that have come in go to the brush, and the normals
    /// the next dabs read are brought up to date. A stroke fed all at once
    /// would run into the brushes' ceiling of dabs per frame and stop short.
    fn dyntopo_demo_tick(&mut self) {
        let doc = &mut self.doc;
        for sample in self.input.samples.drain(..) {
            doc.sculptor.sample(&mut doc.mesh, &self.local, sample);
        }
        doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
        doc.mesh.flush_normals();
    }

    /// Once a frame: hands the `live` variant's stroke its next few samples,
    /// and lets go of the brush when they have run out. Does nothing in any
    /// other run.
    pub(super) fn dyntopo_demo_frame(&mut self) {
        let Some(mut samples) = LIVE.take() else { return };
        if samples.is_empty() {
            self.finish_stroke();
            self.dyntopo_demo_frame_shape();
            return;
        }
        let batch = samples.len().min(LIVE_SAMPLES_PER_FRAME);
        self.input.samples.extend(samples.drain(..batch));
        LIVE.set(Some(samples));
    }

    /// Turns the camera to the whole shape, which by now reaches well outside
    /// the view the ball was framed in.
    fn dyntopo_demo_frame_shape(&mut self) {
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, self.view.size.x / self.view.size.y.max(1.0), false);
        self.sync_view();
    }

    fn dyntopo_demo_brush(&mut self, brush: Brush, radius_px: f32, strength: f32) {
        let settings = &mut self.doc.sculptor.settings;
        settings.brush = brush;
        settings.mask_mode = false;
        settings.radius_px = radius_px;
        settings.per_brush[brush.index()].strength = strength;
    }

    /// One scripted stroke of the brush in hand, as an undo step.
    fn dyntopo_demo_stroke(&mut self, mut samples: impl Iterator<Item = PointerSample>) {
        let doc = &mut self.doc;
        let Some(press) = samples.next() else { return };
        if !doc.begin_stroke(&self.local, press, false, false) {
            return;
        }
        for (i, sample) in samples.enumerate() {
            doc.sculptor.sample(&mut doc.mesh, &self.local, sample);
            if i % 4 == 3 {
                doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
                // As drawing a frame does: the next dabs read these normals.
                doc.mesh.flush_normals();
            }
        }
        if let Some(delta) = doc.sculptor.end(&mut doc.mesh, &self.local) {
            doc.history.push_stroke(delta);
        }
    }
}
