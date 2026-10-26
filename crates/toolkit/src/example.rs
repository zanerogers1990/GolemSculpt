//! A complete small tool, to read and to copy from: Studs.
//!
//! Click the model to stand a stud on it; Make Studs turns them into one new
//! object. It is short, but it does one of everything a tool does: takes
//! presses and passes on the ones that are not its own, keeps its own
//! history for Z, shows a ghost under the pointer and its studs as instances
//! of one preview mesh, puts the transform gizmo on the stud placed last,
//! builds its result on a background job with progress, commits it as one
//! undo step, says what closing the app would lose, and draws its panel with
//! the theme.
//!
//! The app does not list it, but `--demo-tool studs` brings it up.

use std::time::Duration;

use egui::Color32;
use glam::{Mat4, Quat, Vec3};
use sculpt_geom::TriMesh;

use crate::theme::{self, ACCENT, ButtonStyle};
use crate::{
    Button, Command, Ctx, Draw, Event, GizmoTarget, Info, Job, Key, Loss, MeshData, NewObject, Placement, Preview,
    RadialItem, Style, Tool, testing,
};

const STUD: Color32 = Color32::from_rgb(214, 130, 92);
const GHOST: Color32 = Color32::from_rgba_premultiplied(120, 150, 190, 150);

pub struct Studs {
    /// Where each stud stands: a unit cube's own space into world space.
    studs: Vec<Mat4>,
    /// Studs taken back with Ctrl+Z, for Ctrl+Shift+Z.
    undone: Vec<Mat4>,
    size: f32,
    /// The one mesh every stud and the ghost are drawn from.
    cube: Preview,
    ghost: Option<Mat4>,
    /// The build in progress, if Make Studs was pressed.
    job: Option<Job<MeshData>>,
}

impl Default for Studs {
    fn default() -> Studs {
        Studs {
            studs: Vec::new(),
            undone: Vec::new(),
            size: 0.08,
            cube: Preview::from_mesh(testing::cube(0.5)),
            ghost: None,
            job: None,
        }
    }
}

/// The tool, ready to register.
pub fn tool() -> Box<dyn Tool> {
    Box::new(Studs::default())
}

impl Studs {
    pub fn count(&self) -> usize {
        self.studs.len()
    }

    /// A stud standing on the surface under a pixel, if there is one.
    fn stud_at(&self, cx: &Ctx, px: glam::Vec2) -> Option<Mat4> {
        let hit = cx.pick(px)?;
        let upright = Quat::from_rotation_arc(Vec3::Y, hit.normal);
        Some(Mat4::from_scale_rotation_translation(Vec3::splat(self.size), upright, hit.position + hit.normal * self.size * 0.5))
    }

    fn place(&mut self, cx: &Ctx, px: glam::Vec2) -> bool {
        let Some(stud) = self.stud_at(cx, px) else { return false };
        self.studs.push(stud);
        self.undone.clear();
        true
    }

    /// Starts building the studs into one mesh, off the interface thread.
    /// `pause` slows each stud down, so the demo has progress to show.
    fn make(&mut self, cx: &mut Ctx, pause: Duration) {
        if self.studs.is_empty() || self.job.is_some() {
            return;
        }
        // The job cannot borrow from the tool or the scene: it gets copies.
        let studs = self.studs.clone();
        self.job = Some(cx.spawn("Make Studs", move |job| {
            let mut mesh = TriMesh::default();
            for (i, stud) in studs.iter().enumerate() {
                if job.cancelled() {
                    return Err("Cancelled.".into());
                }
                job.progress(format!("{} of {}", i + 1, studs.len()));
                let mut cube = testing::cube(0.5);
                cube.transform(stud);
                mesh.append(&cube);
                std::thread::sleep(pause);
            }
            // Baked here, so the commit on the interface thread is instant.
            Ok(job.bake(mesh, None))
        }));
    }

