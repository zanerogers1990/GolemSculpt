//! Stroke handling: turns pointer samples into evenly spaced, symmetric dabs.
//!
//! Dabs are paced by distance travelled, never by time, so a stroke looks the
//! same at any frame rate and tapers naturally at both ends. The pointer path
//! is joined with a centripetal Catmull-Rom spline, so a fast stroke is a
//! curve rather than a polygon.
//!
//! How far apart the dabs are, how big each is under the pen's pressure and
//! what is random about it are the brush's own (`BrushSettings`), settled
//! dab by dab in `Sculptor::emit`. A stroke faster than the machine can lay
//! falls behind the pen and catches up rather than losing any of its path
//! (`pace.rs`).

pub mod alpha;
pub mod cavity;
pub mod dyntopo;
pub mod erase;
mod falloff;
mod input;
mod jitter;
mod kernels;
pub mod library;
#[cfg(test)]
mod library_tests;
mod modes;
mod pace;
pub mod paint;
mod pose;
mod radial;
mod share;
#[cfg(test)]
mod kernel_tests;
#[cfg(test)]
mod stroke_tests;
mod symmetry;
#[cfg(test)]
mod symmetry_tests;
#[cfg(test)]
mod tests;

use std::collections::VecDeque;
use std::ops::RangeInclusive;
use std::time::Instant;

use glam::{Vec2, Vec3};
use rayon::prelude::*;

use crate::camera::View;
use crate::mesh::{Mesh, StrokeDelta};
use crate::util::BitSet;
pub use falloff::{Falloff, FalloffCurve, FalloffPreset, focal_k, shifted};
pub use input::PenMeter;
use input::{LazyRope, PenPressure};
use jitter::Jitter;
use kernels::{CurvatureBase, Dab, Params, SymOp};
pub use modes::{FlattenMode, SmoothMode};
pub use pace::Pace;
use share::{Settling, Shared, Tally};
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
    Pinch,
    Nudge,
    EraseLayer,
    Paint,
}

impl Brush {
    /// How many brushes there are: what everything kept per brush is sized by.
    pub const COUNT: usize = 14;

    pub const ALL: [Brush; Brush::COUNT] = [
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
        Brush::Pinch,
        Brush::Nudge,
        Brush::EraseLayer,
        Brush::Paint,
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
            Brush::Pinch => "Pinch",
            Brush::Nudge => "Nudge",
            Brush::EraseLayer => "Erase Layer",
            Brush::Paint => "Paint",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Brush::Move => "Grab the surface under the ring and drag it. Ctrl: push it in or pull it out.",
            Brush::Clay => "Build up or dig away material in flat-topped strips.",
            Brush::Smooth => "Relax the surface. Hold Shift with any brush for this.",
            Brush::Crease => "Cut a sharp groove and pinch its edges together.",
            Brush::Inflate => "Swell the surface outward along its own normals.",
            Brush::Flatten => "Level the surface to the average plane under the ring.",
            Brush::Pose => "Bend a limb at a joint the brush finds, like posing a figure.",
            Brush::Standard => "Raise a soft, rounded stroke.",
            Brush::Layer => "Raise an even step of fixed height.",
            Brush::SnakeHook => "Pull out long tendrils that follow the cursor.",
            Brush::Pinch => "Draw the surface in toward the stroke to tighten an edge or sharpen a ridge. Ctrl spreads it apart.",
            Brush::Nudge => "Push the surface sideways along the stroke, like sliding clay with a thumb.",
            Brush::EraseLayer => {
                "Rub out the sculpt layer that is recording, under the ring: its detail goes, the model underneath                  and the other layers stay."
            }
            Brush::Paint => {
                "Paint colour and materials on the model, on the active paint layer. Ctrl erases, Shift blurs the paint."
            }
        }
    }

    /// The number key that picks the brush: the same ones as the Unity app,
    /// which are all there are. A brush added since has none, and is picked
    /// from the panel or the wheel.
    pub fn hotkey(self) -> Option<char> {
        Some(match self {
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
            // Not a number key: P takes it up and puts it down again (app.rs).
            Brush::Paint => 'P',
            Brush::Pinch | Brush::Nudge | Brush::EraseLayer => return None,
        })
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
            Brush::Pinch => "brush_pinch",
            Brush::Nudge => "brush_nudge",
            Brush::EraseLayer => "brush_erase_layer",
            Brush::Paint => "brush_paint",
        }
    }

    /// Whether Ctrl / right button flips this brush. Erase Layer has nothing
    /// to flip to: what it rubbed out is in no layer any more (`erase.rs`).
    /// Paint adds and removes no clay: Ctrl swaps painting and erasing
    /// instead (`paint.rs`), which the Add and Sub buttons do not show.
    pub fn has_polarity(self) -> bool {
        !matches!(
            self,
            Brush::Smooth | Brush::Move | Brush::Pose | Brush::SnakeHook | Brush::Nudge | Brush::EraseLayer | Brush::Paint
        )
    }

    /// Whether the brush is on the hold-Space wheel until the user says
    /// otherwise. Erase Layer is not: it is for one job, does nothing until
    /// a layer records, and the wheel is what a sculptor's hand knows. It is
    /// pinned from the Brush Library card like any other. Nor is Paint, so
    /// that the wheel stays as the hand knows it: P takes it up.
    pub fn starts_on_wheel(self) -> bool {
        !matches!(self, Brush::EraseLayer | Brush::Paint)
    }

    /// Whether the brush works across the way its stroke is going, and so
    /// needs to know which way that is: Crease cuts and pinches across its
    /// travel, and Pinch draws the surface in to the stroke's line.
    fn follows_travel(self) -> bool {
        matches!(self, Brush::Crease | Brush::Pinch)
    }

    /// Whether a stroke's opening dab waits until the stroke has set off, to
    /// take its direction. A dab with none draws the surface in toward its
    /// own middle, along the stroke as well as across it. Crease puts that
    /// right as it goes: each of its dabs sets a vertex back where it began
    /// along the stroke. Pinch works on the surface as it finds it and puts
    /// nothing back, so a first dab laid at once would leave a pucker at the
    /// head of every stroke.
    fn waits_for_travel(self) -> bool {
        self == Brush::Pinch
    }

    /// The distance between dabs the brush starts with, as a fraction of its
    /// radius, and the one its amounts were tuned at: a dab at any other
    /// Spacing does as much more or less as keeps the stroke building at the
    /// same rate (see [`Stroke::dose`]). Crease is a thin line and needs its
    /// dabs twice as close to read as one.
    pub fn default_spacing(self) -> f32 {
        if self == Brush::Crease { 0.1 } else { 0.2 }
    }

    /// Whether a stroke of this brush is a run of dabs laid along the pen's
    /// path, each of which Spacing, Scatter, Size Jitter and the pen's
    /// pressure shape. Move and Pose take hold once at the press and are
    /// dragged, and Snake Hook pulls the surface after the pen step by step:
    /// none of them has dabs to space, and none reads pressure.
    pub fn lays_dabs(self) -> bool {
        !matches!(self, Brush::Move | Brush::Pose | Brush::SnakeHook)
    }

    /// Standard and Clay place their dabs on the surface as it was when the
    /// stroke began; the others follow the live surface.
    fn uses_start_surface(self) -> bool {
        matches!(self, Brush::Standard | Brush::Clay)
    }
}

/// One brush's own settings: everything that shapes its stroke apart from
/// the size of the ring, which every brush shares.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct BrushSettings {
    pub strength: f32,
    pub accumulate: bool,
    pub build_up: f32,
    pub positive: bool,
    pub focal_shift: f32,
    pub front_facing: bool,
    /// Stay in Group: a stroke changes only the face group it began on. The
    /// mesh sees to it, as it does for hidden faces (`Mesh::stay_in_groups`,
    /// set for the stroke by `app/groups.rs`); nothing in a brush reads this.
    pub stay_in_group: bool,
    /// Spacing: the distance between dabs, as a fraction of the dab's radius.
    pub spacing: f32,
    /// Scatter: each dab is thrown off the stroke's path, along the surface,
    /// by a random distance up to this fraction of its radius.
    pub scatter: f32,
    /// Size Jitter: each dab is smaller than the brush by a random share of
    /// its size, up to this.
    pub size_jitter: f32,
    /// Pressure -> Size: how much smaller the brush is at the pen's lightest
    /// touch. 0 is off: pressure does not change the size.
    pub pressure_size: f32,
    /// Pressure -> Strength: how much of its Strength the brush loses at the
    /// pen's lightest touch. 0 is off: pressure does not change the strength.
    pub pressure_strength: f32,
    /// For a brush that smooths: what it evens out, and Keep Volume, how much
    /// of the shrinkage smoothing causes is given back (0 to 1).
    pub smooth_mode: SmoothMode,
    pub keep_volume: f32,
    /// For a brush that flattens: which side of its plane it works on, and
    /// Lock Plane, whether the whole stroke levels to the plane its first
    /// dab found.
    pub flatten_mode: FlattenMode,
    pub lock_plane: bool,
}

