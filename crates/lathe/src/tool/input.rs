//! Picking and gestures. Every drag is computed absolutely from the press, the handle stays under the
//! cursor (the grab offset is kept, so nothing jumps on the first move), and grabbing a mirrored handle
//! (the far silhouette) edits the same point through the mirror.
//!
//! The functions take the pixel they act at rather than reading the pointer, so the demo and the tests
//! drive exactly the code the pointer does.

use glam::Vec2;
use sculpt_toolkit::{Ctx, View};

use super::{
    AXIS_PICK, AXIS_SNAP, CURVE_PICK, DOUBLE_CLICK_SECONDS, DRAG_THRESHOLD, Drag, HANDLE_PICK, Hover, Insert, Lathe,
    SKETCH_CLOSE, SKETCH_SPACING,
};
use crate::{curve, sketch};

/// Straight pieces a curve segment is cut into to find where the pointer is on it.
const CURVE_STEPS: usize = 20;

impl Lathe {
    // ---------------------------------------------------------------------------- picking

    /// The handle nearest `px`, and which side of the axis it was found on. A hair of preference goes to
    /// the canonical side, where the two overlap near the axis.
    pub(super) fn pick_point(&self, view: &View, px: Vec2) -> Option<(usize, f32)> {
        let mut found = None;
        let mut best = HANDLE_PICK * view.pixels_per_point;
        for (i, point) in self.profile.points().iter().enumerate() {
            for side in [1.0, -1.0] {
                if side < 0.0 && point.position.x <= 0.0 {
                    continue;
                }
                let Some(screen) = self.plane.screen(view, point.position, side) else { continue };
                let score = screen.distance(px) + if side < 0.0 { 0.5 } else { 0.0 };
                if score < best {
                    best = score;
                    found = Some((i, side));
                }
            }
        }
        found
    }

    /// The nearest place on the curve (or on a cap) within reach of `px`, as a place to insert a point.
    pub(super) fn pick_curve(&self, view: &View, px: Vec2) -> Option<Insert> {
        let count = self.profile.len();
        if count < 2 {
            return None;
        }
        let points = self.profile.points();
        let closed = self.profile.closed_loop();
        let mut best = CURVE_PICK * view.pixels_per_point;
        let mut found = None;
        for side in [1.0f32, -1.0] {
            for segment in 0..curve::segment_count(points, closed) {
                let mut previous = points[segment].position;
                let mut previous_screen = self.plane.screen(view, previous, side);
                for step in 1..=CURVE_STEPS {
                    let current = curve::evaluate(points, closed, segment, step as f32 / CURVE_STEPS as f32);
                    let current_screen = self.plane.screen(view, current, side);
                    if let (Some(a), Some(b)) = (previous_screen, current_screen) {
                        let (distance, t) = sketch::distance_to_segment(px, a, b);
                        // Not right on top of an existing handle: that press is a handle grab.
                        let near_end = (step == 1 && t < 0.3) || (step == CURVE_STEPS && t > 0.7);
                        if distance < best && !near_end {
                            best = distance;
                            found = Some(Insert { index: segment + 1, position: previous.lerp(current, t), side, sharpen: None });
                        }
                    }
                    previous = current;
                    previous_screen = current_screen;
                }
            }
            // The caps are part of the silhouette too. A point inserted on one becomes the new end, and
            // the old end, where the cap met the wall, stays a corner.
            if !closed && self.settings.cap_ends {
                for (end, insert) in [(0, 0), (count - 1, count)] {
                    if let Some((distance, hit)) = self.cap_hit(view, px, end, insert, side, best) {
                        best = distance;
                        found = Some(hit);
                    }
                }
            }
        }
        found
    }

    /// Where on the flat cap closing the open end `end` the pointer is, if it is closer than `best`. A
    /// point put there goes in at `insert`.
    fn cap_hit(&self, view: &View, px: Vec2, end: usize, insert: usize, side: f32, best: f32) -> Option<(f32, Insert)> {
        let end_point = self.profile[end].position;
        if end_point.x <= 0.0 {
            return None;
        }
        let on_axis = Vec2::new(0.0, end_point.y);
        let (a, b) = (self.plane.screen(view, end_point, side)?, self.plane.screen(view, on_axis, side)?);
        let (distance, t) = sketch::distance_to_segment(px, a, b);
        if distance >= best || !(0.15..=0.95).contains(&t) {
            return None;
        }
        Some((distance, Insert { index: insert, position: end_point.lerp(on_axis, t), side, sharpen: Some(end) }))
    }

