//! The four region tools (Mask, Hide, Trim, Split): one gesture, pointed at
//! four different things.
//!
//! In the Unity app these were modes armed from a wheel inside sculpting;
//! while one was armed, every drag in the viewport drew a region and the
//! camera was on Alt. Here bringing the tool up is the arming, and the same
//! rule holds while it is up: a press anywhere on the viewport starts a
//! region, because a box or a cut line has to be able to start on empty
//! background beside the model. The press is passed on (so the camera
//! orbits, as everywhere else in the app) only when no region could come of
//! it: a cut is still being worked out, or the selected object is hidden.

use egui::Color32;
use glam::Vec2;
use sculpt_cut::region::{CLICK_SLOP_PIXELS, LASSO_POINT_SPACING_PIXELS};
use sculpt_cut::ScreenRegion;
use sculpt_toolkit::{Button, Ctx, Draw, Event, Info, Job, Key, Mods, RadialItem, Tool, View};

use crate::action::{Action, REMOVE};
use crate::cut::{self, Cut, Landed, Outcome, Snapshot};
use crate::gesture::{Drag, Marquee, Shape, mixed};
use crate::{hide, mask, panel};

/// Above this many triangles a cut runs as a background job. The geometry
/// takes about 50 ms per million triangles and building the sculpting
/// structures several times that; below this neither is long enough to see.
const JOB_TRIANGLES: usize = 100_000;

/// The pointer while the tool is armed, in points: four arms round a gap, so
/// the exact pixel a shape will start from stays visible.
const CROSSHAIR_GAP: f32 = 5.0;
const CROSSHAIR_ARM: f32 = 9.0;

/// One of the region tools.
pub struct Region {
    info: Info,
    pub(crate) action: Action,
    pub(crate) shape: Shape,
    /// The region being dragged out.
    pub(crate) drag: Option<Drag>,
    /// A cut being worked out off the interface thread.
    pub(crate) job: Option<Job<Outcome>>,
    /// Per-vertex "the shape covers this", kept across gestures.
    covered: Vec<bool>,
    /// The last result, for the panel.
    pub(crate) last: Option<(String, bool)>,
    /// The object a split last made.
    pub(crate) last_piece: Option<sculpt_toolkit::ObjectId>,
    pub(crate) job_above: usize,
}

impl Region {
    fn new(info: Info, action: Action) -> Region {
        Region {
            info,
            action,
            shape: action.shapes()[0],
            drag: None,
            job: None,
            covered: Vec::new(),
            last: None,
            last_piece: None,
            job_above: JOB_TRIANGLES,
        }
    }

    /// Box and lasso mask.
    pub fn mask() -> Region {
        let info = Info {
            id: "mask",
            name: "Mask",
            icon: "region_box_mask",
            tip: "Drag a box or draw a lasso over the model to mask everything it covers, front and back (N).",
            hotkey: Some(Key::N),
        };
        Region::new(info, Action::Mask)
    }

    /// Box and lasso hide. No key: H is the app's own, for whole objects.
    pub fn hide() -> Region {
        let info = Info {
            id: "hide",
            name: "Hide",
            icon: "region_box_hide",
            tip: "Hide part of the selected object to get at what is behind it: drag a box or draw a lasso over it. A click shows everything again.",
            hotkey: None,
        };
        Region::new(info, Action::Hide)
    }

    /// Box, lasso and straight-line trim.
    pub fn trim() -> Region {
        let info = Info {
            id: "trim",
            name: "Trim",
            icon: "region_box_trim",
            tip: "Cut away what a box, lasso or straight line covers, and close the hole it leaves.",
            hotkey: None,
        };
        Region::new(info, Action::Trim)
    }

    /// Straight-line, lasso and box split.
    pub fn split() -> Region {
        let info = Info {
            id: "split",
            name: "Split",
            icon: "region_line_split",
            tip: "Cut the model in two along a line, lasso or box: both pieces are kept, the covered one as a new object (K).",
            hotkey: Some(Key::K),
        };
        Region::new(info, Action::Split)
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn shape(&self) -> Shape {
        self.shape
    }

    pub fn is_dragging(&self) -> bool {
        self.drag.is_some()
    }

    /// Whether a cut is still being worked out.
    pub fn is_busy(&self) -> bool {
        self.job.is_some()
    }

    pub(crate) fn name(&self) -> &'static str {
        self.info.name
    }