impl BrushSettings {
    /// What the sliders allow. Spacing runs from dabs all but on top of each
    /// other to prints a brush's width and more apart.
    pub const SPACING: RangeInclusive<f32> = 0.02..=2.5;
    pub const SCATTER: RangeInclusive<f32> = 0.0..=2.0;
    pub const SIZE_JITTER: RangeInclusive<f32> = 0.0..=0.9;
    /// Both of the pressure settings.
    pub const PRESSURE: RangeInclusive<f32> = 0.0..=1.0;
}

/// Where Pressure -> Strength starts: the lightest touch works at 12% of the
/// brush's Strength, as it did when that floor was one setting for every
/// brush.
pub const DEFAULT_PRESSURE_STRENGTH: f32 = 0.88;

/// The spacing, in radii, at which one dab alone stands as tall as the
/// stroke its brush draws: past it the dabs are separate prints, and a print
/// is given the stroke's height and no more (see [`Stroke::dose`]).
const PRINT_SPACING: f32 = 1.0;

/// Clay sweeps its tip from each dab to the next so a stroke is one strip.
/// With its dabs further apart than this many radii they are meant to show
/// as dabs, and the sweep would smear them into a strip again.
const SWEEP_SPACING: f32 = 0.5;

/// The brushes' settings as strokes read them: for each kernel one working
/// set (`per_brush`, `falloff`, `alpha` and the kernel's own numbers below),
/// and what holds for every stroke.
///
/// A brush in the panel is a named record over a kernel, and there can be
/// many over one kernel (`library.rs`). The one in hand for a kernel is the
/// one whose record is laid in that kernel's working set here; the library
/// swaps records in and out as brushes are picked, and nothing in the
/// stroke code knows there is more than one.
#[derive(Clone)]
pub struct Settings {
    pub brush: Brush,
    pub per_brush: [BrushSettings; Brush::COUNT],
    /// Each brush's hand-drawn falloff, or `None` while it keeps its own.
    pub falloff: [Option<FalloffCurve>; Brush::COUNT],
    /// Each brush's alpha: the picture it prints, for the brushes that take
    /// one (see `alpha.rs`).
    pub alpha: [alpha::AlphaSettings; Brush::COUNT],
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
    /// The pen's own response, whichever brush it holds (see
    /// `input::pen_curve`). How far pressure then changes a brush's strength
    /// and size is that brush's: `BrushSettings`.
    pub pressure_curve: f32,
    pub lazy_mouse: bool,
    pub lazy_radius: f32,
    pub lazy_strength: f32,
    pub build_up_on_hold: bool,
    /// The brush Shift smooths with, when that is not the one in Smooth's
    /// own working set (the library says which it is, and keeps this in
    /// step: `Library::select`). `None` is the working set, which is how it
    /// is while there is only the one Smooth.
    pub shift: Option<ShiftBrush>,
    /// The Paint brush's colour, mode, channels and Zadd (`paint.rs`).
    pub paint: paint::PaintSettings,
}