    /// The finished mesh becomes an object: one undo step.
    fn land(&mut self, cx: &mut Ctx, result: Result<MeshData, String>) {
        match result {
            Ok(mesh) => {
                let count = self.studs.len();
                cx.commit("Make Studs", vec![Command::Add(NewObject::new("Studs", mesh))]);
                cx.status(format!("Made \"Studs\" from {count} studs. Undo (Ctrl+Z) removes it."), true);
                self.studs.clear();
                self.undone.clear();
                cx.finish();
            }
            Err(e) => cx.status(e, false),
        }
    }
}

impl Tool for Studs {
    fn info(&self) -> Info {
        Info {
            id: "studs",
            name: "Studs",
            icon: "prim_cube",
            tip: "Click the model to stand studs on it, then make them one object.",
            hotkey: None,
        }
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        self.ghost = None;
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            // On the model: a stud. Off it: not ours, so the camera orbits.
            Event::Press(Button::Left) => self.place(cx, cx.pointer),
            Event::Wheel(lines) if cx.pick(cx.pointer).is_some() => {
                self.size = (self.size * (1.0 + 0.1 * lines.signum())).clamp(0.01, 0.5);
                true
            }
            Event::KeyDown(Key::Enter) => {
                self.make(cx, Duration::ZERO);
                true
            }
            Event::Undo => match self.studs.pop() {
                Some(stud) => {
                    self.undone.push(stud);
                    true
                }
                None => false,
            },
            Event::Redo => match self.undone.pop() {
                Some(stud) => {
                    self.studs.push(stud);
                    true
                }
                None => false,
            },
            Event::Radial(0) => {
                self.make(cx, Duration::ZERO);
                true
            }
            Event::Radial(1) => {
                self.studs.clear();
                true
            }
            // A handle of the gizmo on the last stud is being dragged: the
            // stud goes where the gizmo says. It is the same stud, so it
            // stays one step of this tool's history: Ctrl+Z takes it back whole.
            Event::Gizmo(placement, _) => {
                if let Some(stud) = self.studs.last_mut() {
                    *stud = placement.matrix();
                }
                true
            }
            _ => false,
        }
    }

    /// Studs placed and not yet made into an object are in no file, so the
    /// app asks before closing over them. They stand in the world, not on an
    /// object, so a scene opened in place of this one leaves them be.
    fn scaffold_at_risk(&self, loss: Loss) -> Option<String> {
        match (loss, self.studs.len()) {
            (Loss::SceneReplaced, _) | (_, 0) => None,
            (Loss::Closing, 1) => Some("1 stud (Make Studs first)".to_owned()),
            (Loss::Closing, count) => Some(format!("{count} studs (Make Studs first)")),
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // A job started here keeps running if the user switches tool, so
        // its result is collected whether or not this tool is up.
        if let Some(result) = self.job.as_mut().and_then(Job::poll) {
            self.job = None;
            self.land(cx, result);
        }
        if !cx.active {
            return;
        }
        self.ghost = if cx.hovering { self.stud_at(cx, cx.pointer) } else { None };
        draw.instances(&self.cube, Style::solid(STUD).flat(), &self.studs);
        if let Some(ghost) = self.ghost {
            draw.instances(&self.cube, Style::shell(GHOST).flat(), &[ghost]);
        }
        if let Some(px) = self.studs.last().and_then(|m| cx.view.project(m.w_axis.truncate())) {
            draw.overlay.text(px - glam::Vec2::new(0.0, 28.0 * cx.view.pixels_per_point), format!("{} studs", self.studs.len()));
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        let summary = theme::count(self.studs.len());
        theme::category(ui, "Studs", STUD, &summary, true, |ui| {
            theme::hint(ui, "Click the model to stand a stud on it. Scroll over the model to size the next one.");
            theme::slider(ui, "Stud Size", &mut self.size, 0.01..=0.5, |v| format!("{v:.2}"));
            match &self.job {
                Some(job) => theme::line(ui, &format!("Making: {}", job.progress()), theme::OK),
                None => theme::line(ui, &format!("{} studs.", self.studs.len()), theme::HINT()),
            }
            ui.add_enabled_ui(!self.studs.is_empty() && self.job.is_none(), |ui| {
                let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                let r = theme::button(ui, ui.available_width(), "Make Studs", style);
                if theme::tip(r, cx.tips, "Turns the studs into one new object you can sculpt.").clicked() {
                    self.make(cx, Duration::ZERO);
                }
            });
        });
    }

    fn radial(&self) -> Vec<RadialItem> {
        vec![
            RadialItem { label: "Make", icon: "check", on: false },
            RadialItem { label: "Clear", icon: "trash", on: false },
        ]
    }

    fn can_undo(&self, redo: bool) -> bool {
        if redo { !self.undone.is_empty() } else { !self.studs.is_empty() }
    }

    /// The stud placed last can be moved, turned and resized by hand before
    /// the studs are made into an object.
    fn gizmo(&self) -> Option<GizmoTarget> {
        let (scale, rotation, position) = self.studs.last()?.to_scale_rotation_translation();
        // Half the cube's diagonal: the handles stand clear of its corners.
        Some(GizmoTarget::body(Placement { position, rotation, scale }, scale.length() * 0.5))
    }

    /// A ring of studs on the selected object. `made` goes on to make them
    /// an object; `job` leaves a slow build running, to show its progress.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        let bounds = cx.scene().bounds(cx.scene().active());
        let (centre, reach) = (bounds.center(), bounds.size().max_element() * 0.5);
        for i in 0..10 {
            let a = i as f32 / 10.0 * std::f32::consts::TAU;
            let toward = centre + (cx.view.right * a.cos() + cx.view.up * a.sin()) * reach * 0.6;
            if let Some(px) = cx.view.project(toward) {
                self.place(cx, px);
            }
        }
        match what {
            "made" => {
                self.make(cx, Duration::ZERO);
                if let Some(job) = self.job.take() {
                    let result = job.wait();
                    self.land(cx, result);
                }
            }
            "job" => self.make(cx, Duration::from_millis(400)),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec2;

    use super::*;
    use crate::Scene;
    use crate::testing::Bench;

    fn bench() -> (Bench, Studs) {
        let mut bench = Bench::new();
        bench.add("Cube", testing::cube(0.5));
        let mut tool = Studs::default();
        bench.enter(&mut tool);
        (bench, tool)
    }

    #[test]
    fn a_click_on_the_model_places_a_stud_and_z_takes_it_back() {
        let (mut bench, mut tool) = bench();
        let on_model = bench.px(Vec3::new(0.1, 0.1, 0.5));
        bench.click(&mut tool, on_model, Button::Left);
        assert_eq!(tool.count(), 1);
        // The stud sits on the face, half its height out.
        let draw = bench.frame(&mut tool);
        let placed = draw.previews()[0].transforms[0].w_axis.truncate();
        assert!(placed.distance(Vec3::new(0.1, 0.1, 0.54)) < 1e-3, "{placed:?}");

        assert!(tool.can_undo(false) && !tool.can_undo(true));
        assert!(bench.undo(&mut tool));
        assert_eq!(tool.count(), 0);
        assert!(bench.redo(&mut tool));
        assert_eq!(tool.count(), 1);
    }

    #[test]
    fn the_gizmo_sits_on_the_last_stud_and_moves_it() {
        let (mut bench, mut tool) = bench();
        assert!(tool.gizmo().is_none(), "nothing to put it on yet");
        bench.click(&mut tool, bench.px(Vec3::new(0.1, 0.1, 0.5)), Button::Left);
        let target = tool.gizmo().expect("the stud just placed");
        assert!(target.placement.position.distance(Vec3::new(0.1, 0.1, 0.54)) < 1e-3);
        assert!(target.moves && target.turns && target.scales);

        let to = Placement { position: Vec3::new(0.3, 0.2, 0.6), scale: Vec3::splat(0.2), ..target.placement };
        assert!(bench.gizmo(&mut tool, to));
        let draw = bench.frame(&mut tool);
        let stud = draw.previews()[0].transforms[0];
        assert!(stud.w_axis.truncate().distance(to.position) < 1e-5);
        assert!((stud.x_axis.truncate().length() - 0.2).abs() < 1e-5, "and resizes it");
        // It is still the one stud, and one Z still takes it back.
        assert_eq!(tool.count(), 1);
        assert!(bench.undo(&mut tool));
        assert!(tool.gizmo().is_none());
    }

    #[test]
    fn a_press_off_the_model_is_passed_on() {
        let (mut bench, mut tool) = bench();
        assert!(!bench.press(&mut tool, Vec2::new(5.0, 5.0), Button::Left));
        bench.release(&mut tool);
        assert_eq!(tool.count(), 0);
    }

    #[test]
    fn the_ghost_shows_only_under_a_free_pointer() {
        let (mut bench, mut tool) = bench();
        bench.hover(bench.px(Vec3::new(0.0, 0.0, 0.5)));
        assert_eq!(bench.frame(&mut tool).previews().len(), 1);
        bench.env.hovering = false;
        assert!(bench.frame(&mut tool).previews().is_empty());
    }

    #[test]
    fn making_adds_one_object_in_one_undo_step_and_puts_the_tool_away() {
        let (mut bench, mut tool) = bench();
        bench.call(|cx| tool.demo(cx, "made"));
        assert_eq!(bench.doc.objects().len(), 2);
        let made = bench.doc.active_info().unwrap();
        assert_eq!(made.name, "Studs");
        assert_eq!(made.triangles % 12, 0);
        assert!(made.triangles > 0 && bench.finished);
        // With the tool's own history empty, Ctrl+Z reaches the scene's.
        assert!(bench.undo(&mut tool));
        assert_eq!(bench.doc.objects().len(), 1);
    }

    #[test]
    fn studs_not_yet_made_are_what_closing_would_lose() {
        let (mut bench, mut tool) = bench();
        assert_eq!(tool.scaffold_at_risk(Loss::Closing), None, "nothing placed, nothing to lose");
        bench.click(&mut tool, bench.px(Vec3::new(0.1, 0.1, 0.5)), Button::Left);
        assert_eq!(tool.scaffold_at_risk(Loss::Closing), Some("1 stud (Make Studs first)".to_owned()));
        bench.click(&mut tool, bench.px(Vec3::new(-0.1, 0.1, 0.5)), Button::Left);
        assert_eq!(tool.scaffold_at_risk(Loss::Closing), Some("2 studs (Make Studs first)".to_owned()));
        assert_eq!(tool.scaffold_at_risk(Loss::SceneReplaced), None, "they stand in the world: a new scene keeps them");
        // Ctrl+Z takes one back; made into an object, they are the scene's and nothing is at risk.
        assert!(bench.undo(&mut tool));
        assert_eq!(tool.scaffold_at_risk(Loss::Closing), Some("1 stud (Make Studs first)".to_owned()));
        assert!(bench.key(&mut tool, Key::Enter));
        let start = std::time::Instant::now();
        while bench.doc.objects().len() < 2 {
            assert!(start.elapsed() < Duration::from_secs(10), "the job never landed");
            bench.frame(&mut tool);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(tool.scaffold_at_risk(Loss::Closing), None);
    }

    #[test]
    fn a_job_left_running_lands_on_a_later_frame() {
        let (mut bench, mut tool) = bench();
        bench.click(&mut tool, bench.px(Vec3::new(0.0, 0.0, 0.5)), Button::Left);
        assert!(bench.key(&mut tool, Key::Enter));
        let start = std::time::Instant::now();
        while bench.doc.objects().len() < 2 {
            assert!(start.elapsed() < Duration::from_secs(10), "the job never landed");
            bench.frame(&mut tool);
            std::thread::sleep(Duration::from_millis(1));
        }
        assert_eq!(bench.status.as_ref().map(|s| s.1), Some(true));
    }
}
