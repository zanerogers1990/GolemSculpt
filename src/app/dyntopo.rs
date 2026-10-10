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

use super::Running;
use crate::doc::Action;
use crate::mesh::primitives::{self, Primitive};
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
        if what == "instance" {
            let original = self.doc.scene.active_id();
            self.act(Action::InstanceObject);
            self.doc.scene.active_mut().position = Vec3::new(0.0, -1.3, 0.0);
            self.act(Action::SelectObject(original));
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
            if doc.sculptor.begin(&mut doc.mesh, &self.local, press, false, false) {
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
        if !doc.sculptor.begin(&mut doc.mesh, &self.local, press, false, false) {
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