    pub(crate) fn hotkey(&self) -> Option<Key> {
        self.info.hotkey
    }

    pub(crate) fn set_shape(&mut self, cx: &mut Ctx, shape: Shape) {
        if !self.action.shapes().contains(&shape) {
            return;
        }
        self.shape = shape;
        self.drag = None;
        cx.toast(format!("{} {}", shape.name(), self.action.name()));
        cx.status(self.action.prompt(shape), true);
    }

    /// The tool's own key steps through its shapes. Past the last it is not
    /// taken, so the app puts the tool away: Box, Lasso, off, as in the Unity
    /// app.
    fn next_shape(&mut self, cx: &mut Ctx) -> bool {
        let shapes = self.action.shapes();
        let at = shapes.iter().position(|&s| s == self.shape).unwrap_or(0);
        match shapes.get(at + 1) {
            Some(&next) => {
                self.set_shape(cx, next);
                true
            }
            None => {
                self.shape = shapes[0];
                false
            }
        }
    }

    fn say(&mut self, cx: &mut Ctx, text: impl Into<String>, ok: bool) {
        let text = text.into();
        cx.status(text.clone(), ok);
        self.last = Some((text, ok));
    }

    pub(crate) fn report(&mut self, cx: &mut Ctx, result: Result<String, String>) {
        match result {
            Ok(text) => self.say(cx, text, true),
            Err(text) => self.say(cx, text, false),
        }
    }

