//! `--demo-tool lathe[:what]`: the tool in states worth a screenshot, reached through the same functions
//! the gestures call, since no person is at the tablet.

use glam::Vec2;
use sculpt_toolkit::Ctx;

use super::Lathe;
use crate::LathePreset;

/// A goblet, as the (radius, height) corners of one pen stroke, in fractions of the shape's height.
const GOBLET: [(f32, f32); 9] =
    [(0.0, 0.0), (0.26, 0.0), (0.26, 0.04), (0.06, 0.10), (0.05, 0.38), (0.30, 0.56), (0.36, 0.94), (0.30, 1.0), (0.0, 1.0)];

impl Lathe {
    pub(super) fn run_demo(&mut self, cx: &mut Ctx, what: &str) {
        self.plane.face(&cx.view);
        match what {
            "edit" => self.demo_edit(cx),
            "bowl" => self.apply_preset(cx, LathePreset::Bowl),
            "ring" => self.apply_preset(cx, LathePreset::Ring),
            "sketch" => self.demo_sketch(cx),
            "created" => self.demo_created(cx),
            // The same, with Radial Symmetry on Create ticked.
            "radial" => {
                self.set_radial_on_create(true);
                self.demo_created(cx);
            }
            _ => {}
        }
        // So the first frame already has the shape.
        self.refresh_mesh();
    }

    /// The vase's shoulder pulled out and up the way a drag does it, left selected so its handle shows.
    fn demo_edit(&mut self, cx: &mut Ctx) {
        let view = cx.view;
        let index = 4;
        let height = self.profile.height();
        let from = self.profile[index].position;
        let to = from + Vec2::new(0.16, 0.06) * height;
        let (Some(a), Some(b)) = (self.plane.screen(&view, from, 1.0), self.plane.screen(&view, to, 1.0)) else { return };
        self.selected = Some(index);
        self.begin_point_drag(cx.time, &view, index, 1.0, a, "Move Point");
        for step in 1..=6 {
            self.drag_point(&view, a.lerp(b, step as f32 / 6.0), false);
        }
        self.end_drag();
        cx.status("Move Point: the shoulder is pulled out. Ctrl+Z puts it back.", true);
    }

    /// A goblet drawn as one Ctrl-stroke: simplified to a handful of points, ends snapped to the axis.
    fn demo_sketch(&mut self, cx: &mut Ctx) {
        let view = cx.view;
        let height = self.reference / self.plane.scale;
        let mut path: Vec<Vec2> = Vec::new();
        for pair in GOBLET.windows(2) {
            let (from, to) = (Vec2::from(pair[0]) * height, Vec2::from(pair[1]) * height);
            for step in 0..20 {
                path.push(from.lerp(to, step as f32 / 20.0));
            }
        }
        path.push(Vec2::from(GOBLET[GOBLET.len() - 1]) * height);
        let on_screen: Vec<Vec2> = path.iter().filter_map(|&p| self.plane.screen(&view, p, 1.0)).collect();
        let Some(&first) = on_screen.first() else { return };
        self.begin_sketch(&view, first);
        for &px in &on_screen {
            self.drag_sketch(&view, px);
        }
        self.release(cx);
    }

    /// Create done: the object standing beside the model, the tool put away.
    fn demo_created(&mut self, cx: &mut Ctx) {
        self.create(cx);
        if let Some(job) = self.job.take() {
            let result = job.wait();
            self.land_result(cx, result);
        }
    }
}
