//! The rulers themselves: laying them, dragging their ends, picking them and
//! drawing them.
//!
//! Ends snap to whatever surface is under the pointer, so a ruler measures
//! the model itself; off the model an end slides on the plane facing the
//! camera through where it was. Rulers live in world space and are a
//! scaffold, not scene objects: not saved, and not on the undo history.

use egui::Color32;
use glam::{Vec2, Vec3};
use sculpt_geom::{LengthUnit, units};
use sculpt_toolkit::{Ctx, Draw, Label, Run, View};

/// Pick distances and the click-or-drag threshold, in interface points.
const HANDLE_PICK: f32 = 10.0;
const LINE_PICK: f32 = 6.0;
const DRAG_THRESHOLD: f32 = 4.0;
/// Half the length of the tick across each end.
const TICK: f32 = 7.0;

const LINE: Color32 = Color32::from_rgba_premultiplied(242, 198, 61, 242);
const SELECTED_LINE: Color32 = Color32::from_rgb(255, 237, 140);
const HANDLE: Color32 = Color32::from_rgb(255, 209, 64);
const HOVER_HANDLE: Color32 = Color32::WHITE;
const HANDLE_OUTLINE: Color32 = Color32::from_rgba_premultiplied(19, 19, 24, 242);
const LABEL_BACKGROUND: Color32 = Color32::from_rgba_premultiplied(17, 17, 22, 217);
const SELECTED_LABEL_BACKGROUND: Color32 = Color32::from_rgba_premultiplied(38, 33, 14, 235);
/// Dashed legs of the selected ruler, one per world axis.
const AXIS_LEGS: [Color32; 3] = [
    Color32::from_rgba_premultiplied(218, 80, 80, 230),
    Color32::from_rgba_premultiplied(103, 207, 92, 230),
    Color32::from_rgba_premultiplied(92, 138, 230, 230),
];
/// The axis letters in a breakdown.
pub const AXIS_INK: [Color32; 3] =
    [Color32::from_rgb(242, 90, 90), Color32::from_rgb(115, 230, 102), Color32::from_rgb(102, 153, 255)];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ruler {
    pub a: Vec3,
    pub b: Vec3,
}

impl Ruler {
    fn end(&self, end: usize) -> Vec3 {
        if end == 0 { self.a } else { self.b }
    }

    fn set_end(&mut self, end: usize, p: Vec3) {
        if end == 0 {
            self.a = p;
        } else {
            self.b = p;
        }
    }

    /// From the first end to the second, along each world axis.
    pub fn delta(&self) -> Vec3 {
        self.b - self.a
    }

    pub fn length(&self) -> f32 {
        self.delta().length()
    }