    /// Why a press cannot start a region now, when it cannot: the press is
    /// then passed on and the camera orbits. An empty reason is not worth a
    /// line of status.
    pub(crate) fn refusal(&self, cx: &Ctx) -> Option<&'static str> {
        if cx.clean_view {
            return Some("");
        }
        if self.job.is_some() {
            return Some("Still working on the last cut.");
        }
        match cx.scene().active_info() {
            None => Some("No object selected."),
            Some(info) if !info.visible => Some("The selected object is hidden."),
            Some(info) if info.triangles == 0 => Some("No geometry."),
            Some(_) => None,
        }
    }

    /// A button went down at `at`. True when a region was started.
    pub(crate) fn begin(&mut self, cx: &mut Ctx, at: Vec2, inverted: bool) -> bool {
        if let Some(why) = self.refusal(cx) {
            if !why.is_empty() {
                cx.status(why, false);
            }
            return false;
        }
        self.drag = Some(Drag::begin(self.shape, on_screen(&cx.view, at), inverted));
        true
    }

    pub(crate) fn drag_to(&mut self, cx: &Ctx, at: Vec2) {
        let spacing = LASSO_POINT_SPACING_PIXELS * cx.view.pixels_per_point;
        if let Some(drag) = &mut self.drag {
            drag.move_to(on_screen(&cx.view, at), spacing);
        }
    }

    /// The button came up: a click resets, a drag acts on what it covers as
    /// seen through `view`.
    pub(crate) fn finish(&mut self, cx: &mut Ctx, view: &View) {
        // None when Esc cancelled the drag while the button was still down.
        let Some(drag) = self.drag.take() else { return };
        if drag.is_click(CLICK_SLOP_PIXELS * cx.view.pixels_per_point) {
            return self.click(cx);
        }
        let line = drag.shape == Shape::Line;
        match drag.region(cx.mods.shift) {
            // Shift turns a line rather than choosing the outside: a line
            // has no inside, only two sides, which Ctrl already swaps.
            Ok(region) => self.apply(cx, view, region, drag.inverted || cx.mods.ctrl, cx.mods.shift && !line),
            Err(why) => self.say(cx, why, false),
        }
    }

    /// A click without a drag resets what the mode edits. Trim and Split
    /// have nothing to reset (undo is the way back from a cut), so a stray
    /// click there only says what to do.
    fn click(&mut self, cx: &mut Ctx) {
        match self.action {
            Action::Mask => {
                let result = mask::clear(cx);
                self.report(cx, result);
            }
            Action::Hide => {
                let result = hide::show_all(cx);
                self.report(cx, result);
            }
            Action::Trim | Action::Split => self.say(cx, self.action.click_hint(self.shape), true),
        }
    }

    /// Does the action to what `region` covers. `inverse` (Ctrl, or a drag
    /// with the right button) reverses it; `outside` (Shift) acts on
    /// everything the shape does not cover.
    pub(crate) fn apply(&mut self, cx: &mut Ctx, view: &View, region: ScreenRegion, inverse: bool, outside: bool) {
        match self.action {
            Action::Mask => {
                let value = if inverse { 0.0 } else { 1.0 };
                let result = mask::apply(cx, view, &region, value, outside, &mut self.covered);
                self.report(cx, result);
            }
            Action::Hide => {
                let result = hide::apply(cx, view, &region, outside, !inverse, &mut self.covered);
                self.report(cx, result);
            }
            // A cut has only two sides, so both modifiers mean the same
            // thing: keep the covered part instead of cutting it away.
            Action::Trim => self.cut(cx, view, region, Cut::Trim { remove_covered: !(inverse || outside) }),
            // A split keeps both sides, so neither modifier has one to pick.
            Action::Split => self.cut(cx, view, region, Cut::Split),
        }
    }

    fn cut(&mut self, cx: &mut Ctx, view: &View, region: ScreenRegion, cut: Cut) {
        let snapshot = match Snapshot::take(cx, view) {
            Ok(snapshot) => snapshot,
            Err(why) => return self.say(cx, why, false),
        };
        if snapshot.triangles() <= self.job_above {
            let result = cut::run(&snapshot, &region, cut).map(|pieces| pieces.bake(&snapshot, cut::plain));
            return self.landed(cx, result);
        }
        self.job = Some(cx.spawn(cut.label(), move |job| {
            job.progress("Cutting");
            let pieces = cut::run(&snapshot, &region, cut)?;
            if job.cancelled() {
                return Err("Cancelled.".to_owned());
            }
            job.progress("Building");
            Ok(pieces.bake(&snapshot, |mesh, mask| job.bake(mesh, mask)))
        }));
    }

    fn landed(&mut self, cx: &mut Ctx, result: Result<Outcome, String>) {
        match result {
            Ok(outcome) => {
                let Landed { message, ok, piece } = cut::land(cx, outcome);
                self.last_piece = piece;
                self.say(cx, message, ok);
            }
            Err(why) => {
                if self.action == Action::Split {
                    cx.toast("Nothing split");
                }
                self.say(cx, why, false);
            }
        }
    }

    /// Waits for a cut in flight and lands it: for `demo` and tests, where
    /// there is no later frame to wait for.
    pub(crate) fn settle(&mut self, cx: &mut Ctx) {
        if let Some(job) = self.job.take() {
            let result = job.wait();
            self.landed(cx, result);
        }
    }

    /// Esc: abandons the drag in progress.
    fn cancel(&mut self, cx: &mut Ctx) -> bool {
        if self.drag.take().is_none() {
            return false;
        }
        self.say(cx, "Cancelled.", true);
        true
    }

    /// The marquee's colour: the mode's own, the erasing red while a mask or
    /// hide drag would take away, washed toward white while Shift has it
    /// acting on everything outside. Trim stays its own red whichever way
    /// the modifiers point: both directions delete, only the side changes.
    fn tint(&self, drag: &Drag, mods: Mods) -> Color32 {
        let removes = drag.inverted || mods.ctrl;
        let base = if removes && !self.action.cuts() { REMOVE } else { self.action.tint() };
        if mods.shift && drag.shape != Shape::Line { mixed(base, Color32::WHITE, 0.4) } else { base }
    }

    fn draw_crosshair(&self, cx: &Ctx, draw: &mut Draw) {
        let ppp = cx.view.pixels_per_point;
        let tint = self.action.tint();
        for arm in [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y] {
            let from = cx.pointer + arm * CROSSHAIR_GAP * ppp;
            draw.overlay.line(from, from + arm * CROSSHAIR_ARM * ppp, 1.5, tint);
        }
    }

    /// The tiles of the hold-Space wheel: the shapes, then for the mask and
    /// for hiding their two whole-object actions.
    fn wheel(&self) -> Vec<(RadialItem, Pick)> {
        let mut tiles: Vec<(RadialItem, Pick)> = self
            .action
            .shapes()
            .iter()
            .map(|&shape| {
                let tile = RadialItem { label: shape.name(), icon: self.action.icon(shape), on: shape == self.shape };
                (tile, Pick::Shape(shape))
            })
            .collect();
        if self.action == Action::Mask {
            tiles.push((RadialItem { label: "Clear", icon: "close", on: false }, Pick::ClearMask));
            tiles.push((RadialItem { label: "Invert", icon: "region_invert", on: false }, Pick::InvertMask));
        }
        if self.action == Action::Hide {
            tiles.push((RadialItem { label: "Show All", icon: "region_show_all", on: false }, Pick::ShowAll));
            tiles.push((RadialItem { label: "Invert", icon: "region_invert", on: false }, Pick::InvertVisible));
        }
        tiles
    }

    fn pick(&mut self, cx: &mut Ctx, pick: Pick) {
        match pick {
            Pick::Shape(shape) => self.set_shape(cx, shape),
            Pick::ClearMask => {
                let result = mask::clear(cx);
                self.report(cx, result);
            }
            Pick::InvertMask => {
                let result = mask::invert(cx);
                self.report(cx, result);
            }
            Pick::ShowAll => {
                let result = hide::show_all(cx);
                self.report(cx, result);
            }
            Pick::InvertVisible => {
                let result = hide::invert(cx);
                self.report(cx, result);
            }
        }
    }
}

