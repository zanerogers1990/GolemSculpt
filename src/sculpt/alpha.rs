//! Brush alphas: a greyscale picture laid on the surface under the brush,
//! which the brush's weight is multiplied by, so a dab prints the picture
//! instead of a round dot (ZBrush's Alpha).
//!
//! White is full weight and black none. The picture lies in the plane the
//! dab touches the surface in and, at Size 100%, fills the square the brush
//! ring sits in. Its top points up the screen or, with Turn With Stroke, the
//! way the stroke is going. The brush's own falloff still applies on top of
//! it, as in ZBrush: Focal Shift toward Hard shows more of the picture.
//!
//! Each brush that lays material down (see [`Brush::takes_alpha`]) keeps an
//! alpha of its own. A stroke bakes it into a [`Stamp`] when it begins, and
//! every dab carries the [`Frame`] the picture lies in, which is mirrored
//! and turned with the dab under symmetry: the copies print mirror images,
//! bit for bit across the object's own planes.
//!
//! A stroke lays the picture down dab after dab, which suits a rake or a
//! rough texture and smears anything with a shape of its own. For that
//! there is Drag to Place ([`Print`]): the whole stroke is one print.
//!
//! The pictures themselves are in `library` (the ones that come with the
//! app, drawn by `patterns`, and the ones read from the alpha folder), and
//! they are never freed: a stroke holds a plain reference to its picture,
//! which keeps `Params` the small `Copy` value every kernel takes.

mod library;
mod patterns;
#[cfg(test)]
mod tests;

use std::ops::RangeInclusive;

use glam::{Vec2, Vec3};

use super::jitter::Jitter;
use super::kernels::{self, Dab, SymOp};
use super::{Brush, Sculptor, Settings, Stroke};
use crate::camera::View;
use crate::mesh::Mesh;

pub use library::{Thumb, find, import, listing, rescan};

/// A square greyscale picture: `size` samples a side, rows from the top,
/// each from 0 (no weight) to 1 (full weight).
pub struct AlphaImage {
    pub name: String,
    size: usize,
    samples: Box<[f32]>,
}

impl AlphaImage {
    pub fn new(name: &str, size: usize, samples: Vec<f32>) -> AlphaImage {
        assert!(size > 0 && samples.len() == size * size, "an alpha is square");
        AlphaImage { name: name.to_owned(), size, samples: samples.into_boxed_slice() }
    }

    /// The picture drawn by `shade`, which is asked for each sample's centre
    /// (0..1 across, 0..1 down).
    fn drawn(name: &str, size: usize, shade: impl Fn(f32, f32) -> f32) -> AlphaImage {
        let at = |i: usize| (i as f32 + 0.5) / size as f32;
        let samples = (0..size * size).map(|i| shade(at(i % size), at(i / size)).clamp(0.0, 1.0)).collect();
        AlphaImage::new(name, size, samples)
    }

