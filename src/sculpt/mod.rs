//! Stroke handling: turns pointer samples into evenly spaced, symmetric dabs.
//!
//! Dabs are paced by distance travelled, never by time, so a stroke looks the
//! same at any frame rate and tapers naturally at both ends. The pointer path
//! is joined with a centripetal Catmull-Rom spline, so a fast stroke is a
//! curve rather than a polygon.

pub mod dyntopo;
mod falloff;
mod input;
mod kernels;
mod pose;
mod radial;
mod symmetry;
#[cfg(test)]
mod symmetry_tests;
#[cfg(test)]
mod tests;

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use crate::camera::View;
use crate::mesh::{Mesh, StrokeDelta};
use crate::util::BitSet;
pub use falloff::{Falloff, FalloffCurve, FalloffPreset, focal_k, shifted};
use input::{LazyRope, PenPressure};
use kernels::{CurvatureBase, Dab, Params, SymOp};
pub use pose::{PoseMode, PoseSettings};
pub use symmetry::{Radial, RadialAxis};

#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
pub enum Brush {
    Move,
    Clay,
    Smooth,
    Crease,
    Inflate,
    Flatten,
    Pose,
    Standard,
    Layer,
    SnakeHook,
}

impl Brush {
    pub const ALL: [Brush; 10] = [
        Brush::Move,
        Brush::Clay,
        Brush::Smooth,
        Brush::Crease,
        Brush::Inflate,
        Brush::Flatten,
        Brush::Pose,
        Brush::Standard,
        Brush::Layer,
        Brush::SnakeHook,
    ];

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|&b| b == self).unwrap()
    }

    pub fn name(self) -> &'static str {
        match self {
            Brush::Move => "Move",
            Brush::Clay => "Clay",
            Brush::Smooth => "Smooth",
            Brush::Crease => "Crease",
            Brush::Inflate => "Inflate",
            Brush::Flatten => "Flatten",
            Brush::Pose => "Pose",
            Brush::Standard => "Standard",
            Brush::Layer => "Layer",
            Brush::SnakeHook => "Snake Hook",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Brush::Move => "Grab the surface under the ring and drag it.",
            Brush::Clay => "Build up or dig away material in flat-topped strips.",
            Brush::Smooth => "Relax the surface. Hold Shift with any brush for this.",
            Brush::Crease => "Cut a sharp groove and pinch its edges together.",
            Brush::Inflate => "Swell the surface outward along its own normals.",
            Brush::Flatten => "Level the surface to the average plane under the ring.",
            Brush::Pose => "Bend a limb at a joint the brush finds, like posing a figure.",
            Brush::Standard => "Raise a soft, rounded stroke.",
            Brush::Layer => "Raise an even step of fixed height.",
            Brush::SnakeHook => "Pull out long tendrils that follow the cursor.",
        }
    }

    /// Same number keys as the Unity app.
    pub fn hotkey(self) -> char {
        match self {
            Brush::Move => '1',
            Brush::Clay => '2',
            Brush::Smooth => '3',
            Brush::Crease => '4',
            Brush::Inflate => '5',
            Brush::Flatten => '6',
            Brush::Pose => '7',
            Brush::Standard => '8',
            Brush::Layer => '9',
            Brush::SnakeHook => '0',
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Brush::Move => "brush_move",
            Brush::Clay => "brush_clay",
            Brush::Smooth => "brush_smooth",
            Brush::Crease => "brush_crease",
            Brush::Inflate => "brush_inflate",
            Brush::Flatten => "brush_flatten",
            Brush::Pose => "brush_pose",
            Brush::Standard => "brush_standard",
            Brush::Layer => "brush_layer",
            Brush::SnakeHook => "brush_snakehook",
        }
    }

    /// Whether Ctrl / right button flips this brush.
    pub fn has_polarity(self) -> bool {
        !matches!(self, Brush::Smooth | Brush::Move | Brush::Pose | Brush::SnakeHook)
    }

    fn spacing(self) -> f32 {
        if self == Brush::Crease { 0.1 } else { 0.2 }
    }

    fn max_dabs_per_frame(self) -> usize {
        if self == Brush::Crease { 48 } else { 24 }
    }

    /// Standard and Clay place their dabs on the surface as it was when the
    /// stroke began; the others follow the live surface.
    fn uses_start_surface(self) -> bool {
        matches!(self, Brush::Standard | Brush::Clay)
    }
}

#[derive(Clone, Copy)]
pub struct BrushSettings {
    pub strength: f32,
    pub accumulate: bool,
    pub build_up: f32,
    pub positive: bool,
    pub focal_shift: f32,
    pub front_facing: bool,
}

pub struct Settings {
    pub brush: Brush,
    pub per_brush: [BrushSettings; 10],
    /// Each brush's hand-drawn falloff, or `None` while it keeps its own.
    pub falloff: [Option<FalloffCurve>; 10],
    pub mask_mode: bool,
    pub mask_hardness: f32,
    /// Brush radius in screen pixels; shared by every brush on purpose, so
    /// switching brush never resizes the ring behind your back.
    pub radius_px: f32,
    /// On (ZBrush's Draw Size): the brush is sized in screen pixels, so it
    /// stays the same size on screen as you zoom in for detail or out for
    /// big forms. Off: it has a fixed size on the model, `radius_world`.
    pub screen_space_size: bool,
    /// Brush radius in the model's own units. With Screen-Space Size on it
    /// follows `radius_px` from frame to frame (see [`Sculptor::sync_size`]);
    /// with it off this is the size, and `radius_px` follows it.
    pub radius_world: f32,
    /// Largest side of the model being sculpted, as of the last
    /// [`Sculptor::sync_size`]. The range of the fixed size scales with it,
    /// so a model a hundred units long gets a range as usable as the
    /// starting sphere's.
    model_size: f32,
    /// What one pixel covers on the model under the pointer, as of the same.
    world_per_pixel: f32,
    /// Move: grab only what is connected to the point under the pointer.
    pub move_connected: bool,
    /// The Pose brush's own settings.
    pub pose: PoseSettings,
    /// The mirror axes strokes are repeated across: X, Y, Z.
    pub symmetry: [bool; 3],
    /// Strokes repeated round an axis, on top of the mirror axes (see
    /// `symmetry.rs`, which makes the two into a stroke's list of copies).
    pub radial: Radial,
    /// `None` while the mirror planes and the radial axis are the sculpted
    /// object's own, through its origin. When they are the world's instead
    /// (its origin, its axes), this is where the object stands in it, which
    /// is what carries them into the mesh's space.
    pub symmetry_world: Option<sculpt_symmetry::Placement>,
    pub clay_depth: f32,
    pub clay_softness: f32,
    pub clay_relax: f32,
    pub crease_pinch: f32,
    pub crease_depth: f32,
    pub flatten_offset: f32,
    pub layer_height: f32,
    pub pressure_floor: f32,
    pub pressure_curve: f32,
    pub lazy_mouse: bool,
    pub lazy_radius: f32,
    pub lazy_strength: f32,
    pub build_up_on_hold: bool,
}