    fn is_a_dot(&self) -> bool {
        self.delta().length_squared() < 1e-12
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum DragKind {
    #[default]
    None,
    /// A new ruler, its second end under the held button.
    Creating,
    /// A new ruler started with a click: its second end follows the pointer
    /// until the next click fixes it.
    Placing,
    /// One end of an existing ruler.
    End,
}

#[derive(Default)]
pub struct Rulers {
    rulers: Vec<Ruler>,
    selected: Option<usize>,
    drag: DragKind,
    drag_ruler: usize,
    drag_end: usize,
    /// Where the dragged end was, for Esc.
    drag_start: Vec3,
    press_px: Vec2,
    drag_moved: bool,
    /// The plane an end slides on while the pointer is off every surface:
    /// facing the camera, through where that end was when the drag began, so
    /// it never jumps in depth.
    plane_point: Vec3,
    /// The axis the last Shift-constrained update locked to, for the label.
    locked_axis: Option<usize>,
    /// The ruler under the pointer, and which end of it if an end.
    hover: Option<(usize, Option<usize>)>,
}

fn distance_to_segment(p: Vec2, a: Vec2, b: Vec2) -> f32 {
    let ab = b - a;
    let len2 = ab.length_squared();
    let t = if len2 > 1e-6 { ((p - a).dot(ab) / len2).clamp(0.0, 1.0) } else { 0.0 };
    (a + ab * t).distance(p)
}

fn non_zero_axes(d: Vec3) -> usize {
    (0..3).filter(|&axis| d[axis].abs() > 1e-6).count()
}

/// "X 12.00  Y 3.20  Z 20.10" as text and colour pairs, each letter in its
/// axis colour.
pub fn axis_breakdown(delta: Vec3, unit: LengthUnit) -> Vec<(String, Color32)> {
    let mut parts = Vec::with_capacity(6);
    for (axis, name) in ["X", "Y", "Z"].into_iter().enumerate() {
        let gap = if axis == 0 { "" } else { "  " };
        parts.push((format!("{gap}{name}"), AXIS_INK[axis]));
        parts.push((format!(" {}", units::format(delta[axis].abs(), unit, false)), Color32::WHITE));
    }
    parts
}

impl Rulers {
    pub fn len(&self) -> usize {
        self.rulers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rulers.is_empty()
    }

    pub fn get(&self, index: usize) -> Option<Ruler> {
        self.rulers.get(index).copied()
    }

    pub fn selected(&self) -> Option<Ruler> {
        self.selected.and_then(|i| self.get(i))
    }

    /// True while a clicked ruler is waiting for the click that fixes its
    /// second end.
    pub fn is_placing(&self) -> bool {
        self.drag == DragKind::Placing
    }

    pub fn is_dragging(&self) -> bool {
        self.drag != DragKind::None
    }

    // ---- operations --------------------------------------------------------

    pub fn clear(&mut self) {
        *self = Rulers::default();
    }

    pub fn delete_selected(&mut self) -> bool {
        match self.selected {
            Some(index) => {
                self.delete(index);
                true
            }
            None => false,
        }
    }

    fn delete(&mut self, index: usize) {
        if index >= self.rulers.len() {
            return;
        }
        if self.drag != DragKind::None {
            if self.drag_ruler == index {
                self.drag = DragKind::None;
                self.locked_axis = None;
            } else if self.drag_ruler > index {
                self.drag_ruler -= 1;
            }
        }
        self.rulers.remove(index);
        // Rulers after the one removed move up a row.
        let shift = |i: usize| if i > index { i - 1 } else { i };
        self.selected = match self.selected {
            Some(s) if s == index => self.rulers.len().checked_sub(1),
            other => other.map(shift),
        };
        self.hover = self.hover.filter(|(r, _)| *r != index).map(|(r, end)| (shift(r), end));
    }

    /// Lets go of the selection. False if there was none.
    pub fn deselect(&mut self) -> bool {
        self.selected.take().is_some()
    }

    // ---- gesture steps -----------------------------------------------------
    // What a press, drag and release do. Public so scripted input (the demo,
    // and tests) drives exactly the path the pointer does.

    /// A left press at `px`: grabs the ruler end under it, selects the ruler
    /// under it, or starts a new ruler there. While a clicked ruler is
    /// waiting for its second end, fixes it. `off_surface` lets a new ruler
    /// start where there is no model (and ignores the model where there is).
    /// Returns false when there was nothing to grab or start.
    pub fn press(&mut self, cx: &Ctx, px: Vec2, off_surface: bool) -> bool {
        if self.drag == DragKind::Placing {
            // A pen can tap without hovering there first, so the loose end is
            // brought to the press before it is fixed.
            self.drag_to(cx, px, cx.mods.shift, off_surface);
            self.release();
            return true;
        }
        if self.drag != DragKind::None {
            return true;
        }
        if let Some((ruler, end)) = self.pick_handle(&cx.view, px) {
            self.selected = Some(ruler);
            self.drag = DragKind::End;
            self.drag_ruler = ruler;
            self.drag_end = end;
            self.drag_start = self.rulers[ruler].end(end);
            self.plane_point = self.drag_start;
            self.press_px = px;
            self.drag_moved = false;
            self.locked_axis = None;
            return true;
        }
        if let Some(ruler) = self.pick_line(&cx.view, px) {
            self.selected = Some(ruler);
            return true;
        }
        self.begin_create(cx, px, off_surface)
    }

    fn begin_create(&mut self, cx: &Ctx, px: Vec2, off_surface: bool) -> bool {
        let on_model = if off_surface { None } else { cx.pick(px).map(|hit| hit.position) };
        let start = match on_model {
            Some(p) => p,
            None if off_surface => {
                // Nothing under the pointer: a camera-facing plane through
                // the middle of what is being worked on, which is where the
                // eye reads an off-model click as being.
                let Some(p) = cx.view.on_facing_plane(px, default_depth_point(cx)) else { return false };
                p
            }
            None => return false,
        };
        self.rulers.push(Ruler { a: start, b: start });
        let index = self.rulers.len() - 1;
        self.selected = Some(index);
        self.drag = DragKind::Creating;
        self.drag_ruler = index;
        self.drag_end = 1;
        self.drag_start = start;
        self.plane_point = start;
        self.press_px = px;
        self.drag_moved = false;
        self.locked_axis = None;
        true
    }

    /// A click on empty background: one end of a new ruler goes there, and
    /// the other follows the pointer until the next click.
    pub fn click_off_model(&mut self, cx: &Ctx, px: Vec2) -> bool {
        if self.drag != DragKind::None || !self.begin_create(cx, px, true) {
            return false;
        }
        self.drag = DragKind::Placing;
        true
    }

    /// The pointer at `px` with the button held (or, while placing, just
    /// hovering). `lock_axis` keeps the ruler on the world axis it already
    /// runs closest to; `off_surface` stops the end snapping to the model.
    pub fn drag_to(&mut self, cx: &Ctx, px: Vec2, lock_axis: bool, off_surface: bool) {
        if self.drag == DragKind::None {
            return;
        }
        if self.drag != DragKind::Placing && px.distance(self.press_px) > DRAG_THRESHOLD * cx.view.pixels_per_point {
            self.drag_moved = true;
        }
        let Some(ruler) = self.rulers.get_mut(self.drag_ruler) else { return };
        let on_model = if off_surface { None } else { cx.pick(px).map(|hit| hit.position) };
        let Some(mut p) = on_model.or_else(|| cx.view.on_facing_plane(px, self.plane_point)) else { return };

        let anchor = ruler.end(1 - self.drag_end);
        self.locked_axis = None;
        if lock_axis {
            // Onto the world axis the ruler already runs closest to: the way
            // to read a pure height, width or depth off a model that is not
            // a box.
            let d = (p - anchor).abs();
            let axis = if d.x >= d.y && d.x >= d.z {
                0
            } else if d.y >= d.z {
                1
            } else {
                2
            };
            let mut locked = anchor;
            locked[axis] = p[axis];
            p = locked;
            self.locked_axis = Some(axis);
        }
        ruler.set_end(self.drag_end, p);
    }

    /// The button let go, or, while placing, the second click.
    pub fn release(&mut self) {
        match self.drag {
            DragKind::None => {}
            DragKind::Placing => {
                let ruler = self.drag_ruler;
                self.end_drag();
                // Two clicks on the same spot measure nothing: drop it
                // rather than leave a dot.
                if self.rulers.get(ruler).is_some_and(Ruler::is_a_dot) {
                    self.delete(ruler);
                }
            }
            DragKind::Creating if !self.drag_moved => {
                // A click rather than a drag: the second end follows the
                // pointer until the next click, for when one long drag
                // across the model is awkward.
                self.drag = DragKind::Placing;
            }
            DragKind::Creating | DragKind::End => self.end_drag(),
        }
    }

    /// Esc: drops a new ruler, or puts a dragged end back. False if no drag
    /// was in progress.
    pub fn cancel_drag(&mut self) -> bool {
        let kind = std::mem::take(&mut self.drag);
        self.locked_axis = None;
        match kind {
            DragKind::None => return false,
            DragKind::End => {
                if let Some(ruler) = self.rulers.get_mut(self.drag_ruler) {
                    ruler.set_end(self.drag_end, self.drag_start);
                }
            }
            DragKind::Creating | DragKind::Placing => self.delete(self.drag_ruler),
        }
        true
    }

    fn end_drag(&mut self) {
        self.drag = DragKind::None;
        self.locked_axis = None;
        // A drag that ended with both ends on one point would be an
        // invisible ruler nothing could pick.
        if self.drag_moved && self.rulers.get(self.drag_ruler).is_some_and(Ruler::is_a_dot) {
            self.delete(self.drag_ruler);
        }
    }

    /// A right press at `px`: abandons a ruler being placed, else deletes the
    /// ruler under the pointer. False if neither.
    pub fn right_press(&mut self, view: &View, px: Vec2) -> bool {
        if self.drag == DragKind::Placing {
            return self.cancel_drag();
        }
        if self.drag != DragKind::None {
            return true;
        }
        match self.pick_handle(view, px).map(|(ruler, _)| ruler).or_else(|| self.pick_line(view, px)) {
            Some(ruler) => {
                self.delete(ruler);
                true
            }
            None => false,
        }
    }

    // ---- picking -----------------------------------------------------------

    fn pick_handle(&self, view: &View, px: Vec2) -> Option<(usize, usize)> {
        let mut best = None;
        let mut best_distance = HANDLE_PICK * view.pixels_per_point;
        for (i, ruler) in self.rulers.iter().enumerate() {
            for end in 0..2 {
                let Some(s) = view.project(ruler.end(end)) else { continue };
                // A hair of preference for the selected ruler where two
                // share an end.
                let d = s.distance(px) - if self.selected == Some(i) { 0.5 } else { 0.0 };
                if d < best_distance {
                    best_distance = d;
                    best = Some((i, end));
                }
            }
        }
        best
    }

    fn pick_line(&self, view: &View, px: Vec2) -> Option<usize> {
        let mut best = None;
        let mut best_distance = LINE_PICK * view.pixels_per_point;
        for (i, ruler) in self.rulers.iter().enumerate() {
            let (Some(a), Some(b)) = (view.project(ruler.a), view.project(ruler.b)) else { continue };
            let d = distance_to_segment(px, a, b);
            if d < best_distance {
                best_distance = d;
                best = Some(i);
            }
        }
        best
    }

    // ---- every frame -------------------------------------------------------

    /// Follows the pointer (the second end of a ruler being placed, and what
    /// is under it otherwise) and draws the rulers.
    pub fn frame(&mut self, cx: &Ctx, draw: &mut Draw) {
        self.hover = None;
        if cx.clean_view {
            return;
        }
        match self.drag {
            // The second end follows the pointer until the next click fixes
            // it. Over a panel it holds still, so reaching for a button does
            // not drag it off.
            DragKind::Placing if cx.hovering => self.drag_to(cx, cx.pointer, cx.mods.shift, cx.mods.ctrl),
            DragKind::None if cx.hovering => {
                self.hover = self
                    .pick_handle(&cx.view, cx.pointer)
                    .map(|(ruler, end)| (ruler, Some(end)))
                    .or_else(|| self.pick_line(&cx.view, cx.pointer).map(|ruler| (ruler, None)));
            }
            _ => {}
        }
        self.draw(cx, draw);
    }

    fn draw(&self, cx: &Ctx, draw: &mut Draw) {
        let view = &cx.view;
        let ppp = view.pixels_per_point;
        // Labels go on after every line, later rulers over earlier ones and
        // the selected one on top.
        let mut labels = Vec::with_capacity(self.rulers.len());
        let mut selected_label = None;

        for (i, ruler) in self.rulers.iter().enumerate() {
            let (Some(a), Some(b)) = (view.project(ruler.a), view.project(ruler.b)) else { continue };
            let selected = self.selected == Some(i);
            let dragging = self.drag != DragKind::None && self.drag_ruler == i;

            let dir = b - a;
            let screen_length = dir.length();
            let mut side = if screen_length > 1e-3 { Vec2::new(-dir.y, dir.x) / screen_length } else { Vec2::NEG_Y };

            // The selected ruler's axis legs, so its X/Y/Z breakdown can be
            // seen, not just read.
            if selected && !ruler.is_a_dot() {
                self.draw_axis_legs(view, ruler, draw);
            }

            let line = if selected { SELECTED_LINE } else { LINE };
            draw.overlay.line(a, b, if selected { 2.5 } else { 2.0 }, line);
            // End ticks across the line: a ruler reads as a dimension, not a
            // stray stroke.
            let tick = side * TICK * ppp;
            draw.overlay.line(a - tick, a + tick, 2.0, line);
            draw.overlay.line(b - tick, b + tick, 2.0, line);

            for (end, s) in [a, b].into_iter().enumerate() {
                let lit = self.hover == Some((i, Some(end))) || (dragging && end == self.drag_end);
                let (radius, fill) = if lit { (5.5, HOVER_HANDLE) } else { (4.0, HANDLE) };
                draw.overlay.dot(s, radius, fill, HANDLE_OUTLINE);
            }

            // The label, beside the middle of the line on whichever side is
            // higher on screen.
            if side.y > 0.0 {
                side = -side;
            }
            let label = Label {
                at: (a + b) * 0.5,
                push: side,
                gap: 8.0,
                runs: self.label_runs(ruler, selected, dragging, cx.unit),
                background: if selected { SELECTED_LABEL_BACKGROUND } else { LABEL_BACKGROUND },
            };
            if selected {
                selected_label = Some(label);
            } else {
                labels.push(label);
            }
        }
        for label in labels.into_iter().chain(selected_label) {
            draw.overlay.label(label);
        }
    }

    fn label_runs(&self, ruler: &Ruler, selected: bool, dragging: bool, unit: LengthUnit) -> Vec<Run> {
        let delta = ruler.delta();
        let length = Run::new(units::format(delta.length(), unit, true), Color32::WHITE);
        if !selected && !dragging {
            return vec![length];
        }
        let mut runs = vec![length.bold()];
        match self.locked_axis.filter(|_| dragging) {
            Some(axis) => runs.push(Run::new(format!("  ({} only)", ["X", "Y", "Z"][axis]), Color32::WHITE)),
            None if non_zero_axes(delta) > 1 => {
                runs.push(Run::new("\n", Color32::WHITE).size(11.0));
                runs.extend(axis_breakdown(delta, unit).into_iter().map(|(text, color)| Run::new(text, color).size(11.0)));
            }
            None => {}
        }
        runs
    }

    /// Three coloured legs from one end to the other, one world axis at a
    /// time.
    fn draw_axis_legs(&self, view: &View, ruler: &Ruler, draw: &mut Draw) {
        let mut p = ruler.a;
        for axis in 0..3 {
            let d = ruler.b[axis] - ruler.a[axis];
            if d.abs() < 1e-6 {
                continue;
            }
            let mut q = p;
            q[axis] += d;
            if let (Some(s0), Some(s1)) = (view.project(p), view.project(q)) {
                draw.overlay.dashed(s0, s1, 1.5, AXIS_LEGS[axis], 6.0, 4.0);
            }
            p = q;
        }
    }
}

/// The middle of the selected object: the depth an off-model click is taken
/// to be at.
fn default_depth_point(cx: &Ctx) -> Vec3 {
    let scene = cx.scene();
    let bounds = scene.bounds(scene.active());
    if bounds.is_empty() { Vec3::ZERO } else { bounds.center() }
}