    /// The picture's own value at (`x`, `y`): 0..1 across and 0..1 down,
    /// blended between the four nearest samples. Past an edge the picture
    /// comes round again with `wrap`, and otherwise keeps its edge's value.
    #[inline]
    pub fn sample(&self, x: f32, y: f32, wrap: bool) -> f32 {
        let n = self.size as i32;
        let (fx, fy) = (x * n as f32 - 0.5, y * n as f32 - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let place = |i: i32| if wrap { i.rem_euclid(n) as usize } else { i.clamp(0, n - 1) as usize };
        let (xa, xb) = (place(x0 as i32), place(x0 as i32 + 1));
        let (ya, yb) = (place(y0 as i32) * self.size, place(y0 as i32 + 1) * self.size);
        let s = &self.samples;
        let top = s[ya + xa] + (s[ya + xb] - s[ya + xa]) * tx;
        let bottom = s[yb + xa] + (s[yb + xb] - s[yb + xa]) * tx;
        top + (bottom - top) * ty
    }

    /// Whether every sample of the picture is exactly `value`.
    fn is_all(&self, value: f32) -> bool {
        self.samples.iter().all(|&sample| sample == value)
    }

    /// The picture shrunk to `side` grey bytes a side, for its tile in the
    /// panel.
    pub fn thumbnail(&self, side: usize) -> Vec<u8> {
        let mut out = Vec::with_capacity(side * side);
        for ty in 0..side {
            for tx in 0..side {
                // The mean of the samples this pixel covers (one at least).
                let span = |t: usize| {
                    let from = t * self.size / side;
                    from..((t + 1) * self.size / side).clamp(from + 1, self.size)
                };
                let (mut sum, mut count) = (0.0, 0);
                for y in span(ty) {
                    for x in span(tx) {
                        sum += self.samples[y * self.size + x];
                        count += 1;
                    }
                }
                out.push((sum / count.max(1) as f32 * 255.0).round() as u8);
            }
        }
        out
    }
}

/// One brush's alpha as the panel sets it.
#[derive(Clone, Copy)]
pub struct AlphaSettings {
    /// The picture, or `None` for the brush's plain round tip.
    pub image: Option<&'static AlphaImage>,
    /// Degrees the picture is turned, anticlockwise as you look at it.
    pub rotation: f32,
    /// The picture's size against the brush ring's: at 1 it fills the square
    /// the ring sits in.
    pub scale: f32,
    /// Black prints and white does not.
    pub invert: bool,
    /// The picture repeats past its edges instead of ending there.
    pub tile: bool,
    /// The picture's top points the way the stroke is going, instead of up
    /// the screen.
    pub follow: bool,
    /// Drag to Place: the stroke is one print, sized by dragging out from
    /// where the pen landed (see [`Print`]).
    pub drag: bool,
    /// Random Rotation: each print is turned by a random angle up to this
    /// many degrees either way, on top of `rotation` (see `jitter.rs`).
    pub random_rotation: f32,
}

/// The same picture (the very one: the library never frees or copies them)
/// laid the same way.
impl PartialEq for AlphaSettings {
    fn eq(&self, other: &Self) -> bool {
        let same_picture = match (self.image, other.image) {
            (Some(a), Some(b)) => std::ptr::eq(a, b),
            (None, None) => true,
            _ => false,
        };
        let laid = |a: &AlphaSettings| (a.rotation, a.scale, a.invert, a.tile, a.follow, a.drag, a.random_rotation);
        same_picture && laid(self) == laid(other)
    }
}

impl Default for AlphaSettings {
    fn default() -> Self {
        Self {
            image: None,
            rotation: 0.0,
            scale: 1.0,
            invert: false,
            tile: false,
            follow: false,
            drag: false,
            random_rotation: 0.0,
        }
    }
}

impl AlphaSettings {
    /// What the Size slider allows.
    pub const SCALE: RangeInclusive<f32> = 0.1..=3.0;
    /// What the Random Rotation slider allows, in degrees: at the top a
    /// print may face any way at all.
    pub const RANDOM_ROTATION: RangeInclusive<f32> = 0.0..=180.0;

    /// The alpha as a stroke with a brush of `radius` uses it.
    pub fn stamp(&self, radius: f32) -> Option<Stamp> {
        let image = self.image?;
        let scale = self.scale.clamp(*Self::SCALE.start(), *Self::SCALE.end());
        let (sin, cos) = self.rotation.to_radians().sin_cos();
        let stamp = Stamp {
            image,
            scale,
            inv: 0.0,
            turn: (cos, sin),
            invert: self.invert,
            tile: self.tile,
            follow: self.follow,
            drag: self.drag,
            random: self.random_rotation.clamp(0.0, *Self::RANDOM_ROTATION.end()).to_radians(),
            blank: self.tile && image.is_all(if self.invert { 0.0 } else { 1.0 }),
        };
        Some(stamp.sized(radius))
    }

