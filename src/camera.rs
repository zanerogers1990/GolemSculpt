//! Orbit camera. Behaviour follows the Unity app: no inertia or damping on
//! direct manipulation, zoom measured against the nearest visible surface so
//! the camera approaches it asymptotically, grab-style panning, and an
//! optional orbit/zoom anchor on the last sculpted point.

use glam::{Mat3, Mat4, Quat, Vec2, Vec3, Vec4};

use crate::mesh::bvh::Aabb;

pub const ORBIT_DEG_PER_PX: f32 = 0.25;
pub const WHEEL_ZOOM_STEP: f32 = 0.18;
pub const DRAG_ZOOM_PER_PX: f32 = 0.004;
const SNAP_SECONDS: f32 = 0.3;
const GLIDE_SECONDS: f32 = 0.35;

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Signed shortest difference between two angles in degrees.
fn delta_angle(from: f32, to: f32) -> f32 {
    let d = (to - from).rem_euclid(360.0);
    if d > 180.0 { d - 360.0 } else { d }
}

/// The camera's turn for a yaw and a pitch in degrees.
fn rotation_of(yaw: f32, pitch: f32) -> Quat {
    Quat::from_rotation_y(-yaw.to_radians()) * Quat::from_rotation_x(-pitch.to_radians())
}

// The projections use reverse depth (near plane = 1, far = 0) with a float
// depth buffer, which keeps precision even when zoomed right into a surface.

/// Right-handed view matrix from an orthonormal camera basis.
fn look_to(eye: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(right.x, up.x, -forward.x, 0.0),
        Vec4::new(right.y, up.y, -forward.y, 0.0),
        Vec4::new(right.z, up.z, -forward.z, 0.0),
        Vec4::new(-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
    )
}

/// Perspective projection with no far plane.
fn perspective_infinite_reverse(tan_half_fov: f32, aspect: f32, near: f32) -> Mat4 {
    let f = 1.0 / tan_half_fov;
    Mat4::from_cols(
        Vec4::new(f / aspect, 0.0, 0.0, 0.0),
        Vec4::new(0.0, f, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 0.0, -1.0),
        Vec4::new(0.0, 0.0, near, 0.0),
    )
}

/// Symmetric orthographic projection.
fn ortho_reverse(half_width: f32, half_height: f32, near: f32, far: f32) -> Mat4 {
    let range = far - near;
    Mat4::from_cols(
        Vec4::new(1.0 / half_width, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / half_height, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / range, 0.0),
        Vec4::new(0.0, 0.0, far / range, 1.0),
    )
}

struct Snap {
    from: (f32, f32),
    to: (f32, f32),
    t: f32,
    /// The point the turn goes about, when that is not the pivot.
    about: Option<Vec3>,
}

struct Glide {
    from: (Vec3, f32),
    to: (Vec3, f32),
    t: f32,
}

pub struct Camera {
    /// Degrees. Yaw 0 looks down -Z (front view); positive yaw turns the model
    /// to the right on screen.
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pivot: Vec3,
    pub fov_y: f32,
    pub ortho: bool,
    snap: Option<Snap>,
    glide: Option<Glide>,
}

/// Everything needed to turn pixels into rays and back for one frame.
#[derive(Clone, Copy)]
pub struct View {
    pub pos: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub forward: Vec3,
    pub view_proj: Mat4,
    pub view: Mat4,
    /// Viewport rectangle in physical pixels: origin and size.
    pub origin: Vec2,
    pub size: Vec2,
    pub tan_half_fov: f32,
    pub ortho_half_height: Option<f32>,
    /// The camera in world space (position, right, up, forward and
    /// orthographic half-height), which rays are built from.
    eye: (Vec3, Vec3, Vec3, Vec3, Option<f32>),
    /// World space into this view's space, when that is an object's own.
    to_local: Option<Mat4>,
    /// Corrects depth to size on screen in the space of an object stretched
    /// unevenly; 1 otherwise.
    depth_scale: f32,
}

