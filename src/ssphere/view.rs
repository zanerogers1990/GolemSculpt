//! The armature as drawn: every sphere and link the rig puts in the world,
//! reflections included, as one mesh, plus the placement ghost and the mirror
//! plane.
//!
//! Nothing here is used for picking. The gestures pick analytically against
//! the same [`super::rig::Rig::geometry`] output this file draws, so the
//! thing under the cursor is exactly the thing on screen.
//!
//! The mesh is built in the rig's own space, where the mirror plane is
//! x = 0, and moved into the world as the last step.

use std::f32::consts::{PI, TAU};

use bytemuck::{Pod, Zeroable};
use glam::Vec3;

use super::SSpheres;
use super::input::LINK_SCALE;
use super::rig;

const SPHERE_SEGMENTS: usize = 18;
const SPHERE_RINGS: usize = 12;
const TUBE_SIDES: usize = 14;
/// Band length along a link, as a multiple of the link's mean radius. The
/// alternating bands earn their place: on a smooth tube there is nothing to
/// tell you which way it is twisting or how long it is in depth.
const BAND_LENGTH_PER_RADIUS: f32 = 0.55;
const MAX_BANDS_PER_LINK: usize = 40;
/// Spheres are drawn a touch inside their true surface. The skin's shell lies
/// exactly on that surface wherever a sphere shows through it, and two
/// surfaces in the same place flicker; picking still uses the true radius.
const SPHERE_INSET: f32 = 0.97;

// Colours as picked (sRGB); converted to linear when written to a vertex.
const NODE: [f32; 4] = [0.78, 0.79, 0.82, 1.0];
const SELECTED: [f32; 4] = [1.0, 0.6, 0.18, 1.0];
const HOVERED: [f32; 4] = [0.62, 0.86, 1.0, 1.0];
const LINK_LIGHT: [f32; 4] = [0.66, 0.67, 0.71, 1.0];
const LINK_DARK: [f32; 4] = [0.5, 0.51, 0.55, 1.0];
const LINK_HOVERED: [f32; 4] = [0.52, 0.8, 0.98, 1.0];
const GHOST: [f32; 4] = [0.62, 0.86, 1.0, 0.35];
const PLANE: [f32; 4] = [1.0, 0.25, 0.25, 0.14];

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Vertex {
    pub position: [f32; 3],
    /// Zero for flat, unlit geometry (the mirror plane).
    pub normal: [f32; 3],
    /// Linear colour; alpha below 1 draws as a see-through shell.
    pub color: [f32; 4],
}

/// One frame's armature geometry. The opaque part comes first in `indices`,
/// then the see-through part (ghost and mirror plane), which has to be drawn
/// after everything solid.
#[derive(Default)]
pub struct Armature {
    pub vertices: Vec<Vertex>,
    pub indices: Vec<u32>,
    pub opaque: u32,
}

fn linear(c: [f32; 4]) -> [f32; 4] {
    let f = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    [f(c[0]), f(c[1]), f(c[2]), c[3]]
}

/// A unit sphere as (points, triangle indices). On a unit sphere a point is
/// its own normal.
fn unit_sphere() -> (Vec<Vec3>, Vec<u32>) {
    let mut points = Vec::with_capacity((SPHERE_RINGS + 1) * (SPHERE_SEGMENTS + 1));
    for r in 0..=SPHERE_RINGS {
        let phi = PI * r as f32 / SPHERE_RINGS as f32;
        for c in 0..=SPHERE_SEGMENTS {
            let theta = TAU * c as f32 / SPHERE_SEGMENTS as f32;
            points.push(Vec3::new(phi.sin() * theta.cos(), phi.cos(), phi.sin() * theta.sin()));
        }
    }
    let stride = (SPHERE_SEGMENTS + 1) as u32;
    let mut indices = Vec::with_capacity(SPHERE_RINGS * SPHERE_SEGMENTS * 6);
    for r in 0..SPHERE_RINGS as u32 {
        for c in 0..SPHERE_SEGMENTS as u32 {
            let a = r * stride + c;
            let b = a + stride;
            indices.extend_from_slice(&[a, a + 1, b, a + 1, b + 1, b]);
        }
    }
    (points, indices)
}

impl Armature {
    fn vertex(&mut self, position: Vec3, normal: Vec3, color: [f32; 4]) {
        self.vertices.push(Vertex { position: position.to_array(), normal: normal.to_array(), color });
    }

    fn sphere(&mut self, unit: &(Vec<Vec3>, Vec<u32>), centre: Vec3, radius: f32, color: [f32; 4]) {
        let base = self.vertices.len() as u32;
        let color = linear(color);
        for &p in &unit.0 {
            self.vertex(centre + p * radius, p, color);
        }
        self.indices.extend(unit.1.iter().map(|&i| base + i));
    }