    /// For `--demo` screenshots (see `Settings::demo_setup`): `spec` is an
    /// alpha's name, then any of `follow`, `tile`, `invert`, `drag`,
    /// `size0.5`, `turn45` and `random180`, joined with `+`.
    pub fn demo_setup(&mut self, spec: &str) {
        let mut words = spec.split('+').map(str::trim);
        self.image = words.next().and_then(find);
        for word in words {
            let number = |prefix: &str| word.strip_prefix(prefix).and_then(|v| v.parse::<f32>().ok());
            match word {
                "follow" => self.follow = true,
                "tile" => self.tile = true,
                "invert" => self.invert = true,
                "drag" => self.drag = true,
                _ => {
                    if let Some(size) = number("size") {
                        self.scale = size;
                    } else if let Some(turn) = number("turn") {
                        self.rotation = turn;
                    } else if let Some(random) = number("random") {
                        self.random_rotation = random;
                    }
                }
            }
        }
    }
}

impl Brush {
    /// Whether this brush can print an alpha: the ones that lay material
    /// down or take it away under the ring, Erase Layer among them (it rubs
    /// a layer out through the picture), and Paint (it paints through it, a
    /// stencil). The others move what is there
    /// (Move, Snake Hook, Pose), even it out (Smooth, Flatten) or cut a line
    /// (Crease), and have no use for a picture.
    pub fn takes_alpha(self) -> bool {
        matches!(self, Brush::Standard | Brush::Clay | Brush::Inflate | Brush::Layer | Brush::EraseLayer | Brush::Paint)
    }
}

impl Settings {
    /// The alpha a stroke of `brush` with this `radius` prints, if it has
    /// one.
    pub fn stamp_of(&self, brush: Brush, radius: f32) -> Option<Stamp> {
        if !brush.takes_alpha() {
            return None;
        }
        self.alpha[brush.index()].stamp(radius)
    }
}

/// A brush's alpha baked for one stroke. Small and `Copy`, like `Falloff`.
#[derive(Clone, Copy)]
pub struct Stamp {
    image: &'static AlphaImage,
    /// The picture's size against the brush's.
    scale: f32,
    /// One over half the picture's width on the surface.
    inv: f32,
    /// Cosine and sine of the picture's own rotation.
    turn: (f32, f32),
    invert: bool,
    tile: bool,
    follow: bool,
    drag: bool,
    /// The furthest Random Rotation turns a print either way, in radians.
    random: f32,
    /// The picture gives every point full weight (white all over, or black
    /// and inverted, and repeating past its edges): a stroke with it is the
    /// stroke with no alpha at all.
    blank: bool,
}

impl Stamp {
    /// The same alpha under a brush of another `radius`.
    pub fn sized(self, radius: f32) -> Stamp {
        Stamp { inv: 1.0 / (radius * self.scale).max(1e-9), ..self }
    }

    /// Whether a stroke with this alpha is one print placed by dragging.
    pub fn drags(&self) -> bool {
        self.drag
    }

    /// Whether the picture gives the dab a shape of its own. One that is
    /// full weight everywhere does not: there is nothing of it to print.
    pub fn shapes(&self) -> bool {
        !self.blank
    }

    /// The angle Random Rotation turns one print by, in radians. `draw` is
    /// that print's own number from -1 to 1 (`Jitter::turn`).
    pub fn random_turn(&self, draw: f32) -> f32 {
        self.random * draw
    }

    /// The alpha's weight at `rel`, a point of the surface measured from the
    /// dab's centre, for a dab whose picture lies in `frame`. Off the edge
    /// of a picture that does not repeat there is no weight at all, inverted
    /// or not: the print ends there.
    #[inline]
    pub fn at(&self, frame: &Frame, rel: Vec3) -> f32 {
        let u = rel.dot(frame.right) * self.inv;
        let v = rel.dot(frame.up) * self.inv;
        if !self.tile && (u.abs() > 1.0 || v.abs() > 1.0) {
            return 0.0;
        }
        let a = self.image.sample(0.5 + 0.5 * u, 0.5 - 0.5 * v, self.tile);
        if self.invert { 1.0 - a } else { a }
    }
}

/// Where a dab's picture lies on the surface: the directions its right and
/// its top point in. Both zero for a dab with no alpha.
#[derive(Clone, Copy, Default, PartialEq, Debug)]
pub struct Frame {
    pub right: Vec3,
    pub up: Vec3,
}

impl Frame {
    pub const NONE: Frame = Frame { right: Vec3::ZERO, up: Vec3::ZERO };