impl View {
    /// The same view seen from the space of an object whose placement is
    /// `model`, so rays and projections work on that object's own
    /// coordinates. Sizes come out in the object's units; for an object
    /// stretched unevenly they use its average scale.
    pub fn local_to(&self, model: Mat4) -> View {
        let inverse = model.inverse();
        let linear = Mat3::from_mat4(model);
        let mean = linear.determinant().abs().cbrt().max(1e-9);
        // Planes facing the camera keep facing it: their normal goes through
        // the transpose, and its length is the scale along the view.
        let normal = linear.transpose() * self.forward;
        let along = normal.length().max(1e-9);
        let dir = |v: Vec3| inverse.transform_vector3(v).normalize_or(v);
        View {
            pos: inverse.transform_point3(self.pos),
            right: dir(self.right),
            up: dir(self.up),
            forward: normal / along,
            view_proj: self.view_proj * model,
            view: self.view * model,
            ortho_half_height: self.ortho_half_height.map(|h| h / mean),
            to_local: Some(inverse * self.to_local.unwrap_or(Mat4::IDENTITY)),
            depth_scale: self.depth_scale * along / mean,
            ..*self
        }
    }

    fn ndc(&self, px: Vec2) -> Vec2 {
        let uv = (px - self.origin) / self.size;
        Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0)
    }

    /// Ray through a pixel, in this view's space.
    pub fn ray(&self, px: Vec2) -> (Vec3, Vec3) {
        let ndc = self.ndc(px);
        let aspect = self.size.x / self.size.y;
        let (pos, right, up, forward, ortho) = self.eye;
        let (origin, dir) = match ortho {
            Some(h) => (pos + right * (ndc.x * h * aspect) + up * (ndc.y * h), forward),
            None => {
                let t = self.tan_half_fov;
                let dir = forward + right * (ndc.x * t * aspect) + up * (ndc.y * t);
                (pos, dir.normalize())
            }
        };
        match self.to_local {
            Some(m) => (m.transform_point3(origin), m.transform_vector3(dir).normalize_or(dir)),
            None => (origin, dir),
        }
    }

    /// Pixel position of a world point, or `None` if it is behind the camera.
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let clip = self.view_proj * p.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(self.origin + Vec2::new(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * self.size)
    }

    pub fn depth_of(&self, p: Vec3) -> f32 {
        (p - self.pos).dot(self.forward)
    }

    /// World units covered by one pixel at a given forward depth.
    pub fn world_per_pixel(&self, depth: f32) -> f32 {
        let half = self.ortho_half_height.unwrap_or(depth.max(1e-6) * self.tan_half_fov * self.depth_scale);
        2.0 * half / self.size.y
    }

    pub fn contains(&self, px: Vec2) -> bool {
        let p = px - self.origin;
        p.x >= 0.0 && p.y >= 0.0 && p.x < self.size.x && p.y < self.size.y
    }
}

impl Default for Camera {
    fn default() -> Self {
        Self {
            yaw: 0.0,
            pitch: 0.0,
            distance: 3.0,
            pivot: Vec3::ZERO,
            fov_y: 60.0,
            ortho: false,
            snap: None,
            glide: None,
        }
    }
}

impl Camera {
    pub fn rotation(&self) -> Quat {
        rotation_of(self.yaw, self.pitch)
    }

    pub fn position(&self) -> Vec3 {
        self.pivot + self.rotation() * Vec3::new(0.0, 0.0, self.distance)
    }

    pub fn tan_half_fov(&self) -> f32 {
        (self.fov_y.to_radians() * 0.5).tan()
    }

    pub fn is_animating(&self) -> bool {
        self.snap.is_some() || self.glide.is_some()
    }

