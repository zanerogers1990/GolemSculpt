//! An object's whole array as one value: what undo steps, the save file and the
//! tool hand around, plus the edits whose arithmetic depends on how the layout
//! reads its points.
//!
//! The points are offsets from the object's position in world axes, so they and
//! the object move together unless an edit here says otherwise. Every edit that
//! carries the object along (a path's first point, posing a copy) cannot reach
//! the object's position from here: it returns what the caller must add to it.

use glam::Vec3;

use crate::layout::axis_vector;
use crate::path;
use crate::types::{ArrayKind, ArraySettings};

/// Settings and handle points. Kind `None` means "no array".
///
/// Saved as: kind id, count, end scale, mid scale, closed, follow, space by
/// size, axis, sweep, and the points (`saved.rs` writes and reads them as the
/// scene file keeps them). A file with no array reads back as kind 0 and no
/// points, which `ArrayState::none` is.
///
/// Equality is the undo system's question "did anything change": every field
/// exactly, except that any two states with no array are alike.
#[derive(Clone, Debug, Default)]
pub struct ArrayState {
    pub settings: ArraySettings,
    /// Handle points as offsets from the object's position: where the path
    /// starts, then the rest of it; a circle's second point is its centre.
    pub points: Vec<Vec3>,
}

impl PartialEq for ArrayState {
    fn eq(&self, other: &ArrayState) -> bool {
        if self.is_none() && other.is_none() {
            return true;
        }
        self.settings == other.settings && self.points == other.points
    }
}

impl ArrayState {
    /// The most handle points an array keeps. A path drawn by hand is simplified to a couple
    /// of dozen, and clicking adds one at a time; this is for a file or script that asks for
    /// a million, whose path would be sampled that many times over.
    pub const MAX_POINTS: usize = 256;
    /// How far from the object a handle point may be, along each axis: ten kilometres at
    /// the app's ten millimetres a unit. Far enough for anything drawn, near enough that
    /// the layout's arithmetic stays well inside what a float can hold.
    pub const MAX_REACH: f32 = 1.0e6;

    pub fn none() -> ArrayState {
        ArrayState { settings: ArraySettings::new(ArrayKind::None), points: Vec::new() }
    }

    pub fn is_none(&self) -> bool {
        self.settings.kind == ArrayKind::None
    }

    /// The state as it should be stored and laid out: settings in range, a
    /// reasonable number of points that are all places (a coordinate that is
    /// not a number is the object's own, one that is far off is brought in), and,
    /// for any kind, the object's own point and at least one more to aim at.
    /// What a loaded file or a script may not have respected.
    pub fn sanitized(&self) -> ArrayState {
        if self.is_none() {
            return ArrayState::none();
        }
        let reach = |c: f32| if c.is_finite() { c.clamp(-Self::MAX_REACH, Self::MAX_REACH) } else { 0.0 };
        let mut points: Vec<Vec3> = self
            .points
            .iter()
            .take(Self::MAX_POINTS)
            .map(|p| Vec3::new(reach(p.x), reach(p.y), reach(p.z)))
            .collect();
        if points.is_empty() {
            points.push(Vec3::ZERO);
        }
        if points.len() == 1 {
            points.push(points[0]);
        }
        ArrayState { settings: self.settings.clamped(), points }
    }

    /// True when the copies sit off the path, which is what moving one with the
    /// gizmo does.
    pub fn has_copy_offset(&self) -> bool {
        self.points.first().is_some_and(|&p| p != Vec3::ZERO)
    }

    /// How many handles the tool shows: every path point, or for a circle just
    /// the object and the centre (any further points are not part of it).
    pub fn handle_count(&self) -> usize {
        match self.settings.kind {
            ArrayKind::None => 0,
            ArrayKind::Radial => self.points.len().min(2),
            _ => self.points.len(),
        }
    }

    /// Moves handle `index` to a world position. Point 0 is the path's start,
    /// which the object rides on: moving it moves the object and leaves every
    /// other point where it is in the world. Returns the object's new position.
    pub fn set_point_world(&mut self, index: usize, world: Vec3, object_position: Vec3) -> Vec3 {
        if index >= self.points.len() {
            return object_position;
        }
        if index > 0 {
            self.points[index] = world - object_position;
            return object_position;
        }
        let delta = world - (object_position + self.points[0]);
        for p in &mut self.points[1..] {
            *p -= delta;
        }
        object_position + delta
    }