/// What a wheel tile does.
#[derive(Clone, Copy)]
enum Pick {
    Shape(Shape),
    ClearMask,
    InvertMask,
    ShowAll,
    InvertVisible,
}

/// A pointer position kept inside the viewport. The pointer can run outside
/// the window while a button is held, and a lasso point out there would
/// stretch the region's grid far past the screen for nothing.
fn on_screen(view: &View, at: Vec2) -> Vec2 {
    at.clamp(Vec2::ZERO, view.size)
}

fn is_brush_key(key: Key) -> bool {
    use Key::*;
    matches!(key, Num0 | Num1 | Num2 | Num3 | Num4 | Num5 | Num6 | Num7 | Num8 | Num9)
}

impl Tool for Region {
    fn info(&self) -> Info {
        self.info
    }

    fn enter(&mut self, cx: &mut Ctx) {
        self.drag = None;
        // The shape is part of what is armed, so the toast names it too.
        cx.toast(format!("{} {}", self.shape.name(), self.action.name()));
        cx.status(self.action.prompt(self.shape), true);
    }

    fn leave(&mut self, _cx: &mut Ctx) {
        self.drag = None;
    }

    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::Press(button) => self.begin(cx, cx.pointer, button == Button::Right),
            Event::Drag => {
                self.drag_to(cx, cx.pointer);
                true
            }
            Event::Release => {
                let view = cx.view;
                self.finish(cx, &view);
                true
            }
            Event::KeyDown(Key::Escape) => self.cancel(cx),
            Event::KeyDown(key) if Some(key) == self.info.hotkey => self.next_shape(cx),
            // Any brush key leaves, so 1-9 is always the way straight back to
            // sculpting whichever region tool is up.
            Event::KeyDown(key) if is_brush_key(key) => {
                cx.finish();
                true
            }
            Event::Radial(index) => {
                if let Some(&(_, pick)) = self.wheel().get(index) {
                    self.pick(cx, pick);
                }
                true
            }
            _ => false,
        }
    }

    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        // A cut started here keeps running if the user moves on, so its
        // result is collected whether or not this tool is up.
        if let Some(result) = self.job.as_mut().and_then(Job::poll) {
            self.job = None;
            self.landed(cx, result);
        }
        if !cx.active {
            return;
        }
        // Nothing is dragged over a clean presentation view.
        if cx.clean_view {
            self.drag = None;
            return;
        }
        match &self.drag {
            Some(drag) => {
                let removes = drag.inverted || cx.mods.ctrl;
                let look = Marquee {
                    tint: self.tint(drag, cx.mods),
                    viewport: cx.view.size,
                    pixels_per_point: cx.view.pixels_per_point,
                    snap: cx.mods.shift,
                    // A line trim with Ctrl cuts the other side: shade that.
                    other_side: self.action == Action::Trim && removes,
                };
                drag.draw(&mut draw.overlay, look);
            }
            None if cx.hovering && self.refusal(cx).is_none() => self.draw_crosshair(cx, draw),
            None => {}
        }
    }

    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        panel::panel(self, ui, cx);
    }

    fn radial(&self) -> Vec<RadialItem> {
        self.wheel().into_iter().map(|(tile, _)| tile).collect()
    }

    /// States worth a screenshot, each reached through the gesture's own
    /// code. See `demo.rs`.
    fn demo(&mut self, cx: &mut Ctx, what: &str) {
        crate::demo::region(self, cx, what);
    }
}