    /// Whether `px` is on the dashed axis.
    pub(super) fn near_axis(&self, view: &View, px: Vec2) -> bool {
        let (low, high) = self.axis_span();
        let (Some(a), Some(b)) = (
            self.plane.screen(view, Vec2::new(0.0, low), 1.0),
            self.plane.screen(view, Vec2::new(0.0, high), 1.0),
        ) else {
            return false;
        };
        sketch::distance_to_segment(px, a, b).0 < AXIS_PICK * view.pixels_per_point
    }

    /// What the highlight shows: only while the pointer is free over the viewport and nothing is being
    /// dragged.
    pub(super) fn find_hover(&self, cx: &Ctx) -> Hover {
        if !cx.hovering || cx.clean_view || self.dragging() {
            return Hover::default();
        }
        let (view, px) = (&cx.view, cx.pointer);
        if let Some(point) = self.pick_point(view, px) {
            return Hover { point: Some(point), ..Hover::default() };
        }
        if let Some(insert) = self.pick_curve(view, px) {
            return Hover { insert: Some(insert), ..Hover::default() };
        }
        Hover { axis: self.near_axis(view, px), ..Hover::default() }
    }

    /// Puts an end point exactly on the axis when it is within reach of it on screen: the only reliable
    /// way to hit r = 0 by hand, and the thing that closes an end into a pole.
    pub(super) fn snap_to_axis(&self, view: &View, mut p: Vec2) -> Vec2 {
        if p.x <= 0.0 {
            return Vec2::new(0.0, p.y);
        }
        let on_axis = self.plane.screen(view, Vec2::new(0.0, p.y), 1.0);
        if let (Some(s), Some(a)) = (self.plane.screen(view, p, 1.0), on_axis)
            && s.distance(a) < AXIS_SNAP * view.pixels_per_point
        {
            p.x = 0.0;
        }
        p
    }

    // ----------------------------------------------------------------------------- events

    /// A left press. Taken (and so followed by drags and a release) when it lands on a handle, on the
    /// curve, on the axis, or with Ctrl held to sketch; otherwise passed on, so a drag there orbits.
    pub(super) fn press(&mut self, cx: &mut Ctx) -> bool {
        // The clean view is look-only.
        if cx.clean_view {
            return false;
        }
        let (view, px, now) = (cx.view, cx.pointer, cx.time);
        if cx.mods.ctrl {
            self.begin_sketch(&view, px);
            return true;
        }
        if let Some((index, side)) = self.pick_point(&view, px) {
            self.selected = Some(index);
            let double = self.last_click.is_some_and(|(i, at)| i == index && now - at < DOUBLE_CLICK_SECONDS);
            if double {
                self.last_click = None;
                self.toggle_sharp(cx, index);
            } else {
                self.last_click = Some((index, now));
                self.begin_point_drag(now, &view, index, side, px, "Move Point");
            }
            return true;
        }
        self.last_click = None;
        if let Some(insert) = self.pick_curve(&view, px) {
            self.begin_edit(now, "Add Point");
            if let Some(end) = insert.sharpen {
                self.profile.set_sharp(end, true);
            }
            let index = self.profile.insert(insert.index, insert.position, false);
            self.selected = Some(index);
            self.begin_point_drag(now, &view, index, insert.side, px, "Add Point");
            return true;
        }
        if self.near_axis(&view, px) {
            self.begin_axis_drag(now, &view, px);
            return true;
        }
        false
    }

    /// A right press on a handle deletes it.
    pub(super) fn right_press(&mut self, cx: &mut Ctx) -> bool {
        if cx.clean_view || self.dragging() {
            return false;
        }
        match self.pick_point(&cx.view, cx.pointer) {
            Some((index, _)) => {
                self.delete_point(cx, index);
                true
            }
            None => false,
        }
    }

    /// A press the tool passed on came up without moving. On empty space that extends the profile; a
    /// drag there would have been the camera's.
    pub(super) fn click(&mut self, cx: &mut Ctx) -> bool {
        if cx.clean_view || self.dragging() {
            return false;
        }
        let px = cx.pointer;
        self.extend_profile(cx, px)
    }

