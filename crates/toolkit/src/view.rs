//! The camera as a tool sees it: pixels to rays, and world points to pixels.
//!
//! Positions on screen are physical pixels from the top-left corner of the
//! viewport, y down, which is what the pointer reports and what the overlay
//! draws in. Everything in the world is world space; a tool that works in an
//! object's own space moves a ray there with [`Ray::transformed`].

use glam::{Mat4, Vec2, Vec3, Vec4};

/// A half-line in space. `dir` is unit length.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Ray {
    pub origin: Vec3,
    pub dir: Vec3,
}

impl Ray {
    pub fn new(origin: Vec3, dir: Vec3) -> Ray {
        Ray { origin, dir: dir.normalize_or(Vec3::NEG_Z) }
    }

    /// The point `distance` along the ray.
    #[inline]
    pub fn at(&self, distance: f32) -> Vec3 {
        self.origin + self.dir * distance
    }

    /// The same ray in another space: pass the inverse of an object's matrix
    /// to get the ray in that object's own coordinates. Distances along the
    /// two rays differ when the matrix scales, so carry hit points back
    /// through the matrix rather than distances.
    pub fn transformed(&self, matrix: &Mat4) -> Ray {
        Ray::new(matrix.transform_point3(self.origin), matrix.transform_vector3(self.dir))
    }

    /// Where the ray crosses the plane through `point` with normal `normal`.
    /// `None` when it runs along the plane or the crossing is behind the
    /// origin.
    pub fn plane(&self, point: Vec3, normal: Vec3) -> Option<Vec3> {
        let along = self.dir.dot(normal);
        if along.abs() < 1e-9 {
            return None;
        }
        let distance = (point - self.origin).dot(normal) / along;
        (distance >= 0.0).then(|| self.at(distance))
    }

    /// How far `p` is from the ray, and how far along the ray its nearest
    /// point lies (never behind the origin).
    pub fn distance_to(&self, p: Vec3) -> (f32, f32) {
        let along = (p - self.origin).dot(self.dir).max(0.0);
        (self.at(along).distance(p), along)
    }
}

/// Everything needed to turn pixels into rays and back for one frame.
#[derive(Clone, Copy, Debug)]
pub struct View {
    /// Where the camera is. In an orthographic view it is parked well outside
    /// the scene, so only directions and [`View::world_per_pixel`] mean much.
    pub eye: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
    /// World space to clip space. Depth is reversed (near is 1), which only
    /// matters to code that reads the depth buffer.
    pub view_proj: Mat4,
    /// Viewport size in physical pixels.
    pub size: Vec2,
    pub tan_half_fov: f32,
    /// Half the world height the view covers, when it is orthographic.
    pub ortho_half_height: Option<f32>,
    /// Physical pixels per interface point. Multiply a size meant in points
    /// (a pick radius, a line width) by this to get pixels, so handles stay
    /// the same size under the finger whatever the interface scale.
    pub pixels_per_point: f32,
}

impl View {
    /// A perspective view from `eye` toward `target`, y up. For tests and
    /// demos; the app hands tools its own view.
    pub fn perspective(eye: Vec3, target: Vec3, fov_y_degrees: f32, size: Vec2) -> View {
        let (right, up, forward) = basis(eye, target);
        let tan_half_fov = (fov_y_degrees.to_radians() * 0.5).tan();
        let aspect = size.x / size.y.max(1.0);
        let near = 0.01;
        let f = 1.0 / tan_half_fov;
        // Reverse depth with no far plane, as the app's own projection.
        let proj = Mat4::from_cols(
            Vec4::new(f / aspect, 0.0, 0.0, 0.0),
            Vec4::new(0.0, f, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 0.0, -1.0),
            Vec4::new(0.0, 0.0, near, 0.0),
        );
        View {
            eye,
            right,
            up,
            forward,
            view_proj: proj * look_to(eye, right, up, forward),
            size,
            tan_half_fov,
            ortho_half_height: None,
            pixels_per_point: 1.0,
        }
    }