    /// The frame of a copy of the dab under symmetry. Both directions are
    /// carried over, so a mirror image prints the picture mirrored.
    pub fn mirrored(self, op: SymOp) -> Frame {
        Frame { right: op.apply_dir(self.right), up: op.apply_dir(self.up) }
    }

    /// The frame turned `angle` radians where it lies, anticlockwise as you
    /// look at the surface (the way `AlphaSettings::rotation` turns it). No
    /// turn at all leaves it as it is, to the bit.
    pub fn turned(self, angle: f32) -> Frame {
        if angle == 0.0 {
            return self;
        }
        let (sin, cos) = angle.sin_cos();
        Frame { right: self.right * cos + self.up * sin, up: self.up * cos - self.right * sin }
    }
}

/// What a stroke remembers to aim its alpha by: which way is up on the
/// screen, and which way the stroke was last going.
#[derive(Clone, Copy, Default)]
pub struct Aim {
    up: Vec3,
    right: Vec3,
    heading: Option<Vec3>,
}

impl Aim {
    /// Takes the screen's directions from the camera, once a pointer sample.
    pub fn look(&mut self, view: &View) {
        (self.up, self.right) = (view.up, view.right);
    }

    /// Tells it which way the stroke sets off, before its opening dab.
    pub fn head(&mut self, travel: Vec3) {
        self.heading = travel.try_normalize().or(self.heading);
    }

    /// The way the stroke was last going, once it has gone anywhere.
    pub fn heading(&self) -> Option<Vec3> {
        self.heading
    }

    /// Whether the stroke's opening dab should wait for a second point: a
    /// picture that turns with the stroke would otherwise be printed once
    /// facing up the screen before the stroke had a direction.
    pub fn waits(stamp: Option<&Stamp>) -> bool {
        stamp.is_some_and(|s| s.follow)
    }

    /// The frame for a dab on a surface facing `normal`. `travel` is how far
    /// the stroke has come since its last dab, if it has made one; a dab
    /// that has not moved on (one made while the pointer rests) keeps the
    /// last heading.
    pub fn frame(&mut self, stamp: Option<&Stamp>, normal: Vec3, travel: Option<Vec3>, radius: f32) -> Frame {
        let Some(stamp) = stamp else { return Frame::NONE };
        // A direction laid flat on the surface, unless it points straight
        // out of it.
        let flat = |v: Vec3| (v - normal * v.dot(normal)).try_normalize();
        let mut top = None;
        if stamp.follow {
            if let Some(travel) = travel.filter(|t| t.length() > 1e-3 * radius) {
                self.head(travel);
            }
            top = self.heading.and_then(flat);
        }
        let up = top
            .or_else(|| flat(self.up))
            .or_else(|| flat(self.right))
            .unwrap_or_else(|| normal.try_normalize().unwrap_or(Vec3::Z).any_orthonormal_vector());
        // Right as the camera sees it: the surface faces the viewer.
        let right = up.cross(normal).try_normalize().unwrap_or(Vec3::ZERO);
        let (cos, sin) = stamp.turn;
        Frame { right: right * cos + up * sin, up: up * cos - right * sin }
    }
}

/// A stroke that is one print of the alpha, placed by dragging (ZBrush's
/// DragRect): the pen lands where the print's middle goes, and dragging
/// away from there sets how big it is and, for a picture that turns with
/// the stroke, which way its top points. Nothing is laid down for good
/// until the pen lifts: each frame the surface goes back to how the stroke
/// found it and the print is made again at its new size.
pub struct Print {
    /// Where the pen landed: on the surface, and on the screen.
    centre: Vec3,
    pressed: Vec2,
    /// The surface's normal there, for when the print is too small to take
    /// one from the surface under it.
    normal: Vec3,
    /// From the centre to the pointer, on the plane through the centre that
    /// faces the camera. Zero until the pointer has left where it landed.
    reach: Vec3,
    cam: Vec3,
    /// The leaves the print now on the surface touched.
    leaves: Vec<u32>,
    dirty: bool,
}

impl Print {
    /// How far the pointer must go, in pixels, before there is a print: a
    /// tap that was not meant as a drag leaves the surface alone.
    const SLACK: f32 = 3.0;

