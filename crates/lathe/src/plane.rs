//! Where the profile sits in the world, and the plane it is edited on.
//!
//! The profile lives in (radius, height) and is turned about the vertical line through [`Plane::root`];
//! `scale` sizes the whole shape about that point. To edit it with a pointer the handles are laid on
//! the plane through the axis that faces the camera, on both sides of the axis, so dragging a handle
//! drags the outline you are looking at. A solid of revolution looks the same from every side, so the
//! plane simply turns with the view between drags (and is held still during one, so it cannot swing
//! under a handle).

use glam::{Mat4, Quat, Vec2, Vec3};
use sculpt_toolkit::View;

/// The overall size the Scale slider allows.
pub const MIN_SCALE: f32 = 0.1;
pub const MAX_SCALE: f32 = 5.0;

/// A ray this close to running along the plane cannot say where on it the pointer is.
const EDGE_ON: f32 = 0.02;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Plane {
    /// The profile's origin: on the axis, at height zero of the profile.
    pub root: Vec3,
    /// Uniform size of the whole shape.
    pub scale: f32,
    /// The horizontal direction, perpendicular to the axis, that profile radius runs along on the
    /// canonical side. Unit length.
    pub u: Vec3,
}

impl Plane {
    pub fn new(root: Vec3) -> Plane {
        Plane { root, scale: 1.0, u: Vec3::X }
    }

    /// Turns the plane to face `view`: for a perspective camera the plane through the axis that faces
    /// the camera's position (the true silhouette plane of a solid of revolution), for an
    /// orthographic one the plane facing the view direction.
    pub fn face(&mut self, view: &View) {
        let mut to_axis = if view.is_orthographic() { view.forward } else { self.root - view.eye };
        to_axis.y = 0.0;
        let mut u = to_axis.cross(Vec3::Y);
        if u.length_squared() < 1e-8 {
            // Looking straight down the axis every plane through it faces the camera equally, so keep
            // the handles on the screen's right.
            u = view.right;
            u.y = 0.0;
        }
        self.u = if u.length_squared() < 1e-8 { Vec3::X } else { u.normalize() };
    }

    /// The plane's normal, pointing at the camera it faces.
    pub fn normal(&self) -> Vec3 {
        self.u.cross(Vec3::Y)
    }

    /// A profile point in the world; `side` is 1 for the canonical side and -1 for its mirror image.
    pub fn world(&self, p: Vec2, side: f32) -> Vec3 {
        self.root + (self.u * (p.x * side) + Vec3::Y * p.y) * self.scale
    }

    /// Where a profile point is drawn, or `None` behind the camera.
    pub fn screen(&self, view: &View, p: Vec2, side: f32) -> Option<Vec2> {
        view.project(self.world(p, side))
    }

    /// The pointer on the plane as a SIGNED profile coordinate: x is negative on the mirrored side.
    /// `None` when the plane is edge-on to the view ray.
    pub fn cursor(&self, view: &View, px: Vec2) -> Option<Vec2> {
        let ray = view.ray(px);
        let normal = self.normal();
        let denominator = ray.dir.dot(normal);
        if denominator.abs() < EDGE_ON {
            return None;
        }
        let along = (self.root - ray.origin).dot(normal) / denominator;
        if along < 0.0 {
            return None;
        }
        let local = (ray.at(along) - self.root) / self.scale;
        Some(Vec2::new(local.dot(self.u), local.y))
    }

    /// Profile space (unscaled, axis on y) into the world: what a mesh built from the profile is drawn
    /// with.
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(Vec3::splat(self.scale), Quat::IDENTITY, self.root)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Vec2 = Vec2::new(800.0, 600.0);

    fn views() -> [View; 3] {
        [
            View::perspective(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60.0, SIZE),
            View::perspective(Vec3::new(4.0, 2.0, 3.0), Vec3::new(0.5, 0.0, 0.0), 45.0, SIZE),
            View::orthographic(Vec3::new(-3.0, 1.0, 4.0), Vec3::ZERO, 2.0, SIZE),
        ]
    }

    fn plane() -> Plane {
        Plane { root: Vec3::new(0.3, -0.5, 0.2), scale: 1.7, u: Vec3::X }
    }

    #[test]
    fn the_plane_faces_the_camera_whichever_way_it_looks() {
        for view in views() {
            let mut plane = plane();
            plane.face(&view);
            assert!((plane.u.length() - 1.0).abs() < 1e-5 && plane.u.y == 0.0);
            let towards_camera = if view.is_orthographic() { -view.forward } else { view.eye - plane.root };
            assert!(plane.normal().dot(towards_camera) > 0.0, "the plane's front faces the eye");
            // Radius runs to the screen's right, so the canonical side is the right-hand one.
            assert!(plane.u.dot(view.right) > 0.0);
        }
    }

    #[test]
    fn looking_straight_down_the_axis_keeps_the_handles_on_the_right() {
        let view = View::perspective(Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO, 60.0, SIZE);
        let mut plane = Plane::new(Vec3::ZERO);
        plane.face(&view);
        assert!(plane.u.dot(view.right) > 0.99 && plane.u.is_finite());
    }

    #[test]
    fn a_profile_point_drawn_at_a_pixel_is_found_there_again() {
        for view in views() {
            let mut plane = plane();
            plane.face(&view);
            for side in [1.0, -1.0] {
                for p in [Vec2::new(0.4, 0.0), Vec2::new(0.1, 0.9), Vec2::new(0.0, -0.3)] {
                    let px = plane.screen(&view, p, side).expect("in front of the camera");
                    let found = plane.cursor(&view, px).expect("the plane faces the camera");
                    let expected = Vec2::new(p.x * side, p.y);
                    assert!(found.distance(expected) < 1e-3, "{p} side {side}: {found}");
                }
            }
        }
    }

    #[test]
    fn a_plane_seen_edge_on_has_no_cursor() {
        let view = View::perspective(Vec3::new(0.0, 0.0, 5.0), Vec3::ZERO, 60.0, SIZE);
        let plane = Plane { root: Vec3::ZERO, scale: 1.0, u: Vec3::Z };
        assert!(plane.cursor(&view, SIZE * 0.5).is_none());
    }

    #[test]
    fn the_matrix_carries_a_built_mesh_to_where_the_handles_are() {
        let mut plane = plane();
        plane.face(&views()[1]);
        let p = Vec2::new(0.35, 0.6);
        let through_matrix = plane.matrix().transform_point3(Vec3::new(p.x, p.y, 0.0));
        // A mesh vertex built at angle zero lies along +x; the handle's canonical side is along u.
        let along_x = plane.world(p, 1.0) - plane.root;
        let around = |v: Vec3| Vec2::new(v.x, v.z).length();
        assert!(((through_matrix - plane.root).y - along_x.y).abs() < 1e-5);
        assert!((around(through_matrix - plane.root) - around(along_x)).abs() < 1e-5);
    }
}