    /// An orthographic view from `eye` toward `target` covering
    /// `2 * half_height` world units top to bottom. For tests and demos.
    pub fn orthographic(eye: Vec3, target: Vec3, half_height: f32, size: Vec2) -> View {
        let (right, up, forward) = basis(eye, target);
        let aspect = size.x / size.y.max(1.0);
        let (near, far) = (0.01, 1000.0);
        let range = far - near;
        let proj = Mat4::from_cols(
            Vec4::new(1.0 / (half_height * aspect), 0.0, 0.0, 0.0),
            Vec4::new(0.0, 1.0 / half_height, 0.0, 0.0),
            Vec4::new(0.0, 0.0, 1.0 / range, 0.0),
            Vec4::new(0.0, 0.0, far / range, 1.0),
        );
        View {
            eye,
            right,
            up,
            forward,
            view_proj: proj * look_to(eye, right, up, forward),
            size,
            tan_half_fov: (30.0f32).to_radians().tan(),
            ortho_half_height: Some(half_height),
            pixels_per_point: 1.0,
        }
    }

    pub fn is_orthographic(&self) -> bool {
        self.ortho_half_height.is_some()
    }

    fn ndc(&self, px: Vec2) -> Vec2 {
        let uv = px / self.size;
        Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0)
    }

    /// The ray through a pixel.
    pub fn ray(&self, px: Vec2) -> Ray {
        let ndc = self.ndc(px);
        let aspect = self.size.x / self.size.y.max(1.0);
        match self.ortho_half_height {
            Some(h) => Ray {
                origin: self.eye + self.right * (ndc.x * h * aspect) + self.up * (ndc.y * h),
                dir: self.forward,
            },
            None => {
                let t = self.tan_half_fov;
                Ray::new(self.eye, self.forward + self.right * (ndc.x * t * aspect) + self.up * (ndc.y * t))
            }
        }
    }

    /// The pixel a world point lands on, or `None` when it is behind the
    /// camera. A point off the edge of the screen still projects.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let clip = self.view_proj * p.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * self.size)
    }

    /// How far in front of the camera a point is.
    pub fn depth_of(&self, p: Vec3) -> f32 {
        (p - self.eye).dot(self.forward)
    }

    /// World units covered by one pixel at a forward depth: the factor that
    /// turns a size in pixels into a size in the world there.
    pub fn world_per_pixel(&self, depth: f32) -> f32 {
        let half = self.ortho_half_height.unwrap_or(depth.max(1e-6) * self.tan_half_fov);
        2.0 * half / self.size.y.max(1.0)
    }

    /// Whether a pixel is inside the viewport.
    pub fn contains(&self, px: Vec2) -> bool {
        px.x >= 0.0 && px.y >= 0.0 && px.x < self.size.x && px.y < self.size.y
    }

    /// Where the ray through a pixel meets the plane that faces the camera
    /// and passes through `through`. This is where a drag goes when there is
    /// no surface under the pointer: the point slides at the depth it already
    /// had instead of jumping toward or away from the eye.
    pub fn on_facing_plane(&self, px: Vec2, through: Vec3) -> Option<Vec3> {
        self.ray(px).plane(through, -self.forward)
    }
}

/// Right, up and forward of a camera at `eye` looking at `target`, y up.
fn basis(eye: Vec3, target: Vec3) -> (Vec3, Vec3, Vec3) {
    let forward = (target - eye).normalize_or(Vec3::NEG_Z);
    // Looking straight up or down leaves no horizon to take "right" from.
    let seed = if forward.cross(Vec3::Y).length_squared() < 1e-8 { Vec3::NEG_Z } else { Vec3::Y };
    let right = forward.cross(seed).normalize();
    (right, right.cross(forward), forward)
}