/// What a Shift stroke takes from the brush Shift uses: its settings and its
/// falloff curve. (A smoothing brush has no alpha and no numbers of its
/// kernel's own.)
#[derive(Clone, PartialEq)]
pub struct ShiftBrush {
    pub settings: BrushSettings,
    pub falloff: Option<FalloffCurve>,
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
            stay_in_group: false,
            spacing: 0.2,
            scatter: 0.0,
            size_jitter: 0.0,
            pressure_size: 0.0,
            pressure_strength: DEFAULT_PRESSURE_STRENGTH,
            smooth_mode: SmoothMode::All,
            keep_volume: 0.0,
            flatten_mode: FlattenMode::Both,
            lock_plane: false,
        };
        let mut per_brush = [base; Brush::COUNT];
        for b in Brush::ALL {
            per_brush[b.index()].spacing = b.default_spacing();
        }
        for b in [Brush::Clay, Brush::Inflate, Brush::Standard] {
            per_brush[b.index()].accumulate = true;
        }
        per_brush[Brush::Crease.index()].positive = false;
        // Move's Strength is how much of the pen's travel the surface
        // follows. Half of it: a pull that is easy to place, not a yank.
        // Nudge's is the same thing, dab by dab.
        per_brush[Brush::Move.index()].strength = 0.5;
        per_brush[Brush::Nudge.index()].strength = 0.5;
        // A stroke of Pinch at this draws the surface a third of the way in.
        per_brush[Brush::Pinch.index()].strength = 0.3;
        // An eraser pressed hard rubs right out; the pen's pressure is what
        // makes it gentle (a light touch works at an eighth of this).
        per_brush[Brush::EraseLayer.index()].strength = 1.0;
        // Paint's Strength is its Flow: at full pressure a dab lays the whole
        // Opacity under the middle of the ring, and the falloff softens the
        // edge. A light touch of the pen lays an eighth of that.
        per_brush[Brush::Paint.index()].strength = 1.0;
        Self {
            brush: Brush::Move,
            per_brush,
            falloff: Default::default(),
            alpha: Default::default(),
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
            pressure_curve: 1.6,
            lazy_mouse: false,
            lazy_radius: 25.0,
            lazy_strength: 1.0,
            build_up_on_hold: false,
            shift: None,
            paint: paint::PaintSettings::default(),
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

    /// The settings and falloff a stroke of `brush` begun now would work
    /// with: that kernel's working set, unless `shift` (Shift is held, and
    /// this is not mask painting) and another Smooth brush is the one marked
    /// for Shift, when they are that brush's. One place says so, for the
    /// stroke and for the cursor that shows what the stroke will do.
    pub fn in_use(&self, brush: Brush, shift: bool) -> (BrushSettings, Falloff) {
        match self.shift.as_ref().filter(|_| shift) {
            Some(marked) => (marked.settings, Falloff::new(marked.settings.focal_shift, marked.falloff.as_ref())),
            None => (self.per_brush[brush.index()], self.falloff_of(brush)),
        }
    }

    /// [`Self::half_weight_radius`] for the brush a stroke begun now would
    /// use: with Shift held, the brush Shift smooths with.
    pub fn half_weight_radius_in_use(&self, brush: Brush, shift: bool) -> f32 {
        kernels::half_weight_radius(brush, &self.in_use(brush, shift).1, self.clay_softness)
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
    /// the shape it starts from), `alpha=rake+follow` (every brush's alpha,
    /// see `AlphaSettings::demo_setup`), `spacing=150`, `scatter=50` and
    /// `jitter=40` (every brush's Spacing, Scatter and Size Jitter, in
    /// percent) and `fixed` (Screen-Space Size off). The
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
                Some(("alpha", spec)) if !strokes_done => {
                    self.alpha.iter_mut().for_each(|slot| slot.demo_setup(spec));
                }
                // Every brush's Spacing, Scatter and Size Jitter, as the
                // sliders show them: `spacing=150` is 150% of the radius.
                Some((name @ ("spacing" | "scatter" | "jitter"), value)) if !strokes_done => {
                    let Ok(percent) = value.parse::<f32>() else { continue };
                    for b in &mut self.per_brush {
                        let (slot, range) = match name {
                            "spacing" => (&mut b.spacing, BrushSettings::SPACING),
                            "scatter" => (&mut b.scatter, BrushSettings::SCATTER),
                            _ => (&mut b.size_jitter, BrushSettings::SIZE_JITTER),
                        };
                        *slot = (percent / 100.0).clamp(*range.start(), *range.end());
                    }
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
    /// Pen pressure 0..1, or `None` for a mouse (and for the pen's lift, which a stroke reads as the pen still).
    pub pressure: Option<f32>,
    /// Seconds since the app started.
    pub time: f64,
}

#[derive(Clone, Copy)]
struct Knot {
    pos: Vec3,
    normal: Vec3,
    dir: Vec3,
    /// The pen's pressure as the brushes read it (`input::pen_curve`): 0 to
    /// 1, and always 1 for a mouse.
    pressure: f32,
    px: Vec2,
}

/// The stretch of the path between two knots that the dabs are being laid
/// along. Its curve is fixed when the walk along it begins, so a walk that a
/// frame could not afford to finish (`pace.rs`) carries on along the same
/// curve in the next, whatever knots have arrived since.
#[derive(Clone, Copy)]
struct Seg {
    /// The knots at its ends, and the curve's control points beyond them.
    k1: Knot,
    k2: Knot,
    p0: Vec3,
    p3: Vec3,
    /// The curve is walked as this many straight pieces.
    pieces: usize,
    /// The piece the walk is on (from 1), where that piece starts, and how
    /// far along it the walk has come.
    piece: usize,
    from: Vec3,
    walked: f32,
    /// Whether that piece's length has been added to the frame's travel.
    counted: bool,
}

impl Seg {
    /// How much of the stretch is still to walk, near enough.
    fn left(&self) -> f32 {
        let pieces_left = (self.pieces + 1).saturating_sub(self.piece) as f32;
        self.k1.pos.distance(self.k2.pos) * pieces_left / self.pieces as f32
    }
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
    /// What the copies come to on a vertex more than one of them holds.
    hold: share::Hold,
    /// Set when the grab goes in and out along the surface, not with the pen.
    along: Option<Along>,
    leaves: Vec<u32>,
    dirty: bool,
}

/// A Move made with Ctrl or the right button (ZBrush's Alt + Move): the grab
/// is pushed in or pulled out along the way the surface under the ring
/// faces, wherever the camera is. From in front, that is the one way a plain
/// Move cannot take it, since a plain Move stays on the plane facing you.
#[derive(Clone, Copy)]
struct Along {
    /// The way the grabbed surface faces.
    normal: Vec3,
    /// The way across the screen the pen goes to pull it out.
    drag: Vec3,
}

impl Along {
    /// Past this much of the surface's facing lying across the view (45
    /// degrees round from head on), the surface is seen from the side.
    const SIDE_ON: f32 = 0.7;

    /// For surface facing `normal`. Seen from the side, the way it faces is
    /// a way across the screen, and the pen is dragged that way to pull it
    /// out. Seen head on there is no such way: up pulls out and down pushes
    /// in. (There has to be a jump between the two somewhere: no rule gives
    /// every facing a way across the screen without one.)
    fn of(normal: Vec3, view: &View) -> Along {
        let across = normal - view.forward * normal.dot(view.forward);
        let drag = if across.length() >= Self::SIDE_ON { across.normalize() } else { view.up };
        Along { normal, drag }
    }
}

struct Stroke {
    /// The stroke's settings, with the RING's radius: each dab takes them
    /// with a radius of its own (`Params::for_dab`).
    params: Params,
    /// The brush's own settings as the stroke found them: its spacing,
    /// scatter, size jitter and what the pen's pressure does. Mask painting
    /// keeps a pace of its own whichever brush is selected (see `begin`).
    bs: BrushSettings,
    /// The spacing the brush's amounts were tuned at (see [`Stroke::dose`]).
    tuned_spacing: f32,
    /// What the stroke's random numbers come from (`jitter.rs`), and how
    /// many dabs it has laid: the next one's place in the stroke.
    seed: u32,
    dab: u32,
    /// Pointer samples a frame could not afford, in order, each with the
    /// view it was made in and its distance in pixels from the one before
    /// (`pace.rs`). The total of those distances is `queued_px`.
    queue: VecDeque<(PointerSample, View, f32)>,
    queued_px: f32,
    /// The stretch of the path being walked, if a frame ran out part way.
    seg: Option<Seg>,
    /// The path so far is to be walked to its very end and then forgotten:
    /// the pointer has left the mesh, or (`ending`) the stroke is over.
    lifted: bool,
    ending: bool,
    /// What the stroke has spent of this frame, and how wide its backlog is
    /// being spaced: 1 unless it is too far behind.
    meter: pace::Meter,
    widen: f32,
    /// What a pixel covered on the model where the stroke landed, for
    /// guessing how many dabs the samples still waiting come to.
    world_per_px: f32,
    /// The camera as of the last sample worked through.
    cam: Vec3,
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
    /// The pen's pressure as of the last sample worked through, as the
    /// brushes read it: 0 to 1, and 1 for a mouse.
    pressure: f32,
    time: f64,
    /// For a brush that works across its travel (`Brush::follows_travel`):
    /// the way the surface under the stroke faces and the way the stroke is
    /// going, each following with a little lag.
    crease_n: Option<Vec3>,
    crease_d: Vec3,
    /// Flatten with Lock Plane: the plane the stroke's first dab found,
    /// which the whole stroke levels to.
    plane: Option<(Vec3, Vec3)>,
    /// Which way the brush's alpha is turned, if it has one.
    aim: alpha::Aim,
    /// The stroke is one print of the alpha, placed by dragging.
    print: Option<alpha::Print>,
    /// This frame's dabs, every copy of each, for Clay relax.
    frame_centres: Vec<kernels::Centre>,
    grab: Option<Grab>,
    /// Snake hook: point being dragged.
    tip: Vec3,
    object_size: f32,
    relax: f32,
    /// What Clay's finishing pass measures curvature against.
    curvature: CurvatureBase,
    /// What the walk over mirror images keeps between dabs, like `scratch`.
    walk: Walked,
    scratch: kernels::Scratch,
    /// The Paint brush with Zadd: the sculpting kernel's settings, run in
    /// the same dab as the paint (`paint.rs`).
    zadd: Option<Params>,
    /// What a Paint stroke carries from dab to dab (Smear).
    painting: paint::Painting,
}

/// The buffers of the walk over mirror images (`Sculptor::apply_symmetric`),
/// kept between dabs: a fresh buffer per dab costs more than filling it.
#[derive(Default)]
struct Walked {
    /// The copies of the set in hand.
    group: Vec<SymOp>,
    /// The leaves each of them reaches, one run per copy, and where each run
    /// ends.
    reached: Vec<u32>,
    ends: Vec<usize>,
    /// Every leaf the set reaches, ascending, and where each starts in
    /// `saved`.
    leaves: Vec<u32>,
    starts: Vec<usize>,
    /// Where the set's vertices were before the dab, for copies near a
    /// mirror plane.
    saved: Vec<Vec3>,
}

impl Stroke {
    /// Whether the stroke is a run of dabs laid along the pen's path: mask
    /// painting whatever the brush, and otherwise the brushes that lay dabs.
    fn lays_dabs(&self) -> bool {
        self.params.mask || self.params.brush.lays_dabs()
    }

    /// Whether the stroke is worked sample by sample, in order, and so can
    /// fall behind the pen (`pace.rs`): the ones that lay dabs, and Snake
    /// Hook, which follows the pen's whole path. Move, Pose and a print
    /// being dragged out only ever want the pen's latest place.
    fn queues(&self) -> bool {
        self.print.is_none() && (self.lays_dabs() || self.params.brush == Brush::SnakeHook)
    }

    /// The radius the brush works at under pen pressure `pen`: the ring's,
    /// or less by the brush's Pressure -> Size.
    fn radius_at(&self, pen: f32) -> f32 {
        self.params.radius * input::pen_size(self.bs.pressure_size, pen)
    }

    /// The Spacing the dabs are being laid at, as a fraction of their
    /// radius: the brush's own, or wider while the stroke is too far behind.
    fn spacing(&self) -> f32 {
        self.bs.spacing * self.widen
    }

    /// The distance on the model from one dab to the next under pen pressure
    /// `pen`. It follows the dab's own radius, so a stroke that tapers under
    /// Pressure -> Size keeps its dabs as close for their size all the way.
    fn gap_at(&self, pen: f32) -> f32 {
        (self.radius_at(pen) * self.spacing()).max(0.0005)
    }

    /// How much a dab laid `spacing` radii from the last does, against one
    /// at the spacing the brush's amounts were tuned at (`Params::dose`).
    ///
    /// In proportion: at twice the spacing half as many dabs cover each
    /// point of the stroke, so each does twice as much, and the stroke
    /// builds as fast as before. Only how smooth it is changes.
    ///
    /// Up to a spacing of one radius. There a dab alone stands as tall as
    /// the stroke: a brush's falloff, added up over dabs one radius apart,
    /// comes to its peak (a smoothstep and the same shape a radius on add up
    /// to exactly 1). Wider than that the dabs are separate prints, and the
    /// proportion would make each taller the further apart they were set,
    /// a row of spikes. So a print is given the stroke's height and no more:
    /// spreading the prints out changes how many there are, not how tall.
    fn dose(spacing: f32, tuned: f32) -> f32 {
        spacing.min(PRINT_SPACING) / tuned
    }

    /// Whether this is Clay sweeping an alpha's print from each dab to the
    /// next. Such a stroke has no opening dab of its own once it sets off.
    ///
    /// Every dab after the first is spread along the stretch of path behind
    /// it, so the strip is laid evenly. The opening dab was one whole dab's
    /// worth stamped in one place, on top of the sweep that then set off
    /// from it. A soft round tip hides that; a picture with an edge printed
    /// it: the head of the strip stood a tenth taller than the rest for one
    /// print's length and then stepped down, a line across every stroke one
    /// print in from where it began. Without the stamp the strip starts as
    /// it ends, rising over one print's length. A press that never sets off
    /// still gets its one print, when it lifts or has been held a moment
    /// ([`OPENING_HOLD`]).
    fn sweeps_a_print(&self) -> bool {
        let p = &self.params;
        let sweeps = self.bs.spacing <= SWEEP_SPACING && self.bs.scatter <= 0.0;
        // A picture that is full weight everywhere has no edge to print: a
        // stroke with it is the stroke with no alpha, to the bit.
        !p.mask && p.brush == Brush::Clay && p.alpha.is_some_and(|stamp| stamp.shapes()) && sweeps
    }

    /// How long the pointer must rest before a stretch of path that is
    /// waiting for its next knot is laid anyway: a short pause for a stroke
    /// under way, longer for the opening of a swept print, which is better
    /// left out if the stroke is about to set off.
    fn pause_to_release(&self) -> f64 {
        let opening = self.last_dab.is_none() && self.done == 0 && self.seg.is_none();
        if opening && self.sweeps_a_print() { OPENING_HOLD } else { 0.05 }
    }

    /// Whether anything of the stroke is still waiting to be laid: samples
    /// in the queue, a stretch of the path part walked, or, once the path is
    /// to be walked right out, any of it that has not been.
    fn behind(&self) -> bool {
        let n = self.knots.len();
        let opened = n == 0 || self.last_dab.is_some() || self.done > 0;
        let walked_out = opened && self.done + 1 >= n;
        !self.queue.is_empty() || self.seg.is_some() || ((self.lifted || self.ending) && !walked_out)
    }

    /// How many dabs the path still to be walked comes to at the brush's own
    /// Spacing, near enough: what is left of the stretch in hand, the knots
    /// after it, and the samples still waiting (by how far apart they are on
    /// screen).
    fn dabs_waiting(&self) -> f32 {
        let mut path = self.seg.as_ref().map_or(0.0, Seg::left);
        let from = (self.done + usize::from(self.seg.is_some())).min(self.knots.len());
        path += self.knots[from..].windows(2).map(|pair| pair[0].pos.distance(pair[1].pos)).sum::<f32>();
        path += self.queued_px * self.world_per_px;
        let radius = self.radius_at(self.pressure);
        let step = if self.lays_dabs() { radius * self.bs.spacing } else { SNAKE_STEP * radius };
        path / step.max(0.0005)
    }

    /// Sets how wide the dabs are spaced, before more of the stroke is laid:
    /// wider than the brush's own Spacing only when more is waiting than
    /// about a second can lay, and then by as much as makes what is waiting
    /// a second's work (`pace.rs`).
    ///
    /// It only ever widens while the stroke is behind, and goes back to the
    /// brush's own once the stroke has caught up (`caught_up`). Narrowing
    /// again as the backlog shrank would have the backlog take ever longer
    /// to clear the nearer it got to clear: a release would wait two or
    /// three seconds for a stroke that could be finished in one.
    fn set_widen(&mut self, pace: Pace) {
        let most = self.meter.per_second(pace);
        self.widen = self.widen.max((self.dabs_waiting() / most).clamp(1.0, MAX_WIDEN));
    }

    /// Nothing is waiting any more: the dabs are spaced as the brush asks.
    fn caught_up(&mut self) {
        if !self.behind() {
            self.widen = 1.0;
        }
    }
}

/// How far a stroke drawn by hand has fallen behind the pen (`pace.rs`), for
/// whatever wants to watch it catch up: the scripted check of the live pace
/// (`app/ui_demo/hand.rs`).
#[derive(Clone, Copy, Debug)]
pub struct Lag {
    /// Anything at all is still waiting to be laid.
    pub behind: bool,
    /// Pointer samples taken in and not yet worked into the stroke.
    pub samples: usize,
    /// About how many dabs the path still to be walked comes to.
    pub dabs: f32,
    /// How much wider than the brush's own Spacing the dabs are being laid:
    /// 1 unless the stroke has got more than about a second behind.
    pub widen: f32,
    /// Seconds a dab has been taking lately.
    pub per_dab: f32,
    /// Where on the screen the brush has got to.
    pub at: Vec2,
}

/// How long, in seconds, a press must be held in place before Clay prints
/// the opening dab of a swept alpha where it landed (see
/// [`Stroke::sweeps_a_print`]). Long enough that a pen which lands and then
/// sets off, as a hand does, has set off first; short enough that a press
/// meant as one print shows it without having to lift.
const OPENING_HOLD: f64 = 0.25;

/// How far Snake Hook pulls the surface in one step, in radii.
const SNAKE_STEP: f32 = 0.15;

/// The most a stroke that is behind is spaced wider. Past any sense, but a
/// number that keeps the arithmetic finite.
const MAX_WIDEN: f32 = 64.0;

#[derive(Default)]
pub struct Sculptor {
    pub settings: Settings,
    /// Every brush there is to pick: the fourteen the kernels start as, the
    /// ones that ship with the app and the user's own (`library.rs`).
    pub library: library::Library,
    /// How much of a stroke each frame lays (`pace.rs`). The app sets it for
    /// strokes drawn by hand; everything else is unpaced.
    pub pace: Pace,
    /// What the pen is reporting, for the panel (`input::PenMeter`).
    pub pen_meter: PenMeter,
    stroke: Option<Stroke>,
    /// Held between strokes: resetting it makes every stroke open with a
    /// spike of strength.
    pen: PenPressure,
    /// Whether the stroke in progress began with a pressure reading, which only
    /// a pen gives. Its later samples without one (the lift) are the pen still.
    pen_stroke: bool,
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

impl Sculptor {
    pub fn is_active(&self) -> bool {
        self.stroke.is_some()
    }

    /// While a stroke runs: where the brush actually is (the lazy-mouse rope
    /// end) and the world radius it is working at, so the cursor never lies
    /// about where the next dab will land or how big it will be. The ring's
    /// own radius is frozen for the stroke; under Pressure -> Size the brush
    /// works at a share of it that follows the pen.
    pub fn stroke_cursor(&self) -> Option<(Vec2, f32, f32)> {
        self.stroke.as_ref().map(|s| {
            let radius = if s.lays_dabs() && s.print.is_none() { s.radius_at(s.pressure) } else { s.params.radius };
            (s.rope.pos, radius, s.params.sign)
        })
    }

    /// How far the stroke under way is behind the pen: see [`Lag`]. `None`
    /// between strokes.
    pub fn lag(&self) -> Option<Lag> {
        self.stroke.as_ref().map(|stroke| Lag {
            behind: stroke.behind(),
            samples: stroke.queue.len(),
            dabs: stroke.dabs_waiting(),
            widen: stroke.widen,
            per_dab: stroke.meter.per_dab(),
            at: stroke.rope.pos,
        })
    }

    /// The way the stroke under way was last going, on the model, once it
    /// has gone anywhere: what an alpha that turns with the stroke points
    /// its top along, for the picture shown under the cursor.
    pub fn stroke_heading(&self) -> Option<Vec3> {
        self.stroke.as_ref().and_then(|stroke| stroke.aim.heading())
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
    /// Shift with the Paint brush blurs the paint instead (`paint.rs`): a
    /// painting brush never moves the surface unasked.
    pub fn effective_brush(&self, smooth_held: bool) -> Brush {
        let paints = self.settings.brush == Brush::Paint;
        if smooth_held && !self.settings.mask_mode && !paints { Brush::Smooth } else { self.settings.brush }
    }

    /// Whether a stroke begun on `mesh` now could add triangles to it: see
    /// [`dyntopo::DynTopo::may_grow`]. `smooth_held` as for [`Self::begin`].
    pub fn may_refine(&self, mesh: &Mesh, smooth_held: bool) -> bool {
        let brush = self.effective_brush(smooth_held);
        self.dyntopo.may_grow(brush, self.settings.mask_mode, mesh.triangle_count())
    }

    /// The pen's pressure as the brushes read it, after this sample: 0 to 1.
    /// What a brush makes of it (its strength, its size) is the brush's own:
    /// `input::pen_strength`, `input::pen_size`.
    fn shape_pressure(&mut self, sample: &PointerSample) -> f32 {
        self.pen.step(sample.pressure, sample.time);
        // A stroke any of whose samples carries pressure is a pen's, even if
        // its first did not (a touch too light for the tablet to measure).
        self.pen_stroke |= sample.pressure.is_some();
        // A mouse never reports pressure and sculpts at full strength and
        // full size. A pen's lift carries none either, and must not count as
        // a mouse: it takes the value the pen held.
        if !self.pen_stroke {
            return 1.0;
        }
        input::pen_curve(self.pen.smoothed, self.settings.pressure_curve)
    }

    /// Zadd's sculpting kernel for a Paint stroke with `params`: the
    /// kernel's own Strength and options, the Paint brush's ring, falloff
    /// and alpha. Ctrl turns it round, as it turns painting into erasing.
    fn zadd_params(&self, params: &Params, invert: bool) -> Option<Params> {
        let s = &self.settings;
        if params.mask || params.brush != Brush::Paint || !s.paint.zadd || params.paint.is_none() {
            return None;
        }
        let kernel = s.paint.zadd_brush;
        if !paint::ZADD_BRUSHES.contains(&kernel) {
            return None;
        }
        let own = s.per_brush[kernel.index()];
        let sign = if own.positive != invert { 1.0 } else { -1.0 };
        Some(Params {
            brush: kernel,
            sign,
            strength: own.strength,
            accumulate: own.accumulate,
            build_up: own.build_up,
            smooth_mode: own.smooth_mode,
            keep_volume: own.keep_volume,
            flatten_mode: own.flatten_mode,
            erase: None,
            paint: None,
            ..*params
        })
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
        // The Paint brush's stroke: what its mode (Blur with Shift, Erase and
        // Paint swapped by Ctrl) lays. Sample is a click, not a stroke.
        let painting = (!mask && brush == Brush::Paint).then(|| {
            let mode = if smooth_held { paint::PaintMode::Blur } else { self.settings.paint.mode };
            self.settings.paint.stroke(mode, invert)
        });
        if painting == Some(None) {
            return false;
        }
        // Shift smooths with the brush marked for it, whichever Smooth brush
        // was last in hand.
        let (mut bs, falloff) = self.settings.in_use(brush, smooth_held && !mask && brush == Brush::Smooth);
        let mut tuned_spacing = brush.default_spacing();
        if mask {
            // A mask is painted where the ring is, at one pace whichever
            // brush happens to be selected: that brush's Spacing, Scatter,
            // Size Jitter and Pressure -> Size are for its own strokes. Its
            // Strength is the mask's too, and with it what pressure does to
            // the strength.
            tuned_spacing = 0.2;
            bs = BrushSettings { spacing: tuned_spacing, scatter: 0.0, size_jitter: 0.0, pressure_size: 0.0, ..bs };
        }
        // The RING's size, frozen here for the whole stroke (each dab then
        // works at a share of it): re-deriving it would change the
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
            falloff: if mask { Falloff::BUILT_IN } else { falloff },
            alpha: if mask { None } else { self.settings.stamp_of(brush, radius) },
            front_facing: bs.front_facing,
            clay_depth: s.clay_depth,
            clay_soft: s.clay_softness,
            crease_pinch: s.crease_pinch,
            crease_depth: s.crease_depth,
            flatten_offset: s.flatten_offset,
            layer_height: s.layer_height,
            mask_hardness: s.mask_hardness,
            smooth_mode: bs.smooth_mode,
            keep_volume: bs.keep_volume,
            flatten_mode: bs.flatten_mode,
            spacing: bs.spacing,
            dose: Stroke::dose(bs.spacing, tuned_spacing),
            erase: if !mask && brush == Brush::EraseLayer { erase::Erased::of(mesh) } else { None },
            paint: painting.flatten(),
        };
        let zadd = self.zadd_params(&params, invert);
        self.pen_stroke = sample.pressure.is_some();
        let pressure = self.shape_pressure(&sample);
        let bounds = mesh.bounds();
        // Scattered dabs land up to that far off the path: the surface is
        // refined that much wider, for the ring's full size.
        self.dyntopo.begin(brush, mask, bounds.size().max_element(), bs.scatter);
        // Clay's finishing pass would smooth an alpha's print away again.
        let relax = if params.alpha.is_some() { 0.0 } else { self.settings.clay_relax };
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
            bs,
            tuned_spacing,
            seed: jitter::seed(hit.pos),
            dab: 0,
            queue: VecDeque::new(),
            queued_px: 0.0,
            seg: None,
            lifted: false,
            ending: false,
            meter: pace::Meter::default(),
            widen: 1.0,
            world_per_px: view.world_per_pixel(view.depth_of(hit.pos)),
            cam: view.pos,
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
            plane: None,
            aim: alpha::Aim::default(),
            print: params.prints().then(|| alpha::Print::begin(hit.pos, mesh.hit_normal(&hit, false), sample.pos)),
            frame_centres: Vec::new(),
            grab: None,
            tip: hit.pos,
            object_size: bounds.size().max_element().max(1e-6),
            relax,
            curvature,
            walk: Walked::default(),
            scratch: kernels::Scratch::default(),
            zadd,
            painting: paint::Painting::default(),
        };
        self.last_hit = Some(hit.pos);
        if !mask && brush == Brush::Move {
            let connected = self.settings.move_connected;
            let mut grab = Self::grab(mesh, &stroke, hit.pos, view.pos, connected);
            // Ctrl or the right button: in and out along the surface instead.
            if invert {
                grab.along = Some(Along::of(Self::facing(mesh, &grab, mesh.hit_normal(&hit, false)), view));
            }
            stroke.grab = Some(grab);
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
            let hold = share::Hold::of(held.weights());
            return Grab { origin: hit, verts, weights: Vec::new(), held: Some(held), hold, along: None, leaves, dirty: false };
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
            keep_connected(mesh, &centres, &mut verts, &mut weights, &mut Connected::default());
        }
        let leaves = mesh.leaves_of(&verts);
        let hold = share::Hold::of(weights.chunks(n_ops));
        Grab { origin: hit, verts, weights, held: None, hold, along: None, leaves, dirty: false }
    }

    /// The way the surface a grab took hold of faces: the normals of what
    /// the copy under the pointer holds, each by how firmly it is held. One
    /// way for the whole grab, so a bumpy patch goes out as a piece and is
    /// not blown up like a balloon. `under` is the normal under the pointer,
    /// for a grab that holds nothing to speak of.
    fn facing(mesh: &Mesh, grab: &Grab, under: Vec3) -> Vec3 {
        let per_vertex = grab.weights.len() / grab.verts.len().max(1);
        let mut sum = glam::DVec3::ZERO;
        for (slot, &v) in grab.verts.iter().enumerate() {
            let weight = match &grab.held {
                Some(held) => held.own(slot),
                None => grab.weights[slot * per_vertex],
            };
            sum += (mesh.normals[v as usize] * weight).as_dvec3();
        }
        let facing = sum.normalize_or_zero().as_vec3();
        if facing == Vec3::ZERO { under } else { facing }
    }

    /// Feeds one pointer sample to the active stroke.
    ///
    /// A stroke that only ever wants the pen's latest place takes it at
    /// once. For the others the sample joins the back of the queue, and as
    /// much of the queue is worked through as the frame can afford: all of
    /// it, unless the stroke has fallen behind the pen (`pace.rs`).
    pub fn sample(&mut self, mesh: &mut Mesh, view: &View, sample: PointerSample) {
        let Some(stroke) = self.stroke.as_mut() else { return };
        if !stroke.queues() {
            return self.work(mesh, view, sample);
        }
        let from = stroke.queue.back().map_or(stroke.pointer, |(last, ..)| last.pos);
        let px = from.distance(sample.pos);
        stroke.queue.push_back((sample, *view, px));
        stroke.queued_px += px;
        self.pump(mesh);
    }

    /// Lays as much of the stroke as the frame can afford: first along the
    /// path already taken from the pointer, then the samples that are
    /// waiting, in order, each worked through exactly as if it had only now
    /// arrived. So a stroke that falls behind comes out as it would have on
    /// a machine that kept up: its dabs land on the surface the dabs before
    /// them left, not on the surface as it was when the pen went by.
    fn pump(&mut self, mesh: &mut Mesh) {
        let pace = self.pace;
        if let Some(stroke) = self.stroke.as_mut() {
            stroke.meter.start();
        }
        while let Some(stroke) = self.stroke.as_mut() {
            stroke.set_widen(pace);
            if !Self::advance(mesh, stroke, false, pace, &mut self.dabs_applied) {
                break;
            }
            if stroke.queue.is_empty() || !stroke.meter.has_room(pace) {
                break;
            }
            let Some((sample, view, px)) = stroke.queue.pop_front() else { break };
            stroke.queued_px = (stroke.queued_px - px).max(0.0);
            self.work(mesh, &view, sample);
        }
        if let Some(stroke) = self.stroke.as_mut() {
            stroke.meter.stop();
            stroke.caught_up();
        }
    }

    /// Works one pointer sample into the stroke: where the brush now is and,
    /// for a stroke that lays dabs, the next knot of its path (the dabs
    /// along it are `advance`'s to lay).
    fn work(&mut self, mesh: &mut Mesh, view: &View, sample: PointerSample) {
        let pressure = self.shape_pressure(&sample);
        let lazy = (self.settings.lazy_mouse, self.settings.lazy_radius, self.settings.lazy_strength);
        let Some(stroke) = self.stroke.as_mut() else { return };
        let dt = (sample.time - stroke.time).max(0.0) as f32;
        stroke.time = sample.time;
        stroke.pressure = pressure;
        stroke.cam = view.pos;

        let p = stroke.params;
        stroke.aim.look(view);
        if let Some(print) = stroke.print.as_mut() {
            // Sized once a frame in `frame`; only the latest position matters.
            return print.drag_to(view, sample.pos);
        }
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
            // Longer steps, and so fewer, while the stroke is too far behind.
            let steps = ((len / (SNAKE_STEP * p.radius * stroke.widen)).ceil() as usize).clamp(1, 64);
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
                    frame: alpha::Frame::NONE,
                    plane: None,
                };
                let began = Instant::now();
                Self::apply_symmetric(mesh, stroke, &p, dab);
                stroke.meter.dab(began.elapsed());
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
            // What is left of the path so far is walked out first.
            stroke.lifted = true;
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
        if !p.mask && p.brush.follows_travel() {
            // The groove's frame follows the stroke with a little lag so it
            // neither jitters nor chases its own wall. It is taken over the
            // ring's whole radius however small the pen has made the brush.
            let foot = kernels::footprint_normal(mesh, hit.pos, p.radius, false).unwrap_or(normal);
            let n = match stroke.crease_n {
                Some(prev) => slerp_dir(prev, foot, 0.35),
                None => foot,
            };
            if let Some(last) = stroke.knots.last() {
                let travel = hit.pos - last.pos;
                let tangential = travel - n * travel.dot(n);
                // Less travel than this says nothing about where the stroke
                // is heading. A quarter of Crease's own spacing, whatever
                // Spacing is set to: it is about the hand, not the dabs.
                if tangential.length() > 0.25 * Brush::Crease.default_spacing() * p.radius {
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
        // Every symmetric copy of the brush refines where its dabs will go,
        // for the ring's full size: the most a dab of the stroke can cover.
        self.dyntopo.refine(mesh, hit.pos, p.radius, &|at, copies| {
            copies.extend(stroke.ops.iter().map(|op| op.apply_point(at)));
        });
        stroke.knots.push(knot);
        stroke.last_knot_time = sample.time;
    }

    /// Lays dabs along every stretch of the path that is ready, for as long
    /// as the frame can afford them (`pace.rs`). The newest stretch waits
    /// for the next knot (the spline needs it) unless `flush` is set, or the
    /// path is being walked out to its end. Returns false if the frame ran
    /// out first: the walk carries on from where it stopped.
    fn advance(mesh: &mut Mesh, stroke: &mut Stroke, flush: bool, pace: Pace, count: &mut u64) -> bool {
        // A path being walked out: once the pointer has left the mesh, or at
        // the end of the stroke when no more samples are waiting to add to it.
        let flush = flush || stroke.lifted || (stroke.ending && stroke.queue.is_empty());
        let n = stroke.knots.len();
        if n > 0 && stroke.last_dab.is_none() && stroke.done == 0 && stroke.seg.is_none() {
            // A stroke opens with a dab right where it lands. An alpha that
            // turns with the stroke waits for the stroke to set off first
            // (or to end where it began), so that dab faces the right way.
            //
            // So does Pinch (`Brush::waits_for_travel`): its opening dab
            // takes the way the stroke sets off.
            let turns = !stroke.params.mask && stroke.params.brush.waits_for_travel();
            // Clay sweeping a print waits too, and once the stroke has set
            // off it has no opening dab at all: where it landed is where its
            // first sweep starts from (`Stroke::sweeps_a_print`).
            let swept = stroke.sweeps_a_print();
            let mut dir = stroke.knots[0].dir;
            if n > 1 {
                stroke.aim.head(stroke.knots[1].pos - stroke.knots[0].pos);
                if turns {
                    dir = stroke.knots[1].dir;
                }
                if swept {
                    stroke.last_dab = Some(stroke.knots[0].pos);
                }
            } else if !flush && (turns || swept || alpha::Aim::waits(stroke.params.alpha.as_ref())) {
                return true;
            }
            if stroke.last_dab.is_none() {
                if !stroke.meter.has_room(pace) {
                    return false;
                }
                let k = stroke.knots[0];
                Self::emit(mesh, stroke, k.pos, k.normal, dir, k.pressure, false, count);
            }
        }
        loop {
            if stroke.seg.is_none() {
                let n = stroke.knots.len();
                if !(stroke.done + 1 < n && (flush || stroke.done + 2 < n)) {
                    break;
                }
                stroke.seg = Some(Self::stretch(stroke));
            }
            if !Self::walk(mesh, stroke, pace, count) {
                return false;
            }
            stroke.seg = None;
            stroke.done += 1;
        }
        // Only the last few knots are ever needed again.
        if stroke.done > 2 {
            let drop = stroke.done - 2;
            stroke.knots.drain(..drop);
            stroke.done -= drop;
        }
        if stroke.lifted {
            // The path up to where the pointer left the mesh is laid: the
            // stroke starts afresh where it comes back.
            stroke.lifted = false;
            stroke.knots.clear();
            stroke.done = 0;
            stroke.carry = 0.0;
            stroke.last_dab = None;
            // Where it comes back it sets off afresh: the way it was going
            // when it left says nothing about the way it goes next.
            (stroke.crease_n, stroke.crease_d) = (None, Vec3::ZERO);
        }
        true
    }

    /// The next stretch of the path to walk: from the last knot the dabs
    /// have reached to the one after it.
    fn stretch(stroke: &Stroke) -> Seg {
        let (i, n) = (stroke.done, stroke.knots.len());
        let k1 = stroke.knots[i];
        let k2 = stroke.knots[i + 1];
        let p0 = if i > 0 { stroke.knots[i - 1].pos } else { k1.pos * 2.0 - k2.pos };
        let p3 = if i + 2 < n { stroke.knots[i + 2].pos } else { k2.pos * 2.0 - k1.pos };
        let chord = k1.pos.distance(k2.pos);
        // Pieces a quarter as long as the closest the dabs come along it.
        let gap = stroke.gap_at(k1.pressure).min(stroke.gap_at(k2.pressure));
        let pieces = ((chord / (gap * 0.25)).ceil() as usize).clamp(2, 64);
        Seg { k1, k2, p0, p3, pieces, piece: 1, from: k1.pos, walked: 0.0, counted: false }
    }

    /// Walks the stretch in hand, laying a dab each time the distance since
    /// the last comes to the gap the brush asks for there. Returns false if
    /// the frame ran out before the end of the stretch.
    fn walk(mesh: &mut Mesh, stroke: &mut Stroke, pace: Pace, count: &mut u64) -> bool {
        let Some(mut seg) = stroke.seg else { return true };
        while seg.piece <= seg.pieces {
            let s = seg.piece as f32 / seg.pieces as f32;
            let q = catmull_rom(seg.p0, seg.k1.pos, seg.k2.pos, seg.p3, s);
            let len = seg.from.distance(q);
            if !seg.counted {
                stroke.travel_this_frame += len;
                seg.counted = true;
            }
            // As far apart as the size the pen gives the brush here asks.
            let gap = stroke.gap_at(seg.k1.pressure + (seg.k2.pressure - seg.k1.pressure) * s);
            while len > 0.0 && stroke.carry + (len - seg.walked) >= gap {
                if !stroke.meter.has_room(pace) {
                    stroke.seg = Some(seg);
                    return false;
                }
                seg.walked += (gap - stroke.carry).max(0.0);
                stroke.carry = 0.0;
                let f = seg.walked / len;
                let t = (seg.piece as f32 - 1.0 + f) / seg.pieces as f32;
                let pos = seg.from.lerp(q, f);
                let normal = slerp_dir(seg.k1.normal, seg.k2.normal, t);
                let pressure = seg.k1.pressure + (seg.k2.pressure - seg.k1.pressure) * t;
                Self::emit(mesh, stroke, pos, normal, seg.k2.dir, pressure, false, count);
            }
            stroke.carry += len - seg.walked;
            (seg.from, seg.piece, seg.walked, seg.counted) = (q, seg.piece + 1, 0.0, false);
        }
        true
    }

    /// Lays one dab of the stroke at `pos` on its path, under pen pressure
    /// `pen`. `resting` marks a dab made while the pointer rests: those come
    /// at the pace the brush was tuned at, whatever its Spacing (see
    /// `frame`).
    ///
    /// Everything random about the dab is drawn here, once, before it is
    /// copied under symmetry (`jitter.rs`).
    #[allow(clippy::too_many_arguments)]
    fn emit(
        mesh: &mut Mesh,
        stroke: &mut Stroke,
        pos: Vec3,
        normal: Vec3,
        dir: Vec3,
        pen: f32,
        resting: bool,
        count: &mut u64,
    ) {
        let bs = stroke.bs;
        let draw = Jitter::of(stroke.seed, stroke.dab);
        stroke.dab = stroke.dab.wrapping_add(1);
        let spacing = if resting { stroke.tuned_spacing } else { stroke.spacing() };
        // The size the pen gives the brush sets the gap to the next dab;
        // Size Jitter then makes this one smaller without moving the others.
        let sized = stroke.radius_at(pen);
        let gap = (sized * spacing).max(0.0005);
        let radius = (sized * (1.0 - bs.size_jitter * draw.shrink)).max(stroke.params.radius * input::MIN_PEN_SIZE);
        let p = stroke.params.for_dab(radius, spacing, Stroke::dose(spacing, stroke.tuned_spacing));
        // Clay sweeps its tip here from the dab before. After a gap (the
        // pointer came back on to the mesh) from one spacing back rather
        // than across the whole gap; and not at all between dabs that are
        // meant to stand apart.
        let sweeps = bs.spacing <= SWEEP_SPACING && bs.scatter <= 0.0;
        let prev = stroke.last_dab.filter(|_| sweeps).map(|p| {
            if p.distance(pos) > 1.5 * gap { pos + (p - pos).normalize() * gap } else { p }
        });
        let travel = stroke.last_dab.map(|last| pos - last);
        let mut frame = stroke.aim.frame(p.alpha.as_ref(), normal, travel, radius);
        if let Some(stamp) = &p.alpha {
            frame = frame.turned(stamp.random_turn(draw.turn));
        }
        let center = if bs.scatter > 0.0 {
            Self::scattered(mesh, &p, pos, normal, draw.scatter * (bs.scatter * radius))
        } else {
            pos
        };
        let pressure = input::pen_strength(bs.pressure_strength, pen);
        // Nudge carries the surface the way the stroke has come since the
        // dab before.
        let step = match travel {
            Some(travel) if !p.mask && p.brush == Brush::Nudge => travel,
            _ => Vec3::ZERO,
        };
        let mut dab = Dab { center, normal, dir, step, prev, cam: stroke.cam, pressure, frame, plane: stroke.plane };
        if bs.lock_plane && stroke.plane.is_none() && !p.mask && p.brush == Brush::Flatten {
            // Lock Plane: the plane this first dab finds is the stroke's.
            let mut leaves = Vec::new();
            mesh.bvh.leaves_in_sphere(center, radius, &mut leaves);
            leaves.sort_unstable();
            stroke.plane = kernels::flatten_plane(mesh, &p, &dab, &leaves);
            dab.plane = stroke.plane;
        }
        let began = Instant::now();
        Self::apply_symmetric(mesh, stroke, &p, dab);
        stroke.meter.dab(began.elapsed());
        // The path's own place, not the scattered one: the next dab's
        // heading and gap are the stroke's.
        stroke.last_dab = Some(pos);
        stroke.dabs_this_frame += 1;
        *count += 1;
    }

    /// Where a dab thrown `off` along the surface from `pos` lands: on the
    /// surface under (or over) the place it was thrown to, looked for along
    /// the way the brush faces. Where there is no surface there within the
    /// dab's reach (thrown past an edge) it stays level with the path.
    fn scattered(mesh: &Mesh, p: &Params, pos: Vec3, normal: Vec3, off: Vec2) -> Vec3 {
        let (across, along) = normal.any_orthonormal_pair();
        let thrown = pos + across * off.x + along * off.y;
        let origin = thrown + normal * p.radius;
        let from_start = !p.mask && p.brush.uses_start_surface();
        let hit = if from_start { mesh.raycast_start(origin, -normal) } else { mesh.raycast(origin, -normal) };
        match hit {
            Some(hit) if hit.pos.distance(thrown) <= p.radius => hit.pos,
            _ => thrown,
        }
    }

    /// Applies a dab and its copies under the stroke's symmetry.
    ///
    /// Copies that are far apart cannot interact and are simply applied in
    /// turn. Copies that overlap (a dab near a mirror plane) would give a
    /// result that depends on which went first, and that bias accumulates into
    /// visible drift of the centre line. So the copies that can meet, two
    /// across one plane, four or eight where planes cross, are each applied
    /// to the same starting state, and what they did to a vertex is shared
    /// out between them (`share.rs`), as `radial.rs` does for the copies
    /// round an axis. That is symmetric by construction, and it is what
    /// makes a dab as strong on a mirror plane as beside it.
    ///
    /// That is the walk over mirror images, the same whether the planes are
    /// the object's own or the world's (`symmetry::Mirrors` says which copy
    /// lies across which plane, and how near a dab is to each). With turns
    /// round a radial axis among the copies they overlap near the axis
    /// instead, many at a time, and `radial::Walk` has the walk for that.
    ///
    /// `p` is the stroke's settings as this dab takes them, with the dab's
    /// own radius (`Params::for_dab`). It is one for every copy, so a dab
    /// the pen or Size Jitter has made smaller is as much smaller in each.
    fn apply_symmetric(mesh: &mut Mesh, stroke: &mut Stroke, p: &Params, dab: Dab) {
        let p = *p;
        let reach = p.reach(&dab);
        if !p.mask && p.brush == Brush::Paint {
            // Zadd: the sculpting kernel first, under the same symmetry, then the paint on what it raised.
            if let Some(z) = stroke.zadd {
                let z = z.for_dab(p.radius, p.spacing, Stroke::dose(p.spacing, z.brush.default_spacing()));
                Self::apply_symmetric(mesh, stroke, &z, dab);
            }
            return paint::dab(mesh, &p, &dab, &stroke.ops, reach, &mut stroke.painting, &mut stroke.scratch);
        }
        if !p.mask && p.brush == Brush::Clay {
            for &op in &stroke.ops {
                let (at, cam) = (op.apply_point(dab.center), op.apply_point(dab.cam));
                stroke.frame_centres.push(kernels::Centre { at, cam, radius: p.radius });
            }
        }
        if let Some(walk) = stroke.radial.as_mut() {
            return walk.apply(mesh, &p, &dab, reach, &mut stroke.scratch);
        }

        let Stroke { ops, mirrors, scratch, walk, .. } = stroke;
        let Walked { group, reached, ends, leaves, starts, saved } = walk;
        let near = mirrors.near(dab.center, 1.5 * reach);
        // Mirror images only come this way, eight at the most.
        debug_assert!(ops.len() <= 64);
        let mut handled = 0u64;
        for i in 0..ops.len() {
            if handled >> i & 1 != 0 {
                continue;
            }
            // Group: copies that differ from this one only across near planes.
            group.clear();
            for (j, &op) in ops.iter().enumerate() {
                if (mirrors.across(j) ^ mirrors.across(i)) & !near == 0 {
                    handled |= 1 << j;
                    group.push(op);
                }
            }

            // The leaves each copy reaches, one ascending run per copy, as
            // `radial::Walk` keeps them: each copy works on its own leaves
            // only. Then all of them, ascending, each leaf once: the order
            // the saved copies below are laid out in, and what lets a dab's
            // vertices be shared out between threads.
            reached.clear();
            ends.clear();
            for &op in group.iter() {
                let from = reached.len();
                mesh.bvh.leaves_in_sphere(op.apply_point(dab.center), reach, reached);
                reached[from..].sort_unstable();
                ends.push(reached.len());
            }
            leaves.clear();
            leaves.extend_from_slice(reached);
            leaves.sort_unstable();
            leaves.dedup();
            if leaves.is_empty() {
                continue;
            }
            let own = |k: usize| &reached[if k == 0 { 0 } else { ends[k - 1] }..ends[k]];

            if group.len() == 1 || p.mask {
                // Mask amounts all have the same sign and saturate, so order
                // does not matter there.
                for (k, &op) in group.iter().enumerate() {
                    if !own(k).is_empty() {
                        kernels::apply(mesh, &p, &dab.mirrored(op), own(k), leaves, scratch);
                    }
                }
            } else {
                // Copies that can meet: each applied to the surface as it
                // stood before any of them, and what they did to a vertex
                // shared out between them.
                let sharing = p.sharing();
                let total = kernels::layout(mesh, leaves, starts);
                save(mesh, leaves, starts, total, saved);
                let mut shared = std::mem::take(&mut scratch.shared);
                shared.clear();
                shared.resize(total, Shared::default());
                let mut tally = Tally::default();
                // A copy moves the vertices of its own leaves only, unless
                // they are too few for `Mesh::move_vertices` to share out
                // between threads and the set's are not: then it is handed
                // the set's, as every copy once was. The vertices beyond its
                // reach it leaves alone either way, but a small dab of a
                // brush that does a lot per vertex (Clay Strips sweeps its
                // tip up to eight times over each one) took half as long
                // again on one thread as on all of them.
                let shares = total >= kernels::PAR_MIN;
                for (k, &op) in group.iter().enumerate() {
                    if own(k).is_empty() {
                        continue;
                    }
                    let mine: usize = own(k).iter().map(|&l| kernels::owned(&mesh.bvh.leaves[l as usize]).len()).sum();
                    let few = mine < kernels::PAR_MIN;
                    let moves = if shares && few { &leaves[..] } else { own(k) };
                    kernels::apply(mesh, &p, &dab.mirrored(op), moves, leaves, scratch);
                    tally.add(share::bank(mesh, own(k), leaves, starts, saved, &mut shared, sharing));
                }
                let (saved, settling) = (&*saved, Settling::of(sharing, &tally));
                mesh.move_vertices(leaves, |at, _| settling.step(&shared[at]).map(|step| saved[at] + step));
                scratch.shared = shared;
            }

            if p.mask {
                mesh.mask_changed(leaves);
            } else {
                mesh.positions_changed(leaves);
            }
        }
    }

    /// Per-frame work: Move drags, what is left of a stroke that has fallen
    /// behind the pen, brushes that keep working while the pointer is held
    /// still, the pause flush, and Clay's finishing pass.
    pub fn frame(&mut self, mesh: &mut Mesh, view: &View, dt: f32) {
        let (build_up_on_hold, pace) = (self.settings.build_up_on_hold, self.pace);
        let Some(stroke) = self.stroke.as_mut() else { return };
        let p = stroke.params;

        if let Some(grab) = stroke.grab.as_mut().filter(|g| g.dirty) {
            grab.dirty = false;
            let (origin, dir) = view.ray(stroke.pointer);
            let denom = dir.dot(view.forward);
            if denom.abs() > 1e-6 {
                let t = (grab.origin - origin).dot(view.forward) / denom;
                // Strength is how much of the pointer's travel the grabbed
                // point follows: all of it at 100%, staying under the pen.
                let mut delta = (origin + dir * t - grab.origin) * p.strength;
                if let Some(along) = grab.along {
                    delta = along.normal * delta.dot(along.drag);
                }
                // That motion as each copy makes it.
                let pulls: Vec<Vec3> = stroke.ops.iter().map(|op| op.apply_dir(delta)).collect();
                let travel = delta.length() as f64;
                let (n_ops, weights, held, hold) = (pulls.len(), &grab.weights, &grab.held, grab.hold);
                mesh.move_listed(&grab.verts, |slot, vt| {
                    let pulled = match held {
                        Some(held) => held.pull(slot, &pulls),
                        None => {
                            let w = &weights[slot * n_ops..(slot + 1) * n_ops];
                            pulls.iter().zip(w).map(|(&pull, &w)| (pull * w).as_dvec3()).sum()
                        }
                    };
                    // Where copies overlap they share the pull, so a grab on
                    // a mirror plane is no firmer than one beside it, and a
                    // vertex on the plane follows the motion along it.
                    // Always re-derived from the stroke-start position, so
                    // nothing drifts however long the drag lasts.
                    Some(vt.start + hold.settle(pulled, travel))
                });
                mesh.positions_changed(&grab.leaves);
                self.last_hit = Some(grab.origin + delta);
            }
        }
        if let Some(tip) = self.pose.frame(mesh, &stroke.ops, view, stroke.pointer) {
            self.last_hit = Some(tip);
        }

        if stroke.print.is_some() {
            return Self::print_frame(mesh, stroke);
        }
        // What the samples since the last frame could not afford, and the
        // frames before: the stroke catching up with the pen.
        if stroke.queues() {
            self.pump(mesh);
        }
        let Some(stroke) = self.stroke.as_mut() else { return };
        if stroke.lays_dabs() {
            // Detail a busy frame left unfinished, while the pointer rests.
            self.dyntopo.frame(mesh, p.radius, &|at, copies| {
                copies.extend(stroke.ops.iter().map(|op| op.apply_point(at)));
            });
            // What follows is for a stroke that is where the pen is. One
            // still catching up has its own path to lay first.
            let resting = !stroke.behind();
            stroke.meter.start();
            if resting {
                stroke.cam = view.pos;
                // A short pause releases the segment that was waiting for a knot.
                let pause = stroke.pause_to_release();
                if stroke.time - stroke.last_knot_time > pause || f64::from(dt) > pause {
                    Self::advance(mesh, stroke, true, pace, &mut self.dabs_applied);
                }
            }
            // Smoothing, flattening, layering, erasing a layer and masking keep
            // working while the pointer rests (each has a place to stop at);
            // additive brushes only do with the option on.
            // At the size the pen is holding the brush at. Not a brush set
            // to lay separate prints: each of those goes where the stroke
            // puts it, and one more wherever the pen happened to pause
            // would be a print out of line with the rest.
            let radius = stroke.radius_at(stroke.pressure);
            let always = p.mask || matches!(p.brush, Brush::Flatten | Brush::Layer | Brush::Smooth | Brush::EraseLayer);
            let virtual_travel = if !resting || stroke.seg.is_some() || stroke.bs.spacing >= PRINT_SPACING {
                0.0
            } else if always {
                (2.0 * radius * dt - stroke.travel_this_frame).max(0.0)
            } else if build_up_on_hold {
                2.0 * radius * dt * 0.35
            } else {
                0.0
            };
            if virtual_travel > 0.0 {
                if let Some(k) = stroke.knots.last().copied() {
                    // Paced as the brush was tuned, whatever its Spacing: that
                    // is the distance between dabs along a stroke, and these
                    // are going nowhere. Set very tight, the brush would
                    // otherwise lay a hundred dabs a second while it rested.
                    //
                    // `carry` is how far the stroke has come toward its next
                    // dab along the path, out of `along`. Rest brings that
                    // dab nearer at its own pace, a whole one every `gap`,
                    // so it counts for `along / gap` as much: exactly as
                    // much, at the brush's own spacing. (Counted plainly,
                    // a pause at the end of a widely spaced stroke found
                    // the carry already many rest-gaps long and laid them
                    // all at once.)
                    let gap = (radius * stroke.tuned_spacing).max(0.0005);
                    let along = stroke.gap_at(stroke.pressure);
                    stroke.carry += virtual_travel * (along / gap);
                    while stroke.carry >= along && stroke.meter.has_room(pace) {
                        stroke.carry -= along;
                        let pressure = stroke.pressure;
                        Self::emit(mesh, stroke, k.pos, k.normal, k.dir, pressure, true, &mut self.dabs_applied);
                    }
                    stroke.carry = stroke.carry.min(along);
                }
            }
            stroke.meter.stop();
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
        stroke.meter.new_frame(pace);
    }

    /// Finishes the stroke and returns its undo record.
    pub fn end(&mut self, mesh: &mut Mesh, view: &View) -> Option<StrokeDelta> {
        if let Some(stroke) = self.stroke.as_mut() {
            // The path is walked out to its end, behind whatever samples are
            // still waiting their turn.
            stroke.ending = true;
            if let Some(g) = stroke.grab.as_mut() {
                g.dirty = true;
            }
            if let Some(print) = stroke.print.as_mut() {
                print.touch();
            }
            self.pose.touch();
        }
        // A stroke that kept up with the pen ends in the one frame it always
        // did. One that fell behind is finished here, a frame's worth at a
        // time, so that what is done once a frame (Clay's finishing pass,
        // dynamic topology's share of splits) is done as often for these
        // dabs as for the ones before them. It cannot take long: a backlog
        // past about a second's work has been laid coarser (`pace.rs`).
        loop {
            self.frame(mesh, view, 0.0);
            if !self.stroke.as_ref().is_some_and(Stroke::behind) {
                break;
            }
            // As between any two frames (`Mesh::take_dirty`): the dabs to
            // come read the normals these have left.
            mesh.flush_normals();
        }
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
fn keep_connected(mesh: &Mesh, centres: &[Vec3], verts: &mut Vec<u32>, weights: &mut Vec<f32>, walk: &mut Connected) {
    let n_ops = centres.len();
    let Connected { grabbed, reached, queue } = walk;
    grabbed.grow(mesh.vertex_count());
    reached.grow(mesh.vertex_count());
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

/// What [`keep_connected`] walks with: two flags per vertex of the mesh, and
/// the queue. Made once for a grab, however many copies it has; each walk
/// leaves the flags clear for the next.
#[derive(Default)]
struct Connected {
    grabbed: BitSet,
    reached: BitSet,
    queue: Vec<u32>,
}

/// Copies the position of every vertex owned by `leaves` into `out`, leaf
/// after leaf from `starts`, `total` in all (see the buffer helpers in
/// `kernels`).
fn save(mesh: &Mesh, leaves: &[u32], starts: &[usize], total: usize, out: &mut Vec<Vec3>) {
    out.resize(total, Vec3::ZERO);
    kernels::each_stretch(&mut out[..total], starts, |k, part| {
        part.copy_from_slice(&mesh.positions[kernels::owned(&mesh.bvh.leaves[leaves[k] as usize])]);
    });
}