    /// Carries the object by `by` while the path stays where it is in the world:
    /// every point takes `-by`, and the caller adds `by` to the object's
    /// position. Posing one copy does this, so every copy sits off the path the
    /// same way; with `by` set to `points[0]` it puts the copies back on it.
    pub fn shift_object(&mut self, by: Vec3) {
        for p in &mut self.points {
            *p -= by;
        }
    }

    /// Puts every copy back on the path, which stays where it is. Returns what the
    /// caller must add to the object's position (the old `points[0]`).
    pub fn reset_copy_offset(&mut self) -> Vec3 {
        let start = self.points.first().copied().unwrap_or(Vec3::ZERO);
        self.shift_object(start);
        start
    }

    /// Adds a point on the path at `t` along `segment`, where the path already
    /// is so nothing moves, and makes the array a curve: a line with a point
    /// in the middle is one. Returns the new point's index.
    pub fn insert_point(&mut self, segment: usize, t: f32) -> usize {
        let closed = self.settings.closed;
        let segments = path::segment_count(self.points.len(), closed);
        let segment = segment.min(segments.saturating_sub(1));
        let position = path::evaluate(&self.points, closed, segment, t);
        self.points.insert(segment + 1, position);
        self.settings.kind = ArrayKind::Curve;
        segment + 1
    }