    /// `view_depth` is the forward distance to the nearest visible surface
    /// (see [`Self::zoom`]); it sets the near plane so close-up work never
    /// clips. `scene_size` is the largest side of the scene bounds.
    pub fn view(&self, origin: Vec2, size: Vec2, view_depth: f32, scene_size: f32) -> View {
        let rot = self.rotation();
        let right = rot * Vec3::X;
        let up = rot * Vec3::Y;
        let forward = rot * Vec3::NEG_Z;
        let aspect = (size.x / size.y.max(1.0)).max(1e-3);
        let tan_half = self.tan_half_fov();

        let (pos, proj, ortho_half) = if self.ortho {
            // The ortho eye is parked well outside the scene so nothing is
            // ever behind it; only the half-height controls magnification.
            let back = 25.0 * scene_size.max(1e-4);
            let half = (self.distance * tan_half).max(1e-5);
            let far = (2.0 * (back.max(self.distance) + 2.0 * scene_size)).max(1000.0);
            let near = (back * 0.1).min(0.3);
            let proj = ortho_reverse(half * aspect, half, near, far);
            (self.pivot - forward * back, proj, Some(half))
        } else {
            let near = (view_depth * 0.02).clamp(1e-4, 0.3);
            let proj = perspective_infinite_reverse(tan_half, aspect, near);
            (self.pivot - forward * self.distance, proj, None)
        };
        let view = look_to(pos, right, up, forward);
        View {
            pos,
            right,
            up,
            forward,
            view_proj: proj * view,
            view,
            origin,
            size,
            tan_half_fov: tan_half,
            ortho_half_height: ortho_half,
            eye: (pos, right, up, forward, ortho_half),
            to_local: None,
            depth_scale: 1.0,
        }
    }

    fn limits(scene_size: f32) -> (f32, f32, f32) {
        let s = scene_size.max(1e-4);
        (0.05 * s, 20.0 * s, 0.001 * s)
    }

    /// Where the pivot goes when the view turns to `yaw` and `pitch` about
    /// `about`: round that point by the same turn, so the point stays on its
    /// pixel. With no such point the turn is about the pivot, which stays.
    fn pivot_turned(&self, yaw: f32, pitch: f32, about: Option<Vec3>) -> Vec3 {
        let Some(about) = about else { return self.pivot };
        about + rotation_of(yaw, pitch) * self.rotation().inverse() * (self.pivot - about)
    }

    fn turn_to(&mut self, yaw: f32, pitch: f32, about: Option<Vec3>) {
        self.pivot = self.pivot_turned(yaw, pitch, about);
        (self.yaw, self.pitch) = (yaw, pitch);
    }

    /// Orbits by a pixel delta. With an anchor, the anchor point stays on its
    /// pixel instead of the pivot.
    pub fn orbit(&mut self, delta: Vec2, anchor: Option<Vec3>) {
        self.snap = None;
        self.glide = None;
        let yaw = self.yaw + delta.x * ORBIT_DEG_PER_PX;
        let pitch = (self.pitch + delta.y * ORBIT_DEG_PER_PX).clamp(-89.0, 89.0);
        self.turn_to(yaw, pitch, anchor);
    }

    /// Moves the pivot along the line of sight until it is level with `at`,
    /// which changes nothing in the picture. The pivot is only where the
    /// last pan, zoom or anchored turn left it, and that can be well in front
    /// of the model or behind it; a turn about it, or a switch to
    /// orthographic (which shows at their true size the things level with
    /// it), then throws the model out of view. Called with a point on the
    /// model before either.
    pub fn seat_pivot(&mut self, at: Vec3) {
        let forward = self.rotation() * Vec3::NEG_Z;
        let shift = (at - self.pivot).dot(forward);
        if !shift.is_finite() {
            return;
        }
        if !self.ortho {
            // The eye stays where it is, so the distance takes up the move.
            // A point behind the eye is not one to look at.
            let distance = self.distance + shift;
            if distance < 1e-5 {
                return;
            }
            self.distance = distance;
        }
        self.pivot += forward * shift;
    }