    pub fn begin(centre: Vec3, normal: Vec3, pressed: Vec2) -> Print {
        Print { centre, pressed, normal, reach: Vec3::ZERO, cam: Vec3::ZERO, leaves: Vec::new(), dirty: false }
    }

    /// The pointer has moved to `px`.
    pub fn drag_to(&mut self, view: &View, px: Vec2) {
        self.cam = view.pos;
        let (origin, dir) = view.ray(px);
        let facing = dir.dot(view.forward);
        self.reach = if px.distance(self.pressed) < Self::SLACK || facing.abs() < 1e-6 {
            Vec3::ZERO
        } else {
            origin + dir * ((self.centre - origin).dot(view.forward) / facing) - self.centre
        };
        self.dirty = true;
    }

    /// Has the print made once more, as the stroke ends.
    pub fn touch(&mut self) {
        self.dirty = true;
    }
}

impl Sculptor {
    /// Once a frame while a print is being dragged out: takes the last one
    /// off the surface and lays the print down at the size it has now.
    pub(super) fn print_frame(mesh: &mut Mesh, stroke: &mut Stroke) {
        let Some(print) = stroke.print.as_mut().filter(|print| print.dirty) else { return };
        print.dirty = false;
        let last = std::mem::take(&mut print.leaves);
        if !last.is_empty() {
            mesh.move_vertices(&last, |_, vt| (vt.pos != vt.start).then_some(vt.start));
            mesh.positions_changed(&last);
        }
        let (centre, reach, cam, fallback) = (print.centre, print.reach, print.cam, print.normal);
        // No bigger than reaches right round the model: past that a longer
        // drag changes nothing but the work.
        let radius = reach.length().min(4.0 * stroke.object_size);
        if radius <= 1e-6 * stroke.object_size {
            return;
        }
        stroke.params.radius = radius;
        stroke.params.alpha = stroke.params.alpha.map(|stamp| stamp.sized(radius));
        let normal = kernels::footprint_normal(mesh, centre, radius, true).unwrap_or(fallback);
        let frame = stroke.aim.frame(stroke.params.alpha.as_ref(), normal, Some(reach), radius);
        // The stroke's one print has the stroke's first draw, the same each
        // time it is remade: Random Rotation must not spin it as it grows.
        let turn = stroke.params.alpha.map_or(0.0, |stamp| stamp.random_turn(Jitter::of(stroke.seed, 0).turn));
        let frame = frame.turned(turn);
        let (dir, step) = (Vec3::ZERO, Vec3::ZERO);
        let dab = Dab { center: centre, normal, dir, step, prev: None, cam, pressure: 1.0, frame, plane: None, copy: SymOp::IDENTITY };
        let mut leaves = Vec::new();
        for op in &stroke.ops {
            mesh.bvh.leaves_in_sphere(op.apply_point(centre), radius, &mut leaves);
        }
        leaves.sort_unstable();
        leaves.dedup();
        let p = stroke.params;
        Self::apply_symmetric(mesh, stroke, &p, dab);
        if let Some(print) = stroke.print.as_mut() {
            print.leaves = leaves;
        }
    }
}