    pub(super) fn release(&mut self, cx: &mut Ctx) {
        match std::mem::take(&mut self.drag) {
            Drag::None => {}
            Drag::Sketch { points, .. } => self.finish_sketch(cx, points),
            Drag::Axis { press, moved, .. } => {
                self.commit_edit();
                // The lathe was not moved: the press was a click on the axis, which places a point there.
                if !moved {
                    self.extend_profile(cx, press);
                }
            }
            Drag::Point { .. } => self.commit_edit(),
        }
    }

    pub(super) fn drag_to(&mut self, cx: &Ctx, px: Vec2, shift: bool) {
        match self.drag {
            Drag::Point { .. } => self.drag_point(&cx.view, px, shift),
            Drag::Axis { .. } => self.drag_axis(&cx.view, px),
            Drag::Sketch { .. } => self.drag_sketch(&cx.view, px),
            Drag::None => {}
        }
    }

    /// Esc: cancels the drag, else lets go of the selection. False when there was neither, and the app
    /// puts the tool away.
    pub(super) fn escape(&mut self, cx: &mut Ctx) -> bool {
        if self.dragging() {
            self.cancel_drag(cx);
            return true;
        }
        self.selected.take().is_some()
    }

    pub(super) fn delete_key(&mut self, cx: &mut Ctx) -> bool {
        if self.dragging() {
            return false;
        }
        match self.selected {
            Some(index) => {
                self.delete_point(cx, index);
                true
            }
            None => false,
        }
    }

    // ------------------------------------------------------------------------ point drags

    pub(super) fn begin_point_drag(
        &mut self,
        now: f64,
        view: &View,
        index: usize,
        side: f32,
        press: Vec2,
        label: &str,
    ) {
        // Re-entrant with the insert that may have just opened this step: an added point and the drag
        // that places it are ONE undo, and Esc takes both back.
        self.begin_edit(now, label);
        let start = self.profile[index].position;
        let grab = match self.plane.cursor(view, press) {
            Some(signed) => start - Vec2::new(signed.x * side, signed.y),
            None => Vec2::ZERO,
        };
        self.drag = Drag::Point { index, side, press, start, grab, moved: false };
    }

    pub(super) fn drag_point(&mut self, view: &View, px: Vec2, shift: bool) {
        let Drag::Point { index, side, press, start, grab, moved } = self.drag else { return };
        if index >= self.profile.len() {
            return;
        }
        if !moved && px.distance(press) < DRAG_THRESHOLD * view.pixels_per_point {
            return;
        }
        if let Drag::Point { moved, .. } = &mut self.drag {
            *moved = true;
        }
        let Some(signed) = self.plane.cursor(view, px) else { return };
        let mut target = Vec2::new(signed.x * side, signed.y) + grab;
        if shift {
            let delta = target - start;
            if delta.x.abs() > delta.y.abs() {
                target.y = start.y;
            } else {
                target.x = start.x;
            }
        }
        // Pulled across the axis, a point stops at it rather than flipping to the other side: the other
        // side is its own mirror image. End points snap onto it, closing that end.
        if self.profile.is_endpoint(index) {
            target = self.snap_to_axis(view, target);
        }
        self.profile.set(index, target);
    }

    // ------------------------------------------------------------------------- axis drags

    pub(super) fn begin_axis_drag(&mut self, now: f64, view: &View, press: Vec2) {
        self.begin_edit(now, "Move Axis");
        let start = self.plane.root;
        let anchor = view.on_facing_plane(press, start).unwrap_or(start);
        self.drag = Drag::Axis { press, start, anchor, moved: false };
    }

    /// The lathe follows the pointer on the plane that faces the camera through where it stood, so it
    /// keeps its distance from the eye.
    pub(super) fn drag_axis(&mut self, view: &View, px: Vec2) {
        let Drag::Axis { press, start, anchor, moved } = self.drag else { return };
        if !moved && px.distance(press) < DRAG_THRESHOLD * view.pixels_per_point {
            return;
        }
        if let Drag::Axis { moved, .. } = &mut self.drag {
            *moved = true;
        }
        if let Some(here) = view.on_facing_plane(px, start) {
            self.plane.root = start + (here - anchor);
        }
    }

    // ---------------------------------------------------------------------- extending