impl Default for Settings {
    fn default() -> Self {
        let base = BrushSettings {
            strength: 0.1,
            accumulate: false,
            build_up: 1.0,
            positive: true,
            focal_shift: 0.0,
            front_facing: false,
        };
        let mut per_brush = [base; 10];
        for b in [Brush::Clay, Brush::Inflate, Brush::Standard] {
            per_brush[b.index()].accumulate = true;
        }
        per_brush[Brush::Crease.index()].positive = false;
        Self {
            brush: Brush::Move,
            per_brush,
            falloff: Default::default(),
            mask_mode: false,
            mask_hardness: 0.5,
            radius_px: 60.0,
            screen_space_size: true,
            radius_world: 0.5,
            model_size: 1.0,
            world_per_pixel: 0.0,
            move_connected: false,
            pose: PoseSettings::default(),
            symmetry: [false; 3],
            radial: Radial::default(),
            symmetry_world: None,
            clay_depth: 0.3,
            clay_softness: 0.6,
            clay_relax: 0.35,
            crease_pinch: 0.6,
            crease_depth: 0.35,
            flatten_offset: 0.0,
            layer_height: 0.15,
            pressure_floor: 0.12,
            pressure_curve: 1.6,
            lazy_mouse: false,
            lazy_radius: 25.0,
            lazy_strength: 1.0,
            build_up_on_hold: false,
        }
    }
}

impl Settings {
    pub fn current(&self) -> &BrushSettings {
        &self.per_brush[self.brush.index()]
    }

    pub fn current_mut(&mut self) -> &mut BrushSettings {
        &mut self.per_brush[self.brush.index()]
    }

    /// What a stroke of `brush` weighs vertices with: its focal shift, and
    /// its hand-drawn curve if it has one.
    pub fn falloff_of(&self, brush: Brush) -> Falloff {
        let i = brush.index();
        Falloff::new(self.per_brush[i].focal_shift, self.falloff[i].as_ref())
    }

    /// Where the cursor draws `brush`'s inner ring, as a fraction of the
    /// radius: the distance at which the brush is at half weight.
    pub fn half_weight_radius(&self, brush: Brush) -> f32 {
        kernels::half_weight_radius(brush, &self.falloff_of(brush), self.clay_softness)
    }

    /// The fixed size's range, as fractions of the model's size. 0.01 is
    /// fine enough for detail with the camera right up against the surface;
    /// 8 reaches across a figure much bigger than the starting sphere.
    const WORLD_SIZE_RANGE: (f32, f32) = (0.01, 8.0);

    /// The brush size in the unit the user is working in: screen pixels with
    /// Screen-Space Size on, the model's units with it off. What the Size
    /// slider, S + drag and the scroll wheel all edit.
    pub fn size(&self) -> f32 {
        if self.screen_space_size { self.radius_px } else { self.radius_world }
    }

    /// The range [`Self::size`] may take. `viewport_height` is in pixels: the
    /// largest screen size is a share of it, so a bigger window is not capped
    /// at a brush that covers less of the screen.
    pub fn size_range(&self, viewport_height: f32) -> (f32, f32) {
        if self.screen_space_size {
            (2.0, (viewport_height * 600.0 / 1080.0).max(4.0))
        } else {
            let (min, max) = Self::WORLD_SIZE_RANGE;
            (min * self.model_size, max * self.model_size)
        }
    }

    pub fn set_size(&mut self, size: f32, viewport_height: f32) {
        let (min, max) = self.size_range(viewport_height);
        let size = size.clamp(min, max);
        if self.screen_space_size {
            self.radius_px = size;
        } else {
            self.radius_world = size;
            if self.world_per_pixel > 0.0 {
                self.radius_px = size / self.world_per_pixel;
            }
        }
    }

    /// How much [`Self::size`] changes per pixel of an S + drag.
    pub fn size_per_drag_px(&self) -> f32 {
        if self.screen_space_size { 0.5 } else { 0.01 * self.model_size }
    }