    /// Grab-style pan: the surface under the cursor follows it 1:1.
    pub fn pan(&mut self, delta: Vec2, world_per_pixel: f32) {
        self.glide = None;
        let rot = self.rotation();
        self.pivot -= (rot * Vec3::X * delta.x - rot * Vec3::Y * delta.y) * world_per_pixel;
    }

    /// Zooms by `factor` (< 1 zooms in), measured against `view_depth`, the
    /// forward distance to the nearest surface in view (capped at the pivot
    /// distance). `anchor` optionally keeps a world point fixed on screen.
    pub fn zoom(&mut self, factor: f32, view_depth: f32, scene_size: f32, anchor: Option<Vec3>) {
        self.glide = None;
        let (min_d, max_d, min_gap) = Self::limits(scene_size);
        // Zooming in must never snap the camera back to a shrunken maximum.
        let upper = if factor < 1.0 { max_d.max(self.distance) } else { max_d };
        let before = self.distance;
        let rot = self.rotation();
        let forward = rot * Vec3::NEG_Z;
        let cam = self.position();

        if self.ortho {
            self.distance = (self.distance * factor).clamp(min_d.min(before), upper);
            if let Some(anchor) = anchor {
                let to = anchor - self.pivot;
                let sideways = to - forward * to.dot(forward);
                self.pivot += sideways * (1.0 - self.distance / before);
            }
            return;
        }

        let anchor_depth = anchor.map(|a| (a - cam).dot(forward)).filter(|d| *d > 1e-6);
        let depth = match anchor_depth {
            Some(a) => view_depth.min(a),
            None => view_depth,
        }
        .min(self.distance)
        .max(1e-6);
        let mut target = depth * factor;
        if factor < 1.0 && depth < self.distance {
            // A surface sits in front of the pivot: stop short of it.
            target = target.max(depth.min(min_gap));
        }
        let wanted = self.distance + (target - depth);
        self.distance = wanted.clamp(min_d.min(before), upper);
        let travel = before - self.distance;
        if let (Some(anchor), Some(anchor_depth)) = (anchor, anchor_depth) {
            // Dolly toward the anchor rather than the pivot, so it stays put.
            let to_anchor = anchor - cam;
            self.pivot += to_anchor * (travel / anchor_depth) - forward * travel;
        }
    }

    /// Animated move that fits `bounds` in view, keeping the current angle.
    pub fn frame(&mut self, bounds: Aabb, aspect: f32, animate: bool) {
        if bounds.is_empty() {
            return;
        }
        let radius = (bounds.size() * 0.5).length().max(1e-4);
        let half_v = self.fov_y.to_radians() * 0.5;
        let half_h = (half_v.tan() * aspect).atan();
        let fitted = radius * 1.1 / half_v.min(half_h).sin();
        let target = (bounds.center(), fitted.max(0.05 * radius));
        if animate {
            self.glide = Some(Glide { from: (self.pivot, self.distance), to: target, t: 0.0 });
        } else {
            self.pivot = target.0;
            self.distance = target.1;
            self.glide = None;
        }
    }

    /// The glide `frame` makes, aimed at a pivot and distance saved earlier:
    /// how leaving Local View goes back to where the view was. The viewing
    /// angle is kept.
    pub fn glide_to(&mut self, pivot: Vec3, distance: f32) {
        self.glide = Some(Glide { from: (self.pivot, self.distance), to: (pivot, distance.max(1e-5)), t: 0.0 });
    }

    /// Animated turn to an exact yaw/pitch (axis views), about `about` if
    /// there is one (see [`Self::orbit`]) and else about the pivot.
    pub fn snap_to(&mut self, yaw: f32, pitch: f32, about: Option<Vec3>) {
        self.snap = Some(Snap { from: (self.yaw, self.pitch), to: (yaw, pitch), t: 0.0, about });
    }

    /// True in the instant between a glide being asked for and its first
    /// step: the press that asked for it is still being handled.
    pub fn glide_is_new(&self) -> bool {
        self.glide.as_ref().is_some_and(|glide| glide.t == 0.0)
    }