    /// A tapered tube from `a` to `b`, in alternating bands.
    fn tube(&mut self, a: Vec3, b: Vec3, radius_a: f32, radius_b: f32, hovered: bool) {
        let axis = b - a;
        let length = axis.length();
        if length < 1e-6 {
            return;
        }
        let d = axis / length;
        // Any perpendicular will do for the ring frame; (u, v, d) keeps the
        // same handedness for every link, so one winding rule faces every
        // tube outward.
        let reference = if d.y.abs() < 0.99 { Vec3::Y } else { Vec3::X };
        let u = d.cross(reference).normalize();
        let v = d.cross(u);
        let mean_radius = (0.5 * (radius_a + radius_b)).max(1e-5);
        let bands = ((length / (mean_radius * BAND_LENGTH_PER_RADIUS)).round() as usize).clamp(1, MAX_BANDS_PER_LINK);
        // Cone slope folded into the normal, so a tapering link shades as a
        // cone rather than as a cylinder with its ends pinched.
        let slope = (radius_a - radius_b) / length;
        let colors = [linear(LINK_LIGHT), linear(LINK_DARK), linear(LINK_HOVERED)];

        // Each band gets its own two rings, so it can carry its own colour.
        for band in 0..bands {
            let color = colors[if hovered { 2 } else { band % 2 }];
            let base = self.vertices.len() as u32;
            for end in 0..2 {
                let t = (band + end) as f32 / bands as f32;
                let centre = a + axis * t;
                let radius = radius_a + (radius_b - radius_a) * t;
                for side in 0..TUBE_SIDES {
                    let angle = side as f32 / TUBE_SIDES as f32 * TAU;
                    let radial = u * angle.cos() + v * angle.sin();
                    self.vertex(centre + radial * radius, (radial + d * slope).normalize(), color);
                }
            }
            let sides = TUBE_SIDES as u32;
            for side in 0..sides {
                let next = (side + 1) % sides;
                let (p0, p1, p2, p3) = (base + side, base + next, base + sides + side, base + sides + next);
                self.indices.extend_from_slice(&[p1, p3, p2, p1, p2, p0]);
            }
        }
    }

    /// A flat square in the mirror plane (x = 0).
    fn plane(&mut self, centre: Vec3, half: f32) {
        let base = self.vertices.len() as u32;
        let color = linear(PLANE);
        for (y, z) in [(-1.0, -1.0), (1.0, -1.0), (-1.0, 1.0), (1.0, 1.0)] {
            self.vertex(Vec3::new(0.0, centre.y + y * half, centre.z + z * half), Vec3::ZERO, color);
        }
        self.indices.extend_from_slice(&[base, base + 2, base + 1, base + 2, base + 3, base + 1]);
    }
}

impl SSpheres {
    /// Everything of the rig to draw this frame.
    pub fn armature(&self) -> Armature {
        let mut out = Armature::default();
        let unit = unit_sphere();
        let (spheres, links) = self.rig.geometry(self.symmetry);
        // The solid preview hides the armature: it is for judging the shape.
        if !self.preview {
            let hovered_link = self.hovered_link();
            for link in &links {
                out.tube(
                    link.a,
                    link.b,
                    link.radius_a * LINK_SCALE,
                    link.radius_b * LINK_SCALE,
                    hovered_link == Some(link.child),
                );
            }
            let hovered = self.hovered.filter(|_| !self.is_dragging());
            for sphere in &spheres {
                // A node and its reflection light up together: they are one
                // node, and an edit to either lands on both, so showing only
                // the half under the cursor would be lying about what a drag
                // is about to do.
                let color = if self.selected == Some(sphere.node) {
                    SELECTED
                } else if hovered == Some(sphere.node) {
                    HOVERED
                } else {
                    NODE
                };
                out.sphere(&unit, sphere.centre, sphere.radius * SPHERE_INSET, color);
            }
        }
        out.opaque = out.indices.len() as u32;

        let ghost = self.ghost();
        if self.symmetry && !self.preview {
            if !self.rig.is_empty() {
                let bounds = self.rig.bounds(self.symmetry);
                let extent = (bounds.size() * 0.5).length().max(self.rig.mean_radius() * 2.0);
                out.plane(bounds.center(), (extent * 1.1).max(0.025));
            } else if let Some((centre, radius)) = ghost {
                // No rig yet: show where the centre line is, so the first
                // sphere can be put on it or deliberately beside it.
                out.plane(centre, radius * 4.0);
            }
        }
        if let Some((centre, radius)) = ghost {
            out.sphere(&unit, centre, radius, GHOST);
            // Off the mirror plane a new sphere comes with its reflection.
            if self.symmetry && centre.x != 0.0 {
                out.sphere(&unit, rig::mirror(centre), radius, GHOST);
            }
        }

        // Out of the rig's space and into the world. Normals only turn: the
        // frame's size is one number, so nothing is sheared.
        let frame = self.frame;
        if !frame.is_identity() {
            for v in &mut out.vertices {
                v.position = frame.to_world(Vec3::from(v.position)).to_array();
                v.normal = frame.direction_to_world(Vec3::from(v.normal)).to_array();
            }
        }
        out
    }
}
