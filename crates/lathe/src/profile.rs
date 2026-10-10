//! The 2D profile a lathe revolves: an ordered list of control points in (radius, height)
//! space, where x is the distance from the axis and y the height along it.
//!
//! The curve passes THROUGH every point (see [`crate::curve`]), so what the artist drags is exactly
//! where the silhouette goes.
//!
//! Radius is never negative: the profile lives on one side of the axis, and the mesh builder
//! supplies the other side by revolving it. Endpoints may sit exactly on the axis (x = 0), which
//! closes that end of the solid with a single pole vertex; interior points are kept a hair off it
//! ([`LatheProfile::min_interior_radius`]), because an interior point on the axis would pinch the
//! surface to a point and split it into two solids touching at a vertex - non-manifold, and useless
//! for sculpting or molding.

use std::ops::Index;

use glam::Vec2;

/// Interior points closer to the axis than this fraction of the profile's size are held off it.
/// Small enough to allow a very narrow neck, large enough that the neck is never a single vertex.
pub const MIN_INTERIOR_RADIUS_FRACTION: f32 = 0.004;

/// One control point of a profile.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProfilePoint {
    /// x = radius (>= 0), y = height.
    pub position: Vec2,
    /// A corner: the curve arrives and leaves in straight-ish lines instead of rounding through.
    /// Off by default - smooth is what a thrown pot looks like - but a flat base meeting a wall
    /// needs one.
    pub sharp: bool,
}

impl ProfilePoint {
    pub fn new(position: Vec2, sharp: bool) -> ProfilePoint {
        ProfilePoint { position, sharp }
    }

    /// A point the curve rounds through.
    pub fn smooth(radius: f32, height: f32) -> ProfilePoint {
        ProfilePoint::new(Vec2::new(radius, height), false)
    }

    /// A corner point.
    pub fn corner(radius: f32, height: f32) -> ProfilePoint {
        ProfilePoint::new(Vec2::new(radius, height), true)
    }
}

/// The profile a lathe revolves, with the axis rules applied on every edit.
#[derive(Clone, Debug, Default)]
pub struct LatheProfile {
    points: Vec<ProfilePoint>,
    closed_loop: bool,
    version: u64,
}

impl Index<usize> for LatheProfile {
    type Output = ProfilePoint;

    fn index(&self, index: usize) -> &ProfilePoint {
        &self.points[index]
    }
}

impl LatheProfile {
    pub fn new() -> LatheProfile {
        LatheProfile::default()
    }

    /// A profile holding `points`, with the axis rules already applied.
    pub fn from_points(points: &[ProfilePoint], closed_loop: bool) -> LatheProfile {
        let mut profile = LatheProfile::new();
        profile.set_points(points, closed_loop);
        profile
    }

    pub fn len(&self) -> usize {
        self.points.len()
    }

    pub fn is_empty(&self) -> bool {
        self.points.is_empty()
    }

    pub fn points(&self) -> &[ProfilePoint] {
        &self.points
    }

    /// Bumped on every change, so views and the mesh cache can tell whether they are stale with one
    /// integer compare.
    pub fn version(&self) -> u64 {
        self.version
    }

    /// The curve closes back on itself (last point joins the first) - revolving it makes a ring or
    /// torus rather than a solid of revolution.
    pub fn closed_loop(&self) -> bool {
        self.closed_loop
    }

    pub fn set_closed_loop(&mut self, closed_loop: bool) {
        if self.closed_loop == closed_loop {
            return;
        }
        self.closed_loop = closed_loop;
        // A loop has no ends, so a point parked on the axis as a pole would become an interior
        // point on the axis - exactly the pinch the module remarks rule out.
        if closed_loop {
            self.clamp_all();
        }
        self.version += 1;
    }

    /// The fewest points that still describe a curve.
    pub fn min_points(&self) -> usize {
        if self.closed_loop { 3 } else { 2 }
    }

    pub fn is_endpoint(&self, index: usize) -> bool {
        !self.closed_loop && !self.points.is_empty() && (index == 0 || index == self.points.len() - 1)
    }

    // ---------------------------------------------------------------------------- edits

    /// Moves a point, applying the axis rules. Returns the position actually stored. A position
    /// that is not a number leaves the point where it was.
    pub fn set(&mut self, index: usize, position: Vec2) -> Vec2 {
        let clamped = self.clamp(index, position);
        if clamped == self.points[index].position {
            return clamped;
        }
        self.points[index].position = clamped;
        self.version += 1;
        clamped
    }

    pub fn set_sharp(&mut self, index: usize, sharp: bool) {
        if self.points[index].sharp == sharp {
            return;
        }
        self.points[index].sharp = sharp;
        self.version += 1;
    }

    /// Inserts a point (the index is clamped to the list) and returns where it went.
    pub fn insert(&mut self, index: usize, position: Vec2, sharp: bool) -> usize {
        let index = index.min(self.points.len());
        self.points.insert(index, ProfilePoint::new(position, sharp));
        // Re-clamped as a whole: inserting at an end demotes the old endpoint to an interior
        // point, which may no longer sit on the axis.
        self.clamp_all();
        self.version += 1;
        index
    }

    pub fn remove(&mut self, index: usize) {
        self.points.remove(index);
        // And the reverse: removing an end promotes its neighbour, which keeps its position.
        self.clamp_all();
        self.version += 1;
    }

    /// Removes every point and opens the loop.
    pub fn clear(&mut self) {
        if self.points.is_empty() && !self.closed_loop {
            return;
        }
        self.points.clear();
        self.closed_loop = false;
        self.version += 1;
    }

    pub fn set_points(&mut self, points: &[ProfilePoint], closed_loop: bool) {
        self.points.clear();
        self.points.extend_from_slice(points);
        self.closed_loop = closed_loop;
        self.clamp_all();
        self.version += 1;
    }