    /// For `--demo` screenshots: the environment variable `SCULPT_DEMO_BRUSH`
    /// sets the brushes up, for a look at what the strokes and the panel make
    /// of it. A comma-separated list of `focal=-0.8` (every brush's Focal
    /// Shift), `curve=plateau` (every brush's falloff curve, by the name of
    /// the shape it starts from) and `fixed` (Screen-Space Size off). The
    /// words about symmetry (`radial=6`, `axis=z`, `mirror=off`,
    /// `space=world`) are the sculpted object's to read: see
    /// `scene::Symmetry::demo_setup`.
    /// `strokes_done` is false before the scripted strokes and true after
    /// them: the fixed size waits until then, because they are sized in
    /// pixels.
    pub fn demo_setup(&mut self, strokes_done: bool) {
        let spec = std::env::var("SCULPT_DEMO_BRUSH").unwrap_or_default();
        for word in spec.split(',').map(str::trim) {
            match word.split_once('=') {
                Some(("focal", value)) if !strokes_done => {
                    let focal = value.parse::<f32>().unwrap_or(0.0).clamp(-1.0, 1.0);
                    self.per_brush.iter_mut().for_each(|b| b.focal_shift = focal);
                }
                Some(("curve", name)) if !strokes_done => {
                    let shape = FalloffPreset::ALL.into_iter().find(|p| p.name().eq_ignore_ascii_case(name));
                    self.falloff.iter_mut().for_each(|slot| *slot = shape.map(FalloffCurve::preset));
                }
                None if strokes_done && word == "fixed" => self.screen_space_size = false,
                _ => {}
            }
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct PointerSample {
    /// Physical pixels in the window.
    pub pos: Vec2,
    /// Pen pressure 0..1, or `None` for a mouse.
    pub pressure: Option<f32>,
    /// Seconds since the app started.
    pub time: f64,
}

#[derive(Clone, Copy)]
struct Knot {
    pos: Vec3,
    normal: Vec3,
    dir: Vec3,
    pressure: f32,
    px: Vec2,
}

struct Grab {
    /// Point on the surface where the drag started.
    origin: Vec3,
    /// In ascending order, which lets the drag move them on all threads.
    verts: Vec<u32>,
    /// `ops.len()` weights per vertex. Empty under radial symmetry, which
    /// keeps `held` instead.
    weights: Vec<f32>,
    /// Under radial symmetry: the copies that hold each vertex.
    held: Option<radial::Held>,
    leaves: Vec<u32>,
    dirty: bool,
}

struct Stroke {
    params: Params,
    /// Every copy the stroke is made as, the stroke itself first: the mirror
    /// images and the turns round the radial axis (see `symmetry.rs`).
    ops: Vec<SymOp>,
    /// The walk over those copies when there are turns among them. Without,
    /// they are mirror images and `Sculptor::apply_symmetric` walks them.
    radial: Option<radial::Walk>,
    /// The planes those mirror images lie across.
    mirrors: symmetry::Mirrors,
    knots: Vec<Knot>,
    /// Index of the last knot the dabs have reached.
    done: usize,
    carry: f32,
    dabs_this_frame: usize,
    travel_this_frame: f32,
    last_dab: Option<Vec3>,
    last_knot_time: f64,
    rope: LazyRope,
    pointer: Vec2,
    pressure: f32,
    time: f64,
    crease_n: Option<Vec3>,
    crease_d: Vec3,
    /// This frame's dab centres with their (mirrored) camera, for Clay relax.
    frame_centres: Vec<(Vec3, Vec3)>,
    grab: Option<Grab>,
    /// Snake hook: point being dragged.
    tip: Vec3,
    object_size: f32,
    relax: f32,
    /// What Clay's finishing pass measures curvature against.
    curvature: CurvatureBase,
    /// Saved positions for dabs near a mirror plane. Kept between dabs, like
    /// `scratch`: a fresh buffer per dab costs more than filling it.
    saved: [Vec<Vec3>; 2],
    scratch: kernels::Scratch,
}

#[derive(Default)]
pub struct Sculptor {
    pub settings: Settings,
    stroke: Option<Stroke>,
    /// Held between strokes: resetting it makes every stroke open with a
    /// spike of strength.
    pen: PenPressure,
    /// The curvature baseline of the meshes Clay has worked on, by
    /// `Mesh::uid`, newest last.
    curvature: Vec<(u64, CurvatureBase)>,
    /// The Pose brush's drag, and the preview of where it would bend.
    pose: pose::State,
    /// Last point sculpted, in world space; the camera can orbit around it.
    pub last_hit: Option<Vec3>,
    pub dabs_applied: u64,
    /// Dynamic topology: its settings, and how far the stroke has refined.
    pub dyntopo: dyntopo::DynTopo,
}

fn slerp_dir(a: Vec3, b: Vec3, t: f32) -> Vec3 {
    let d = a.dot(b).clamp(-1.0, 1.0);
    if d > 0.9995 || d < -0.9995 {
        return a.lerp(b, t).normalize_or(b);
    }
    let theta = d.acos();
    let s = theta.sin();
    (a * ((1.0 - t) * theta).sin() / s + b * (t * theta).sin() / s).normalize_or(b)
}

/// Centripetal Catmull-Rom (Barry-Goldman) between `p1` and `p2`.
fn catmull_rom(p0: Vec3, p1: Vec3, p2: Vec3, p3: Vec3, s: f32) -> Vec3 {
    let knot = |a: Vec3, b: Vec3| a.distance(b).sqrt().max(1e-6);
    let t0 = 0.0;
    let t1 = t0 + knot(p0, p1);
    let t2 = t1 + knot(p1, p2);
    let t3 = t2 + knot(p2, p3);
    let t = t1 + (t2 - t1) * s;
    let a1 = p0 * ((t1 - t) / (t1 - t0)) + p1 * ((t - t0) / (t1 - t0));
    let a2 = p1 * ((t2 - t) / (t2 - t1)) + p2 * ((t - t1) / (t2 - t1));
    let a3 = p2 * ((t3 - t) / (t3 - t2)) + p3 * ((t - t2) / (t3 - t2));
    let b1 = a1 * ((t2 - t) / (t2 - t0)) + a2 * ((t - t0) / (t2 - t0));
    let b2 = a2 * ((t3 - t) / (t3 - t1)) + a3 * ((t - t1) / (t3 - t1));
    b1 * ((t2 - t) / (t2 - t1)) + b2 * ((t - t1) / (t2 - t1))
}

/// Fraction of a disc of radius 1 still covered by a copy shifted `2u` away.
fn disc_overlap(u: f32) -> f32 {
    if u >= 1.0 {
        0.0
    } else {
        (2.0 / std::f32::consts::PI) * (u.acos() - u * (1.0 - u * u).sqrt())
    }
}

impl Sculptor {
    pub fn is_active(&self) -> bool {
        self.stroke.is_some()
    }

    /// While a stroke runs: where the brush actually is (the lazy-mouse rope
    /// end) and the frozen world radius, so the cursor never lies about where
    /// the next dab will land.
    pub fn stroke_cursor(&self) -> Option<(Vec2, f32, f32)> {
        self.stroke.as_ref().map(|s| (s.rope.pos, s.params.radius, s.params.sign))
    }

    /// Keeps the two brush sizes in step, once a frame. `world_per_pixel` is
    /// what one pixel covers at the surface under the pointer (or, off the
    /// model, at the nearest surface in view), in the model's own units.
    ///
    /// With Screen-Space Size on, the fixed size follows the ring, so
    /// switching it off leaves the brush the size it was showing. With it
    /// off, the ring follows the fixed size as the camera moves. Either way
    /// ticking the box never makes the ring jump.
    ///
    /// Nothing changes while a stroke runs: a stroke keeps one size from
    /// press to release, and a Clay build-up growing the model must not move
    /// the range under it.
    pub fn sync_size(&mut self, mesh: &Mesh, world_per_pixel: f32) {
        if self.stroke.is_some() || !(world_per_pixel > 0.0 && world_per_pixel.is_finite()) {
            return;
        }
        let s = &mut self.settings;
        let bounds = mesh.bounds();
        s.model_size = if bounds.is_empty() { 1.0 } else { bounds.size().max_element().max(1e-4) };
        s.world_per_pixel = world_per_pixel;
        if s.screen_space_size {
            s.radius_world = s.radius_px * world_per_pixel;
        } else {
            // Back inside the range: the model changed, or the size came
            // from the ring and the ring's range is far wider.
            let (min, max) = Settings::WORLD_SIZE_RANGE;
            s.radius_world = s.radius_world.clamp(min * s.model_size, max * s.model_size);
            s.radius_px = s.radius_world / world_per_pixel;
        }
    }

    /// The mesh's curvature baseline for Clay's finishing pass, measured the
    /// first time Clay touches a mesh and kept from then on. Kept, because
    /// it is a pass over the whole mesh, and because a baseline that moved
    /// with every stroke would have the pass treat untouched surface
    /// differently from one stroke to the next.
    fn curvature_base(&mut self, mesh: &mut Mesh) -> CurvatureBase {
        if let Some(&(_, base)) = self.curvature.iter().find(|(uid, _)| *uid == mesh.uid) {
            return base;
        }
        // The measure reads the normals, which wait for a frame to end.
        mesh.flush_normals();
        let base = kernels::curvature_base(mesh);
        if self.curvature.len() >= 32 {
            self.curvature.remove(0);
        }
        self.curvature.push((mesh.uid, base));
        base
    }

    /// Brush that a stroke started now would use.
    pub fn effective_brush(&self, smooth_held: bool) -> Brush {
        if smooth_held && !self.settings.mask_mode { Brush::Smooth } else { self.settings.brush }
    }

    /// Whether a stroke begun on `mesh` now could add triangles to it: see
    /// [`dyntopo::DynTopo::may_grow`]. `smooth_held` as for [`Self::begin`].
    pub fn may_refine(&self, mesh: &Mesh, smooth_held: bool) -> bool {
        let brush = self.effective_brush(smooth_held);
        self.dyntopo.may_grow(brush, self.settings.mask_mode, mesh.triangle_count())
    }

    fn shape_pressure(&mut self, sample: &PointerSample) -> f32 {
        self.pen.step(sample.pressure, sample.time);
        if sample.pressure.is_none() {
            return 1.0;
        }
        // An exponent above 1 keeps the response gentle near zero, which is
        // the noisiest part of a pen's sensor.
        let s = &self.settings;
        s.pressure_floor
            + (1.0 - s.pressure_floor) * self.pen.smoothed.clamp(0.0, 1.0).powf(s.pressure_curve)
    }

    /// Starts a stroke if the pointer is over the mesh. Returns whether it did.
    pub fn begin(
        &mut self,
        mesh: &mut Mesh,
        view: &View,
        sample: PointerSample,
        invert: bool,
        smooth_held: bool,
    ) -> bool {
        if self.stroke.is_some() || mesh.vertex_count() == 0 {
            return false;
        }
        let (origin, dir) = view.ray(sample.pos);
        let Some(hit) = mesh.raycast(origin, dir) else { return false };

        let brush = self.effective_brush(smooth_held);
        let mask = self.settings.mask_mode;
        let bs = self.settings.per_brush[brush.index()];
        // Frozen here for the whole stroke: re-deriving it would change the
        // footprint every time the surface under the pointer came nearer or
        // went further (up a Clay build-up's own rising wall, for one).
        let radius = if self.settings.screen_space_size {
            self.settings.radius_px * view.world_per_pixel(view.depth_of(hit.pos))
        } else {
            self.settings.radius_world
        };
        let mut sign = if bs.positive || mask { 1.0 } else { -1.0 };
        if invert && (mask || brush.has_polarity()) {
            sign = -sign;
        }
        let s = &self.settings;
        let params = Params {
            brush,
            mask,
            sign,
            radius,
            strength: bs.strength,
            accumulate: bs.accumulate,
            build_up: bs.build_up,
            // Mask painting has its Hardness instead of a shift or a curve.
            falloff: if mask { Falloff::BUILT_IN } else { self.settings.falloff_of(brush) },
            front_facing: bs.front_facing,
            clay_depth: s.clay_depth,
            clay_soft: s.clay_softness,
            crease_pinch: s.crease_pinch,
            crease_depth: s.crease_depth,
            flatten_offset: s.flatten_offset,
            layer_height: s.layer_height,
            mask_hardness: s.mask_hardness,
        };
        let pressure = self.shape_pressure(&sample);
        let bounds = mesh.bounds();
        self.dyntopo.begin(brush, mask, bounds.size().max_element());
        let relax = self.settings.clay_relax;
        let curvature = if !mask && brush == Brush::Clay && relax > 0.0 {
            self.curvature_base(mesh)
        } else {
            CurvatureBase::default()
        };
        // The rope starts under the pointer, not where the last stroke left it.
        let mut rope = LazyRope::default();
        let lazy = self.settings.lazy_mouse && (mask || !matches!(brush, Brush::Move | Brush::Pose));
        rope.advance(sample.pos, 0.0, true, lazy, self.settings.lazy_radius, self.settings.lazy_strength);
        mesh.begin_stroke();
        let group = self.settings.symmetry_group();
        let ops = symmetry::copies(&group);
        let mut stroke = Stroke {
            params,
            mirrors: symmetry::Mirrors::of(&self.settings, &ops),
            ops,
            radial: radial::Walk::new(group),
            knots: Vec::new(),
            done: 0,
            carry: 0.0,
            dabs_this_frame: 0,
            travel_this_frame: 0.0,
            last_dab: None,
            last_knot_time: sample.time,
            rope,
            pointer: sample.pos,
            pressure,
            time: sample.time,
            crease_n: None,
            crease_d: Vec3::ZERO,
            frame_centres: Vec::new(),
            grab: None,
            tip: hit.pos,
            object_size: bounds.size().max_element().max(1e-6),
            relax,
            curvature,
            saved: [Vec::new(), Vec::new()],
            scratch: kernels::Scratch::default(),
        };
        self.last_hit = Some(hit.pos);
        if !mask && brush == Brush::Move {
            let connected = self.settings.move_connected;
            stroke.grab = Some(Self::grab(mesh, &stroke, hit.pos, view.pos, connected));
        }
        if !mask && brush == Brush::Pose {
            // Ctrl or the right button: twist (or slide) instead.
            let (pose, falloff) = (self.settings.pose, stroke.params.falloff);
            self.pose.begin(mesh, &pose, falloff, &stroke.ops, hit.pos, radius, invert, view, sample.pos);
        }
        self.stroke = Some(stroke);
        if mask || !matches!(brush, Brush::Move | Brush::Pose | Brush::SnakeHook) {
            self.sample(mesh, view, sample);
        }
        true
    }

    /// Captures the vertices under the ring for a Move drag. The set and its
    /// weights stay fixed for the whole drag. `connected` keeps only the
    /// piece of surface under the pointer (see [`keep_connected`]).
    fn grab(mesh: &Mesh, stroke: &Stroke, hit: Vec3, cam: Vec3, connected: bool) -> Grab {
        let p = &stroke.params;
        let r = p.radius;
        let n_ops = stroke.ops.len();
        // Each copy of the brush: its centre, and the camera as it sees it.
        let copies: Vec<(Vec3, Vec3)> =
            stroke.ops.iter().map(|op| (op.apply_point(hit), op.apply_point(cam))).collect();
        let mut leaves = Vec::new();
        for &(centre, _) in &copies {
            mesh.bvh.leaves_in_sphere(centre, r, &mut leaves);
        }
        leaves.sort_unstable();
        leaves.dedup();
        let weight = |v: u32, (centre, cam): (Vec3, Vec3)| {
            let i = v as usize;
            let pos = mesh.positions[i];
            let d2 = pos.distance_squared(centre);
            if d2 >= r * r {
                return 0.0;
            }
            // Front Facing is the same soft ramp every other brush uses. A
            // hard cut gave the grabbed region a sawtooth edge wherever the
            // silhouette crossed it, and the drag tore at exactly the thin,
            // strongly curved forms the option is there to protect.
            let w = p.falloff.at(d2.sqrt() / r, kernels::smooth)
                * (1.0 - mesh.mask[i])
                * kernels::facing(p.front_facing, mesh.normals[i], pos, cam);
            w.max(0.0)
        };
        if stroke.radial.is_some() {
            // Many copies, each reaching few of the vertices: kept by vertex
            // as the copies that hold it, not as a weight for every copy.
            let (verts, held) = radial::grab(mesh, &copies, r, connected, weight);
            let leaves = mesh.leaves_of(&verts);
            return Grab { origin: hit, verts, weights: Vec::new(), held: Some(held), leaves, dirty: false };
        }
        // Every vertex some copy pulls on, then each copy's pull on it.
        let mut verts = kernels::collect(mesh, &leaves, |v| {
            copies.iter().any(|&copy| weight(v, copy) > 0.0).then_some(v)
        });
        let mut weights = vec![0.0f32; verts.len() * n_ops];
        weights.par_chunks_mut(n_ops).zip(verts.par_iter()).with_min_len(1024).for_each(
            |(row, &v)| {
                for (w, &copy) in row.iter_mut().zip(&copies) {
                    *w = weight(v, copy);
                }
            },
        );
        if connected && verts.len() > 1 {
            let centres: Vec<Vec3> = copies.iter().map(|&(centre, _)| centre).collect();
            keep_connected(mesh, &centres, &mut verts, &mut weights);
        }
        let leaves = mesh.leaves_of(&verts);
        Grab { origin: hit, verts, weights, held: None, leaves, dirty: false }
    }

    /// Feeds one pointer sample to the active stroke.
    pub fn sample(&mut self, mesh: &mut Mesh, view: &View, sample: PointerSample) {
        let pressure = self.shape_pressure(&sample);
        let lazy = (self.settings.lazy_mouse, self.settings.lazy_radius, self.settings.lazy_strength);
        let Some(stroke) = self.stroke.as_mut() else { return };
        let dt = (sample.time - stroke.time).max(0.0) as f32;
        stroke.time = sample.time;
        stroke.pressure = pressure;

        let p = stroke.params;
        let is_move = !p.mask && matches!(p.brush, Brush::Move | Brush::Pose);
        // Lazy mouse: the brush trails the pointer on a rope of fixed length.
        // Not for Move or Pose, which follow the pointer 1:1 by design.
        let px = stroke.rope.advance(sample.pos, dt, true, lazy.0 && !is_move, lazy.1, lazy.2);
        stroke.pointer = sample.pos;
        let (origin, dir) = view.ray(px);

        if is_move {
            // Applied once per frame in `frame`; only the latest position matters.
            if let Some(g) = stroke.grab.as_mut() {
                g.dirty = true;
            }
            self.pose.touch();
            return;
        }

        if !p.mask && p.brush == Brush::SnakeHook {
            // Drag plane faces the camera and passes through the moving tip.
            let denom = dir.dot(view.forward);
            if denom.abs() < 1e-6 {
                return;
            }
            let t = (stroke.tip - origin).dot(view.forward) / denom;
            let delta = origin + dir * t - stroke.tip;
            let len = delta.length();
            if len < 1e-9 {
                return;
            }
            let steps = ((len / (0.15 * p.radius)).ceil() as usize).clamp(1, 64);
            let step = delta / steps as f32;
            for _ in 0..steps {
                let dab = Dab {
                    center: stroke.tip,
                    normal: -view.forward,
                    dir: Vec3::ZERO,
                    step,
                    prev: None,
                    cam: view.pos,
                    pressure,
                };
                Self::apply_symmetric(mesh, stroke, dab);
                stroke.tip += step;
                self.dabs_applied += 1;
            }
            self.last_hit = Some(stroke.tip);
            return;
        }

        let from_start = !p.mask && p.brush.uses_start_surface();
        let hit = if from_start { mesh.raycast_start(origin, dir) } else { mesh.raycast(origin, dir) };
        let Some(hit) = hit else {
            // Off the mesh: the stroke pauses and restarts with a fresh first
            // dab when the pointer comes back (still the same undo step).
            Self::advance(mesh, stroke, true, view.pos, &mut self.dabs_applied);
            stroke.knots.clear();
            stroke.done = 0;
            stroke.carry = 0.0;
            stroke.last_dab = None;
            stroke.crease_n = None;
            self.dyntopo.pause();
            return;
        };
        self.last_hit = Some(hit.pos);

        if let Some(last) = stroke.knots.last() {
            if last.px.distance(px) < 2.0 {
                return;
            }
        }
        let normal = mesh.hit_normal(&hit, from_start);
        let mut knot = Knot { pos: hit.pos, normal, dir: Vec3::ZERO, pressure, px };
        if !p.mask && p.brush == Brush::Crease {
            // The groove's frame follows the stroke with a little lag so it
            // neither jitters nor chases its own wall.
            let foot = kernels::footprint_normal(mesh, hit.pos, p.radius, false).unwrap_or(normal);
            let n = match stroke.crease_n {
                Some(prev) => slerp_dir(prev, foot, 0.35),
                None => foot,
            };
            if let Some(last) = stroke.knots.last() {
                let travel = hit.pos - last.pos;
                let tangential = travel - n * travel.dot(n);
                if tangential.length() > 0.25 * p.spacing() * p.radius {
                    let t = tangential.normalize();
                    stroke.crease_d =
                        if stroke.crease_d == Vec3::ZERO { t } else { slerp_dir(stroke.crease_d, t, 0.35) };
                }
            }
            let d = stroke.crease_d;
            stroke.crease_d = (d - n * d.dot(n)).normalize_or_zero();
            stroke.crease_n = Some(n);
            knot.normal = n;
            knot.dir = stroke.crease_d;
        }
        // Dynamic topology goes first, so the dabs land on the new vertices.
        // Every symmetric copy of the brush refines where its dabs will go.
        self.dyntopo.refine(mesh, hit.pos, p.radius, &|at, copies| {
            copies.extend(stroke.ops.iter().map(|op| op.apply_point(at)));
        });
        stroke.knots.push(knot);
        stroke.last_knot_time = sample.time;
        Self::advance(mesh, stroke, false, view.pos, &mut self.dabs_applied);
    }

    /// Lays dabs along every path segment that is ready. The newest segment
    /// waits for the next knot (the spline needs it) unless `flush` is set.
    fn advance(mesh: &mut Mesh, stroke: &mut Stroke, flush: bool, cam: Vec3, count: &mut u64) {
        let n = stroke.knots.len();
        if n == 0 {
            return;
        }
        if stroke.last_dab.is_none() && stroke.done == 0 {
            // A stroke opens with a dab right where it lands.
            let k = stroke.knots[0];
            Self::emit(mesh, stroke, k.pos, k.normal, k.dir, k.pressure, cam, count);
        }
        let spacing = (stroke.params.radius * stroke.params.spacing()).max(0.0005);
        let cap = stroke.params.max_dabs_per_frame();
        while stroke.done + 1 < n && (flush || stroke.done + 2 < n) {
            let i = stroke.done;
            let k1 = stroke.knots[i];
            let k2 = stroke.knots[i + 1];
            let p0 = if i > 0 { stroke.knots[i - 1].pos } else { k1.pos * 2.0 - k2.pos };
            let p3 = if i + 2 < n { stroke.knots[i + 2].pos } else { k2.pos * 2.0 - k1.pos };
            let chord = k1.pos.distance(k2.pos);
            let pieces = ((chord / (spacing * 0.25)).ceil() as usize).clamp(2, 64);
            let mut prev = k1.pos;
            for piece in 1..=pieces {
                let s = piece as f32 / pieces as f32;
                let q = catmull_rom(p0, k1.pos, k2.pos, p3, s);
                let len = prev.distance(q);
                stroke.travel_this_frame += len;
                let mut walked = 0.0;
                while len > 0.0 && stroke.carry + (len - walked) >= spacing {
                    walked += (spacing - stroke.carry).max(0.0);
                    stroke.carry = 0.0;
                    if stroke.dabs_this_frame >= cap {
                        // Over budget: drop the travel. Banking it would
                        // release as a burst of dabs next frame.
                        continue;
                    }
                    let f = walked / len;
                    let t = (piece as f32 - 1.0 + f) / pieces as f32;
                    let pos = prev.lerp(q, f);
                    let normal = slerp_dir(k1.normal, k2.normal, t);
                    let pressure = k1.pressure + (k2.pressure - k1.pressure) * t;
                    Self::emit(mesh, stroke, pos, normal, k2.dir, pressure, cam, count);
                }
                stroke.carry += len - walked;
                prev = q;
            }
            stroke.done += 1;
        }
        // Only the last few knots are ever needed again.
        if stroke.done > 2 {
            let drop = stroke.done - 2;
            stroke.knots.drain(..drop);
            stroke.done -= drop;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn emit(
        mesh: &mut Mesh,
        stroke: &mut Stroke,
        pos: Vec3,
        normal: Vec3,
        dir: Vec3,
        pressure: f32,
        cam: Vec3,
        count: &mut u64,
    ) {
        let spacing = (stroke.params.radius * stroke.params.spacing()).max(0.0005);
        // After a gap (fast stroke, dropped travel) sweep from one spacing
        // back rather than across the whole gap.
        let prev = stroke.last_dab.map(|p| {
            if p.distance(pos) > 1.5 * spacing { pos + (p - pos).normalize() * spacing } else { p }
        });
        let dab = Dab { center: pos, normal, dir, step: Vec3::ZERO, prev, cam, pressure };
        Self::apply_symmetric(mesh, stroke, dab);
        stroke.last_dab = Some(pos);
        stroke.dabs_this_frame += 1;
        *count += 1;
    }

    /// Applies a dab and its copies under the stroke's symmetry.
    ///
    /// Copies that are far apart cannot interact and are simply applied in
    /// turn. Copies that overlap (a dab near a mirror plane) would give a
    /// result that depends on which went first, and that bias accumulates into
    /// visible drift of the centre line. So a pair is applied in both orders
    /// from the same starting state and averaged, which is symmetric by
    /// construction.
    ///
    /// Near two or three planes at once there are four or eight copies, too
    /// many orderings to average over. They are each applied to the same
    /// starting state instead, and their moves added up and shared out by how
    /// much the copies overlap, as `radial.rs` does for the copies round an
    /// axis and for the same reasons (a brush that holds a height included).
    ///
    /// That is the walk over mirror images, the same whether the planes are
    /// the object's own or the world's (`symmetry::Mirrors` says which copy
    /// lies across which plane, and how near a dab is to each). With turns
    /// round a radial axis among the copies they overlap near the axis
    /// instead, many at a time, and `radial::Walk` has the walk for that.
    fn apply_symmetric(mesh: &mut Mesh, stroke: &mut Stroke, dab: Dab) {
        let p = stroke.params;
        let reach = p.reach(&dab);
        if !p.mask && p.brush == Brush::Clay {
            for &op in &stroke.ops {
                stroke.frame_centres.push((op.apply_point(dab.center), op.apply_point(dab.cam)));
            }
        }
        if let Some(walk) = stroke.radial.as_mut() {
            return walk.apply(mesh, &p, &dab, reach, &mut stroke.scratch);
        }

        let mirrors = &stroke.mirrors;
        let near = mirrors.near(dab.center, 1.5 * reach);

        let mut leaves: Vec<u32> = Vec::new();
        let mut handled = vec![false; stroke.ops.len()];
        for i in 0..stroke.ops.len() {
            if handled[i] {
                continue;
            }
            // Group: copies that differ from this one only across near planes.
            let group: Vec<SymOp> = stroke
                .ops
                .iter()
                .enumerate()
                .filter(|&(j, _)| {
                    let same = (mirrors.across(j) ^ mirrors.across(i)) & !near == 0;
                    if same {
                        handled[j] = true;
                    }
                    same
                })
                .map(|(_, &o)| o)
                .collect();

            leaves.clear();
            for &op in &group {
                mesh.bvh.leaves_in_sphere(op.apply_point(dab.center), reach, &mut leaves);
            }
            // Ascending, each leaf once: the order the saved copies below are
            // laid out in, and what lets a dab's vertices be shared out
            // between threads.
            leaves.sort_unstable();
            leaves.dedup();
            if leaves.is_empty() {
                continue;
            }

            if group.len() == 1 || p.mask {
                // Mask amounts all have the same sign and saturate, so order
                // does not matter there.
                for &op in &group {
                    kernels::apply(mesh, &p, &dab.mirrored(op), &leaves, &mut stroke.scratch);
                }
            } else {
                let [saved, other] = &mut stroke.saved;
                let scratch = &mut stroke.scratch;
                save(mesh, &leaves, saved);
                if group.len() == 2 {
                    kernels::apply(mesh, &p, &dab.mirrored(group[0]), &leaves, scratch);
                    kernels::apply(mesh, &p, &dab.mirrored(group[1]), &leaves, scratch);
                    save(mesh, &leaves, other);
                    restore(mesh, &leaves, saved);
                    kernels::apply(mesh, &p, &dab.mirrored(group[1]), &leaves, scratch);
                    kernels::apply(mesh, &p, &dab.mirrored(group[0]), &leaves, scratch);
                    let first = &*other;
                    mesh.move_vertices(&leaves, |at, vt| Some((first[at] + vt.pos) * 0.5));
                } else {
                    // Near two or three planes at once: apply every copy to
                    // the same starting state and share out the sum.
                    let sum = other;
                    sum.clear();
                    sum.resize(saved.len(), Vec3::ZERO);
                    // A brush that holds a height is also held to the furthest
                    // any one copy moved a vertex (see `held_to_furthest`). A
                    // quarter radius from two planes all four copies cover the
                    // middle while the set counts as under three, and Layer
                    // came up 1.37 times its height there.
                    let holds = p.holds_a_height();
                    scratch.furthest.clear();
                    scratch.furthest.resize(if holds { saved.len() } else { 0 }, 0.0);
                    for (n, &op) in group.iter().enumerate() {
                        if n > 0 {
                            restore(mesh, &leaves, saved);
                        }
                        kernels::apply(mesh, &p, &dab.mirrored(op), &leaves, scratch);
                        add_moves(mesh, &leaves, saved, sum);
                        if holds {
                            note_furthest(mesh, &leaves, saved, &mut scratch.furthest);
                        }
                    }
                    let multiplicity: f32 = group
                        .iter()
                        .map(|&op| {
                            let image = op.apply_point(group[0].apply_point(dab.center));
                            disc_overlap(dab.center.distance(image) / (2.0 * reach))
                        })
                        .sum();
                    let scale = 1.0 / multiplicity.max(1.0);
                    restore(mesh, &leaves, saved);
                    let (saved, sum, furthest) = (&*saved, &*sum, &*scratch.furthest);
                    mesh.move_vertices(&leaves, |at, _| {
                        let step = sum[at] * scale;
                        Some(saved[at] + if holds { kernels::held_to_furthest(step, furthest[at]) } else { step })
                    });
                }
            }

            if p.mask {
                mesh.mask_changed(&leaves);
            } else {
                mesh.positions_changed(&leaves);
            }
        }
    }

    /// Per-frame work: Move drags, brushes that keep working while the pointer
    /// is held still, the pause flush, and Clay's finishing pass.
    pub fn frame(&mut self, mesh: &mut Mesh, view: &View, dt: f32) {
        let build_up_on_hold = self.settings.build_up_on_hold;
        let Some(stroke) = self.stroke.as_mut() else { return };
        let p = stroke.params;

        if let Some(grab) = stroke.grab.as_mut().filter(|g| g.dirty) {
            grab.dirty = false;
            let (origin, dir) = view.ray(stroke.pointer);
            let denom = dir.dot(view.forward);
            if denom.abs() > 1e-6 {
                let t = (grab.origin - origin).dot(view.forward) / denom;
                let delta = origin + dir * t - grab.origin;
                // The pointer's motion as each copy makes it.
                let pulls: Vec<Vec3> = stroke.ops.iter().map(|op| op.apply_dir(delta)).collect();
                let (n_ops, weights, held) = (pulls.len(), &grab.weights, &grab.held);
                mesh.move_listed(&grab.verts, |slot, vt| {
                    if let Some(held) = held {
                        return Some(vt.start + held.pull(slot, &pulls));
                    }
                    let w = &weights[slot * n_ops..(slot + 1) * n_ops];
                    let mut disp = Vec3::ZERO;
                    let mut total = 0.0;
                    for (k, &pull) in pulls.iter().enumerate() {
                        disp += pull * w[k];
                        total += w[k];
                    }
                    // Where mirror copies overlap, share the pull so a vertex
                    // on the plane follows the in-plane motion 1:1.
                    if total > 1.0 {
                        disp /= total;
                    }
                    // Always re-derived from the stroke-start position, so
                    // nothing drifts however long the drag lasts.
                    Some(vt.start + disp)
                });
                mesh.positions_changed(&grab.leaves);
                self.last_hit = Some(grab.origin + delta);
            }
        }
        if let Some(tip) = self.pose.frame(mesh, &stroke.ops, view, stroke.pointer) {
            self.last_hit = Some(tip);
        }

        let dab_brush = p.mask || !matches!(p.brush, Brush::Move | Brush::Pose | Brush::SnakeHook);
        if dab_brush {
            // Detail a busy frame left unfinished, while the pointer rests.
            self.dyntopo.frame(mesh, p.radius, &|at, copies| {
                copies.extend(stroke.ops.iter().map(|op| op.apply_point(at)));
            });
            // A short pause releases the segment that was waiting for a knot.
            if stroke.time - stroke.last_knot_time > 0.05 || dt > 0.05 {
                Self::advance(mesh, stroke, true, view.pos, &mut self.dabs_applied);
            }
            // Smoothing, flattening, layering and masking keep working while
            // the pointer rests; additive brushes only do with the option on.
            let always = p.mask || matches!(p.brush, Brush::Flatten | Brush::Layer | Brush::Smooth);
            let virtual_travel = if always {
                (2.0 * p.radius * dt - stroke.travel_this_frame).max(0.0)
            } else if build_up_on_hold {
                2.0 * p.radius * dt * 0.35
            } else {
                0.0
            };
            if virtual_travel > 0.0 {
                if let Some(k) = stroke.knots.last().copied() {
                    let spacing = (p.radius * p.spacing()).max(0.0005);
                    stroke.carry += virtual_travel;
                    while stroke.carry >= spacing && stroke.dabs_this_frame < p.max_dabs_per_frame() {
                        stroke.carry -= spacing;
                        let pressure = stroke.pressure;
                        Self::emit(
                            mesh,
                            stroke,
                            k.pos,
                            k.normal,
                            k.dir,
                            pressure,
                            view.pos,
                            &mut self.dabs_applied,
                        );
                    }
                    stroke.carry = stroke.carry.min(spacing);
                }
            }
            if !p.mask && p.brush == Brush::Clay && !stroke.frame_centres.is_empty() {
                let centres = std::mem::take(&mut stroke.frame_centres);
                kernels::clay_relax(
                    mesh,
                    &p,
                    &centres,
                    stroke.dabs_this_frame.max(1),
                    stroke.relax,
                    stroke.object_size,
                    stroke.curvature,
                    &mut stroke.scratch,
                );
            }
        }
        stroke.frame_centres.clear();
        stroke.dabs_this_frame = 0;
        stroke.travel_this_frame = 0.0;
    }

    /// Finishes the stroke and returns its undo record.
    pub fn end(&mut self, mesh: &mut Mesh, view: &View) -> Option<StrokeDelta> {
        if let Some(stroke) = self.stroke.as_mut() {
            let p = stroke.params;
            if p.mask || !matches!(p.brush, Brush::Move | Brush::Pose | Brush::SnakeHook) {
                Self::advance(mesh, stroke, true, view.pos, &mut self.dabs_applied);
            }
            if let Some(g) = stroke.grab.as_mut() {
                g.dirty = true;
            }
            self.pose.touch();
        }
        self.frame(mesh, view, 0.0);
        let stroke = self.stroke.take()?;
        self.pose.end(mesh.end_stroke(stroke.params.mask))
    }
}

/// Move's Connected Only (ZBrush's Move Topological). For each copy of the
/// brush it keeps only the vertices that can be reached from the one nearest
/// that copy's centre by walking mesh edges without leaving what the copy
/// grabbed. A Move on one finger then leaves the finger next to it alone,
/// and the lip below the one being moved, though both sit inside the ring.
///
/// `weights` holds one weight per copy for every vertex of `verts`; vertices
/// no copy holds afterwards are dropped from both.
fn keep_connected(mesh: &Mesh, centres: &[Vec3], verts: &mut Vec<u32>, weights: &mut Vec<f32>) {
    let n_ops = centres.len();
    let mut grabbed = BitSet::with_len(mesh.vertex_count());
    let mut reached = BitSet::with_len(mesh.vertex_count());
    let mut queue: Vec<u32> = Vec::new();
    for (k, &centre) in centres.iter().enumerate() {
        let held = |slot: usize| weights[slot * n_ops + k] > 0.0;
        let mut seed = None;
        let mut nearest = f32::INFINITY;
        for (slot, &v) in verts.iter().enumerate() {
            if held(slot) {
                grabbed.set(v as usize);
                let d2 = mesh.positions[v as usize].distance_squared(centre);
                if d2 < nearest {
                    (nearest, seed) = (d2, Some(v));
                }
            }
        }
        let Some(seed) = seed else { continue };
        queue.clear();
        queue.push(seed);
        reached.set(seed as usize);
        let mut head = 0;
        while head < queue.len() {
            let v = queue[head];
            head += 1;
            for &o in mesh.neighbours(v) {
                if grabbed.get(o as usize) && !reached.get(o as usize) {
                    reached.set(o as usize);
                    queue.push(o);
                }
            }
        }
        for (slot, &v) in verts.iter().enumerate() {
            if !reached.get(v as usize) {
                weights[slot * n_ops + k] = 0.0;
            }
        }
        grabbed.clear();
        reached.clear();
    }
    let mut kept = 0;
    for slot in 0..verts.len() {
        let row = slot * n_ops..(slot + 1) * n_ops;
        if weights[row.clone()].iter().any(|&w| w > 0.0) {
            verts[kept] = verts[slot];
            weights.copy_within(row, kept * n_ops);
            kept += 1;
        }
    }
    verts.truncate(kept);
    weights.truncate(kept * n_ops);
}

/// Copies the position of every vertex owned by `leaves` into `out`, leaf
/// after leaf (see the buffer helpers in `kernels`).
fn save(mesh: &Mesh, leaves: &[u32], out: &mut Vec<Vec3>) {
    let (starts, total) = kernels::layout(mesh, leaves);
    out.resize(total, Vec3::ZERO);
    let parts = kernels::stretches(out, kernels::spans(&starts, total));
    kernels::each_stretch(parts, total, |k, part| {
        part.copy_from_slice(&mesh.positions[kernels::owned(&mesh.bvh.leaves[leaves[k] as usize])]);
    });
}

fn restore(mesh: &mut Mesh, leaves: &[u32], saved: &[Vec3]) {
    let (starts, total) = kernels::layout(mesh, leaves);
    let ranges = &mesh.bvh.leaves;
    let parts = kernels::stretches(
        &mut mesh.positions,
        leaves.iter().map(|&l| kernels::owned(&ranges[l as usize])),
    );
    kernels::each_stretch(parts, total, |k, part| {
        part.copy_from_slice(&saved[starts[k]..starts[k] + part.len()]);
    });
}

/// Adds how far every vertex of `leaves` has moved from `saved` to `sum`.
fn add_moves(mesh: &Mesh, leaves: &[u32], saved: &[Vec3], sum: &mut [Vec3]) {
    let (starts, total) = kernels::layout(mesh, leaves);
    let parts = kernels::stretches(sum, kernels::spans(&starts, total));
    kernels::each_stretch(parts, total, |k, part| {
        let live = &mesh.positions[kernels::owned(&mesh.bvh.leaves[leaves[k] as usize])];
        for ((s, a), b) in part.iter_mut().zip(live).zip(&saved[starts[k]..]) {
            *s += *a - *b;
        }
    });
}

/// Raises `furthest`, the square of the furthest any copy so far has moved
/// each vertex of `leaves` from `saved`, to how far the copy just applied has.
fn note_furthest(mesh: &Mesh, leaves: &[u32], saved: &[Vec3], furthest: &mut [f32]) {
    let (starts, total) = kernels::layout(mesh, leaves);
    let parts = kernels::stretches(furthest, kernels::spans(&starts, total));
    kernels::each_stretch(parts, total, |k, part| {
        let live = &mesh.positions[kernels::owned(&mesh.bvh.leaves[leaves[k] as usize])];
        for ((f, a), b) in part.iter_mut().zip(live).zip(&saved[starts[k]..]) {
            *f = f.max((*a - *b).length_squared());
        }
    });
}