    /// A click away from the curve adds a point to the nearer END of an open profile (which, on an empty
    /// profile, is simply drawing it point by point) or, on a loop, into the nearest segment. False when
    /// the plane is edge-on to the view and the pointer means nothing on it.
    pub(super) fn extend_profile(&mut self, cx: &mut Ctx, px: Vec2) -> bool {
        let (view, now) = (cx.view, cx.time);
        let Some(signed) = self.plane.cursor(&view, px) else { return false };
        // The far side is the mirror image of the near one: a point there is a point here.
        let mut position = Vec2::new(signed.x.abs(), signed.y);
        let count = self.profile.len();
        let closed = self.profile.closed_loop();

        let index = if count < 2 {
            count
        } else if closed {
            sketch::nearest_insert_index(self.profile.points(), true, position)
        } else {
            let to_first = position.distance_squared(self.profile[0].position);
            let to_last = position.distance_squared(self.profile[count - 1].position);
            if to_first < to_last { 0 } else { count }
        };

        // A new end point, so the axis snap applies from the first moment.
        if !closed {
            position = self.snap_to_axis(&view, position);
        }

        // Extending past an end that is CLOSED on the axis means "take the outline up to here, still
        // closed", not "reopen it". Demoting the old pole to an interior point would leave it pinned a
        // hair off the axis: a pinched neck nobody drew. So the old pole becomes the new point, and a
        // fresh pole closes the end level with it.
        if count >= 2 && !closed {
            let end = if index == 0 { 0 } else { count - 1 };
            if self.profile[end].position.x <= 0.0 {
                self.begin_edit(now, "Extend");
                if position.x <= 0.0 {
                    // Dropped on the axis: just move the pole.
                    self.profile.set(end, position);
                    self.selected = Some(end);
                } else {
                    self.profile.insert(index, Vec2::new(0.0, position.y), false);
                    let moved = if index == 0 { 1 } else { count - 1 };
                    self.profile.set(moved, position);
                    self.selected = Some(moved);
                }
                self.commit_edit();
                return true;
            }
        }

        self.begin_edit(now, "Add Point");
        self.selected = Some(self.profile.insert(index, position, false));
        self.commit_edit();
        if self.profile.len() == 1 {
            cx.status("First point placed. Click again for the next - the shape appears at two.", true);
        }
        true
    }

    // ---------------------------------------------------------------------------- sketch

    pub(super) fn begin_sketch(&mut self, view: &View, px: Vec2) {
        let points = self.plane.cursor(view, px).into_iter().collect();
        self.drag = Drag::Sketch { points, last: px };
    }

    pub(super) fn drag_sketch(&mut self, view: &View, px: Vec2) {
        let Drag::Sketch { points, last } = &mut self.drag else { return };
        if px.distance(*last) < SKETCH_SPACING * view.pixels_per_point {
            return;
        }
        if let Some(signed) = self.plane.cursor(view, px) {
            points.push(signed);
        }
        *last = px;
    }

    /// Turns the stroke into a profile: onto one side of the axis, simplified to a handful of smooth
    /// points (the curve through them reproduces the stroke closely, and a handful is what can be
    /// refined by hand), ends snapped onto the axis if they were drawn to it, and closed into a loop if
    /// the stroke came back to where it started.
    pub(super) fn finish_sketch(&mut self, cx: &mut Ctx, mut stroke: Vec<Vec2>) {
        let (view, now) = (cx.view, cx.time);
        if stroke.len() < 3 {
            cx.status("Sketch a longer stroke: hold Ctrl and drag out the outline.", false);
            return;
        }
        let side = if stroke.iter().map(|p| p.x).sum::<f32>() < 0.0 { -1.0 } else { 1.0 };
        let folded = |p: Vec2| Vec2::new((p.x * side).max(0.0), p.y);

        let mut closed = false;
        if stroke.len() >= 6 {
            let first = self.plane.screen(&view, folded(stroke[0]), 1.0);
            let last = self.plane.screen(&view, folded(stroke[stroke.len() - 1]), 1.0);
            if let (Some(a), Some(b)) = (first, last)
                && a.distance(b) < SKETCH_CLOSE * view.pixels_per_point
            {
                closed = true;
                stroke.pop();
            }
        }

        let snap = |p: Vec2| self.snap_to_axis(&view, p);
        let Some(points) = sketch::stroke_to_points(&stroke, closed, snap) else {
            cx.status("That stroke is too small to make a profile.", false);
            return;
        };
        self.begin_edit(now, "Sketch Profile");
        self.profile.set_points(&points, closed);
        self.selected = None;
        self.commit_edit();
        cx.status(
            if closed {
                format!("Sketched a closed loop ({} points) - a ring.", points.len())
            } else {
                format!("Sketched {} points. Drag the handles to refine.", points.len())
            },
            true,
        );
    }
}