    // --------------------------------------------------------------------- measurement

    pub fn max_radius(&self) -> f32 {
        self.points.iter().fold(0.0, |max, p| max.max(p.position.x))
    }

    pub fn min_height(&self) -> f32 {
        if self.points.is_empty() {
            return 0.0;
        }
        self.points.iter().fold(f32::MAX, |min, p| min.min(p.position.y))
    }

    pub fn max_height(&self) -> f32 {
        if self.points.is_empty() {
            return 0.0;
        }
        self.points.iter().fold(f32::MIN, |max, p| max.max(p.position.y))
    }

    pub fn height(&self) -> f32 {
        self.max_height() - self.min_height()
    }

    /// A length that stands for "how big is this profile", for the tolerances above.
    pub fn size(&self) -> f32 {
        self.max_radius().max(self.height()).max(1e-4)
    }

    pub fn min_interior_radius(&self) -> f32 {
        self.size() * MIN_INTERIOR_RADIUS_FRACTION
    }

    // ------------------------------------------------------------------- whole-profile

    /// Stretches every point away from (or toward) the axis. The Radius slider.
    pub fn scale_radius(&mut self, factor: f32) {
        if factor.is_nan() || factor <= 0.0 || (factor - 1.0).abs() < 1e-6 {
            return;
        }
        for p in &mut self.points {
            p.position.x *= factor;
        }
        self.clamp_all();
        self.version += 1;
    }

    /// Stretches every point along the axis about the profile's base, so the bottom stays where it
    /// is and the top moves. The Height slider.
    pub fn scale_height(&mut self, factor: f32) {
        if factor.is_nan() || factor <= 0.0 || (factor - 1.0).abs() < 1e-6 {
            return;
        }
        let base = self.min_height();
        for p in &mut self.points {
            p.position.y = base + (p.position.y - base) * factor;
        }
        self.version += 1;
    }

    // ------------------------------------------------------------------------ clamping

    fn clamp(&self, index: usize, mut position: Vec2) -> Vec2 {
        if !position.is_finite() {
            return self.points[index].position;
        }
        let min_radius = if self.is_endpoint(index) { 0.0 } else { self.min_interior_radius() };
        position.x = position.x.max(min_radius);
        position
    }

    fn clamp_all(&mut self) {
        if self.points.is_empty() {
            return;
        }
        let min_interior = self.min_interior_radius();
        let last = self.points.len() - 1;
        let closed_loop = self.closed_loop;
        for (i, p) in self.points.iter_mut().enumerate() {
            let endpoint = !closed_loop && (i == 0 || i == last);
            let min_radius = if endpoint { 0.0 } else { min_interior };
            if p.position.x.is_nan() || p.position.x < min_radius {
                p.position.x = min_radius;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar() -> LatheProfile {
        LatheProfile::from_points(
            &[ProfilePoint::corner(0.0, 0.0), ProfilePoint::smooth(0.5, 1.0), ProfilePoint::corner(0.0, 2.0)],
            false,
        )
    }

    #[test]
    fn interior_points_are_held_off_the_axis_but_ends_may_sit_on_it() {
        let mut p = bar();
        assert_eq!(p[0].position.x, 0.0);
        assert_eq!(p[2].position.x, 0.0);
        let stored = p.set(1, Vec2::new(-3.0, 1.0));
        assert!(stored.x > 0.0 && stored.x == p.min_interior_radius());
        assert_eq!(p.set(0, Vec2::new(-1.0, 0.0)).x, 0.0);
    }

    #[test]
    fn inserting_at_an_end_demotes_the_old_end_off_the_axis() {
        let mut p = bar();
        p.insert(0, Vec2::new(0.3, -1.0), false);
        assert!(p[1].position.x > 0.0, "old pole is now interior");
        assert_eq!(p[0].position.x, 0.3);
        p.remove(0);
        assert!(p[0].position.x > 0.0, "removing an end keeps its neighbour where it was");
    }

    #[test]
    fn closing_the_loop_pulls_every_point_off_the_axis() {
        let mut p = bar();
        p.set_closed_loop(true);
        assert!(p.points().iter().all(|q| q.position.x > 0.0));
        assert_eq!(p.min_points(), 3);
        assert!(!p.is_endpoint(0));
    }

    #[test]
    fn version_changes_only_when_the_profile_does() {
        let mut p = bar();
        let v = p.version();
        p.set(1, p[1].position);
        p.set_sharp(1, false);
        p.set_closed_loop(false);
        p.scale_radius(1.0);
        assert_eq!(p.version(), v);
        p.set_sharp(1, true);
        assert!(p.version() > v);
    }

    #[test]
    fn nan_positions_are_ignored() {
        let mut p = bar();
        let v = p.version();
        assert_eq!(p.set(1, Vec2::new(f32::NAN, 1.0)), Vec2::new(0.5, 1.0));
        assert_eq!(p.version(), v);
    }

    #[test]
    fn scaling_keeps_the_base_where_it_was() {
        let mut p = bar();
        p.scale_height(2.0);
        assert_eq!((p.min_height(), p.max_height()), (0.0, 4.0));
        p.scale_radius(3.0);
        assert!((p.max_radius() - 1.5).abs() < 1e-6);
        p.scale_radius(-1.0);
        assert!((p.max_radius() - 1.5).abs() < 1e-6);
    }

    #[test]
    fn clear_opens_the_loop_and_measurements_of_nothing_are_zero() {
        let mut p = bar();
        p.set_closed_loop(true);
        p.clear();
        assert!(p.is_empty() && !p.closed_loop());
        assert_eq!((p.min_height(), p.max_height(), p.max_radius()), (0.0, 0.0, 0.0));
    }
}
