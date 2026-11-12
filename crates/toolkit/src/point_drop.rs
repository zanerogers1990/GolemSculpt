//! A point dragged along a line and let go on top of another point of that line collapses into it:
//! the dragged point goes and the one it was dropped on stays as it was. The tools whose shape is a
//! line of points share this, so they all mean the same by "on top of".
//!
//! It is judged on screen, where the two dots are seen to touch. A dragged point that starts on top of
//! another (zoomed well out, or just made on top of an end to be pulled out of it) has to leave that
//! point first, so a small move of a crowded point never takes its neighbour's place.

use glam::Vec2;

/// How close two points' centres are on screen, in interface points, when their dots touch.
const REACH: f32 = 10.0;

/// How big the point about to take the dragged one is drawn, in interface points.
pub const DROP_RADIUS: f32 = 8.0;

/// Which point of its line a dragged point is on top of. `points` is where each point of the line is on
/// screen, by index; `dragged` is ignored among them, since it is `at` by then.
#[derive(Clone, Debug, Default)]
pub struct PointDrop {
    /// Points the dragged one started on and has not yet left.
    started_on: Vec<usize>,
    over: Option<usize>,
}

impl PointDrop {
    /// The press: the dragged point is at `at`.
    pub fn begin(dragged: usize, at: Vec2, points: &[Option<Vec2>], pixels_per_point: f32) -> PointDrop {
        let reach = REACH * pixels_per_point;
        let on = |p: &Option<Vec2>| p.is_some_and(|p| p.distance(at) < reach);
        let started_on = points.iter().enumerate().filter(|&(i, p)| i != dragged && on(p)).map(|(i, _)| i).collect();
        PointDrop { started_on, over: None }
    }

    /// The dragged point has got to `at`.
    pub fn follow(&mut self, dragged: usize, at: Vec2, points: &[Option<Vec2>], pixels_per_point: f32) {
        let reach = REACH * pixels_per_point;
        let distance = |p: &Option<Vec2>| p.map(|p| p.distance(at)).filter(|&d| d < reach);
        self.started_on.retain(|&i| points.get(i).is_some_and(|p| distance(p).is_some()));
        self.over = points
            .iter()
            .enumerate()
            .filter(|&(i, _)| i != dragged && !self.started_on.contains(&i))
            .filter_map(|(i, p)| Some((i, distance(p)?)))
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
    }

    /// The point the dragged one would collapse into if it were let go now.
    pub fn over(&self) -> Option<usize> {
        self.over
    }

    /// Where point `into` is in the line once point `dragged` has been taken out of it.
    pub fn after_removing(dragged: usize, into: usize) -> usize {
        if into > dragged { into - 1 } else { into }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line() -> Vec<Option<Vec2>> {
        [0.0, 100.0, 200.0].into_iter().map(|x| Some(Vec2::new(x, 0.0))).collect()
    }

    #[test]
    fn a_point_is_over_the_nearest_other_point_in_reach_and_never_itself() {
        let points = line();
        let mut drop = PointDrop::begin(1, Vec2::new(100.0, 0.0), &points, 1.0);
        drop.follow(1, Vec2::new(100.0, 0.0), &points, 1.0);
        assert_eq!(drop.over(), None, "on its own place");
        drop.follow(1, Vec2::new(195.0, 3.0), &points, 1.0);
        assert_eq!(drop.over(), Some(2));
        drop.follow(1, Vec2::new(150.0, 0.0), &points, 1.0);
        assert_eq!(drop.over(), None, "and off it again");
        drop.follow(1, Vec2::new(4.0, 0.0), &points, 1.0);
        assert_eq!(drop.over(), Some(0));
    }

    #[test]
    fn a_point_that_started_on_another_has_to_leave_it_first() {
        let points = vec![Some(Vec2::ZERO), Some(Vec2::new(4.0, 0.0))];
        let mut drop = PointDrop::begin(1, Vec2::new(4.0, 0.0), &points, 1.0);
        drop.follow(1, Vec2::new(6.0, 0.0), &points, 1.0);
        assert_eq!(drop.over(), None, "still where it started");
        drop.follow(1, Vec2::new(40.0, 0.0), &points, 1.0);
        drop.follow(1, Vec2::new(5.0, 0.0), &points, 1.0);
        assert_eq!(drop.over(), Some(0), "it went away and came back");
    }

    #[test]
    fn reach_goes_by_interface_points_and_a_point_off_screen_is_never_under_it() {
        let points = vec![None, Some(Vec2::new(100.0, 0.0)), Some(Vec2::new(300.0, 0.0))];
        let mut drop = PointDrop::begin(2, Vec2::new(300.0, 0.0), &points, 2.0);
        drop.follow(2, Vec2::new(115.0, 0.0), &points, 2.0);
        assert_eq!(drop.over(), Some(1), "15 pixels is within 10 points at twice the density");
        drop.follow(2, Vec2::new(115.0, 0.0), &points, 1.0);
        assert_eq!(drop.over(), None);
        drop.follow(2, Vec2::ZERO, &points, 1.0);
        assert_eq!(drop.over(), None);
    }

    #[test]
    fn the_point_kept_is_found_again_after_the_other_is_taken_out() {
        assert_eq!(PointDrop::after_removing(1, 2), 1);
        assert_eq!(PointDrop::after_removing(2, 1), 1);
        assert_eq!(PointDrop::after_removing(3, 0), 0);
    }
}