/// Right-handed view matrix from an orthonormal camera basis.
fn look_to(eye: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(right.x, up.x, -forward.x, 0.0),
        Vec4::new(right.y, up.y, -forward.y, 0.0),
        Vec4::new(right.z, up.z, -forward.z, 0.0),
        Vec4::new(-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIZE: Vec2 = Vec2::new(1600.0, 960.0);

    fn views() -> [View; 3] {
        let eye = Vec3::new(1.5, 1.0, 4.0);
        [
            View::perspective(eye, Vec3::ZERO, 60.0, SIZE),
            View::orthographic(eye, Vec3::new(0.2, -0.1, 0.0), 2.0, SIZE),
            View::perspective(Vec3::new(0.0, 5.0, 0.0), Vec3::ZERO, 45.0, SIZE),
        ]
    }

    #[test]
    fn a_point_on_a_pixels_ray_projects_back_to_that_pixel() {
        for view in views() {
            for px in [Vec2::new(800.0, 480.0), Vec2::new(12.5, 30.0), Vec2::new(1590.0, 900.0), Vec2::new(400.0, 700.0)] {
                let ray = view.ray(px);
                for distance in [0.5, 3.0, 40.0] {
                    let back = view.project(ray.at(distance)).expect("in front of the camera");
                    assert!(back.distance(px) < 0.05, "{px} came back as {back}");
                }
            }
        }
    }

    #[test]
    fn the_centre_ray_runs_along_forward() {
        for view in views() {
            let ray = view.ray(SIZE * 0.5);
            assert!(ray.dir.dot(view.forward) > 0.999_99);
            assert!((ray.dir.length() - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn screen_axes_match_the_camera_basis() {
        for view in views() {
            let centre = view.eye + view.forward * 5.0;
            let c = view.project(centre).unwrap();
            let r = view.project(centre + view.right).unwrap();
            let u = view.project(centre + view.up).unwrap();
            assert!(r.x > c.x && (r.y - c.y).abs() < 1e-2, "right goes right");
            assert!(u.y < c.y && (u.x - c.x).abs() < 1e-2, "up goes up the screen");
        }
    }

    #[test]
    fn a_point_behind_a_perspective_camera_does_not_project() {
        let view = views()[0];
        assert!(view.project(view.eye - view.forward * 2.0).is_none());
        assert!(view.depth_of(view.eye - view.forward * 2.0) < 0.0);
    }

    #[test]
    fn world_per_pixel_is_the_size_of_a_pixel_there() {
        for view in views() {
            let depth = 5.0;
            let p = view.ray(Vec2::new(800.0, 480.0)).at(depth / view.ray(Vec2::new(800.0, 480.0)).dir.dot(view.forward));
            let step = view.world_per_pixel(view.depth_of(p));
            let moved = view.project(p + view.up * step * 10.0).unwrap();
            let here = view.project(p).unwrap();
            assert!(((here.y - moved.y) - 10.0).abs() < 0.05, "{}", here.y - moved.y);
        }
    }

    #[test]
    fn the_facing_plane_keeps_a_dragged_point_at_its_depth() {
        for view in views() {
            let through = Vec3::new(0.3, 0.2, -0.4);
            let p = view.on_facing_plane(Vec2::new(300.0, 200.0), through).unwrap();
            assert!((view.depth_of(p) - view.depth_of(through)).abs() < 1e-3);
            assert!(view.project(p).unwrap().distance(Vec2::new(300.0, 200.0)) < 0.05);
        }
    }

    #[test]
    fn a_ray_moved_into_an_objects_space_hits_the_same_point() {
        let model = Mat4::from_scale_rotation_translation(
            Vec3::new(2.0, 0.5, 1.5),
            glam::Quat::from_rotation_y(0.7),
            Vec3::new(1.0, -2.0, 0.5),
        );
        let ray = Ray::new(Vec3::new(0.0, 0.0, 5.0), Vec3::new(0.1, -0.3, -1.0));
        let local = ray.transformed(&model.inverse());
        // The object's own plane z = 0, hit in local space and carried back out.
        let hit_local = local.plane(Vec3::ZERO, Vec3::Z).or_else(|| local.plane(Vec3::ZERO, Vec3::NEG_Z)).unwrap();
        let hit_world = model.transform_point3(hit_local);
        let (off_ray, _) = ray.distance_to(hit_world);
        assert!(off_ray < 1e-4, "{off_ray}");
    }

    #[test]
    fn a_plane_behind_the_ray_is_not_hit() {
        let ray = Ray::new(Vec3::ZERO, Vec3::Z);
        assert!(ray.plane(Vec3::new(0.0, 0.0, -1.0), Vec3::Z).is_none());
        assert!(ray.plane(Vec3::new(0.0, 0.0, 1.0), Vec3::X).is_none());
        assert_eq!(ray.plane(Vec3::new(5.0, 5.0, 2.0), Vec3::Z), Some(Vec3::new(0.0, 0.0, 2.0)));
    }
}