    pub fn cancel_glide(&mut self) {
        self.glide = None;
    }

    /// True while an axis snap is turning the view. The turntable holds off
    /// while one runs, so the two do not fight over the yaw.
    pub fn is_snapping(&self) -> bool {
        self.snap.is_some()
    }

    /// Turns the view about the pivot by `degrees` of yaw: the turntable's
    /// spin. Unlike [`Self::orbit`] it never turns about the stroke anchor
    /// (the model stays where it is on screen and simply turns), and it
    /// leaves a glide in flight alone, so framing the model mid-spin works.
    pub fn add_yaw(&mut self, degrees: f32) {
        // Kept within one turn: left spinning for hours, the angle would
        // otherwise grow until it lost its precision.
        self.yaw = (self.yaw + degrees).rem_euclid(360.0);
    }

    /// Advances animations. Returns true while something is still moving.
    pub fn update(&mut self, dt: f32) -> bool {
        if let Some(mut snap) = self.snap.take() {
            snap.t += dt / SNAP_SECONDS;
            let k = smoothstep(snap.t);
            if snap.t >= 1.0 {
                self.turn_to(snap.to.0.rem_euclid(360.0), snap.to.1, snap.about);
            } else {
                let yaw = snap.from.0 + delta_angle(snap.from.0, snap.to.0) * k;
                self.turn_to(yaw, snap.from.1 + (snap.to.1 - snap.from.1) * k, snap.about);
                self.snap = Some(snap);
            }
        }
        if let Some(glide) = &mut self.glide {
            glide.t += dt / GLIDE_SECONDS;
            let k = smoothstep(glide.t);
            self.pivot = glide.from.0.lerp(glide.to.0, k);
            // Distance eases in log space so the zoom rate looks constant.
            self.distance = (glide.from.1.ln() + (glide.to.1.ln() - glide.from.1.ln()) * k).exp();
            if glide.t >= 1.0 {
                self.pivot = glide.to.0;
                self.distance = glide.to.1;
                self.glide = None;
            }
        }
        self.is_animating()
    }
}

/// A view as a `.sclp` file keeps it, so a sculpt reopens looked at from
/// where it was left.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SavedView {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub pivot: [f32; 3],
    pub ortho: bool,
    pub fov_y: f32,
}

impl Default for SavedView {
    fn default() -> Self {
        Camera::default().saved_view()
    }
}

impl Camera {
    /// The view to save. A glide or a turn that is under way is saved as
    /// where it is going, not the point it happens to have reached.
    pub fn saved_view(&self) -> SavedView {
        let (yaw, pitch) = self.snap.as_ref().map_or((self.yaw, self.pitch), |snap| snap.to);
        let about = self.snap.as_ref().and_then(|snap| snap.about);
        let turned = (self.pivot_turned(yaw, pitch, about), self.distance);
        let (pivot, distance) = self.glide.as_ref().map_or(turned, |glide| glide.to);
        SavedView { yaw, pitch, distance, pivot: pivot.to_array(), ortho: self.ortho, fov_y: self.fov_y }
    }

    /// Goes to a saved view at once. A view that makes no sense (a file that
    /// was damaged or edited by hand) is refused, and the camera stays put.
    pub fn restore_view(&mut self, saved: &SavedView) -> bool {
        let pivot = Vec3::from_array(saved.pivot);
        let numbers = [saved.yaw, saved.pitch, saved.distance, saved.fov_y];
        if !pivot.is_finite() || numbers.iter().any(|n| !n.is_finite()) || saved.distance <= 0.0 {
            return false;
        }
        self.snap = None;
        self.glide = None;
        self.yaw = saved.yaw;
        self.pitch = saved.pitch.clamp(-89.0, 89.0);
        self.distance = saved.distance.max(1e-5);
        self.pivot = pivot;
        self.ortho = saved.ortho;
        self.fov_y = saved.fov_y.clamp(10.0, 120.0);
        true
    }
}