    /// Changes a circle's axis. The centre keeps its distance but swings out of
    /// the way if it would otherwise sit on the new axis, where every copy would
    /// land on the object. Anything but a circle, or one without a centre, is left
    /// alone.
    pub fn set_axis(&mut self, axis: i32) {
        let axis = axis.clamp(0, 2);
        if self.settings.kind != ArrayKind::Radial || self.points.len() < 2 {
            return;
        }
        self.settings.axis = axis;
        let direction = axis_vector(axis);
        let centre = self.points[1] - self.points[0];
        let across = centre - direction * centre.dot(direction);
        if across.length() < centre.length() * 0.15 {
            self.points[1] = self.points[0] + axis_vector((axis + 1) % 3) * centre.length().max(1e-4);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn curve_state() -> ArrayState {
        let mut settings = ArraySettings::new(ArrayKind::Curve);
        settings.count = 7;
        settings.end_scale = 0.4;
        settings.mid_scale = 1.5;
        settings.closed = true;
        ArrayState {
            settings,
            points: vec![Vec3::ZERO, Vec3::new(2.0, 1.0, 0.0), Vec3::new(3.0, -1.0, 2.0), Vec3::new(0.0, 0.0, 3.0)],
        }
    }

    #[test]
    fn a_valid_state_survives_sanitizing_unchanged() {
        let state = curve_state();
        assert_eq!(state.sanitized(), state);
    }

    #[test]
    fn sanitizing_keeps_a_reasonable_number_of_points_that_are_all_places() {
        let mut state = curve_state();
        state.points = vec![Vec3::new(f32::NAN, 1.0, f32::INFINITY), Vec3::new(1e30, -1e30, 2.0)];
        state.points.extend((0..1000).map(|i| Vec3::splat(i as f32)));
        let s = state.sanitized();
        assert_eq!(s.points.len(), ArrayState::MAX_POINTS);
        assert!(s.points.iter().all(|p| p.is_finite() && p.abs().max_element() <= ArrayState::MAX_REACH));
        assert_eq!(s.points[0], Vec3::new(0.0, 1.0, 0.0));
        assert_eq!(s.points[1], Vec3::new(ArrayState::MAX_REACH, -ArrayState::MAX_REACH, 2.0));
        assert_eq!(s.sanitized(), s, "sanitizing twice changes nothing");
    }

    #[test]
    fn sanitizing_gives_every_kind_two_points_and_legal_settings() {
        let mut state = ArrayState { settings: ArraySettings::new(ArrayKind::Line), points: Vec::new() };
        state.settings.count = 1000;
        let s = state.sanitized();
        assert_eq!(s.points, vec![Vec3::ZERO; 2]);
        assert_eq!(s.settings.count, ArraySettings::MAX_COUNT);

        state.points = vec![Vec3::new(1.0, 2.0, 3.0)];
        assert_eq!(state.sanitized().points, vec![Vec3::new(1.0, 2.0, 3.0); 2]);
    }

    #[test]
    fn a_circle_shows_two_handles_and_a_path_shows_every_point() {
        let mut state = curve_state();
        assert_eq!(state.handle_count(), 4);
        state.settings.kind = ArrayKind::Radial;
        assert_eq!(state.handle_count(), 2);
        state.points.truncate(1);
        assert_eq!(state.handle_count(), 1);
        assert_eq!(ArrayState::none().handle_count(), 0);
    }

    #[test]
    fn no_array_is_no_array_whatever_else_it_holds() {
        assert!(ArrayState::default().is_none());
        assert!(ArrayState::none().is_none());
        let mut leftover = curve_state();
        leftover.settings.kind = ArrayKind::None;
        assert_eq!(leftover, ArrayState::none());
        assert_eq!(leftover.sanitized(), ArrayState::none());
        assert_ne!(curve_state(), ArrayState::none());

        let mut changed = curve_state();
        changed.points[2].x += 0.001;
        assert_ne!(changed, curve_state());
    }

    #[test]
    fn dragging_the_first_point_carries_the_object_and_leaves_the_rest() {
        let mut state = curve_state();
        let object = Vec3::new(5.0, 0.0, 0.0);
        let world_before: Vec<Vec3> = state.points.iter().map(|&p| object + p).collect();

        let new_object = state.set_point_world(0, Vec3::new(5.0, 3.0, 0.0), object);
        assert_eq!(new_object, Vec3::new(5.0, 3.0, 0.0));
        for (i, (p, before)) in state.points.iter().zip(&world_before).enumerate().skip(1) {
            assert!((new_object + *p - *before).length() < 1e-6, "point {i} stayed put");
        }
        assert_eq!(state.points[0], Vec3::ZERO);

        let still = state.set_point_world(2, Vec3::new(9.0, 9.0, 9.0), new_object);
        assert_eq!(still, new_object);
        assert_eq!(new_object + state.points[2], Vec3::new(9.0, 9.0, 9.0));
        assert_eq!(state.set_point_world(99, Vec3::ONE, new_object), new_object);
    }

    #[test]
    fn shifting_the_object_off_the_path_and_back() {
        let mut state = curve_state();
        let object = Vec3::new(1.0, 1.0, 1.0);
        let world_before: Vec<Vec3> = state.points.iter().map(|&p| object + p).collect();
        assert!(!state.has_copy_offset());

        let by = Vec3::new(0.0, 0.5, 0.0);
        state.shift_object(by);
        assert!(state.has_copy_offset());
        for (i, p) in state.points.iter().enumerate() {
            assert!(((object + by + *p) - world_before[i]).length() < 1e-6, "point {i}: the path stays where it was");
        }

        let back = state.reset_copy_offset();
        assert_eq!(back, -by);
        assert!(!state.has_copy_offset());
        for (now, original) in state.points.iter().zip(&curve_state().points) {
            assert!((*now - *original).length() < 1e-6, "back where it was");
        }
    }

    #[test]
    fn inserting_a_point_on_the_path_changes_nothing_about_where_it_goes() {
        let mut state = curve_state();
        state.settings.closed = false;
        let before = path::evaluate(&state.points, false, 1, 0.5);
        let index = state.insert_point(1, 0.5);
        assert_eq!(index, 2);
        assert_eq!(state.points.len(), 5);
        assert!((state.points[2] - before).length() < 1e-6);
        assert_eq!(state.settings.kind, ArrayKind::Curve);

        let mut line = ArrayState {
            settings: ArraySettings::new(ArrayKind::Line),
            points: vec![Vec3::ZERO, Vec3::new(4.0, 0.0, 0.0)],
        };
        assert_eq!(line.insert_point(7, 0.25), 1, "a segment past the end is the last one");
        assert_eq!(line.settings.kind, ArrayKind::Curve);
        assert!((line.points[1] - Vec3::new(1.0, 0.0, 0.0)).length() < 1e-5);
    }

    #[test]
    fn changing_a_circles_axis_swings_a_centre_that_would_sit_on_it() {
        let mut state = ArrayState {
            settings: ArraySettings::new(ArrayKind::Radial),
            points: vec![Vec3::ZERO, Vec3::new(-2.0, 0.0, 0.0)],
        };
        // Y to Z: the centre at -X is already off the new axis, so it stays.
        state.set_axis(2);
        assert_eq!(state.settings.axis, 2);
        assert_eq!(state.points[1], Vec3::new(-2.0, 0.0, 0.0));

        // To X: the centre would lie on the axis, so it moves round to the next one at the same distance.
        state.set_axis(0);
        assert_eq!(state.settings.axis, 0);
        assert!((state.points[1] - Vec3::new(0.0, 2.0, 0.0)).length() < 1e-6);

        let mut line = ArrayState { settings: ArraySettings::new(ArrayKind::Line), points: vec![Vec3::ZERO; 2] };
        line.set_axis(0);
        assert_eq!(line.settings.axis, 1, "only a circle has an axis to change");
    }
}
