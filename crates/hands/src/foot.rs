//! A foot from its sliders.
//!
//! Built as a right foot in foot units, about 1 from the back of the heel to the tip of the big toe:
//! the ankle joint at the origin, the toes pointing +Z, the sole flat on the ground below, the big toe
//! on the +X side (the inside of a right foot, for a figure facing +Z). The left foot is the mirror
//! image.
//!
//! The arch is a hollow cut from under the inside of the foot, so the outside edge stays on the
//! ground as a real foot's does. Flat Sole cuts the bottom level, for a figure that has to stand.

use glam::{Mat3, Quat, Vec3};

use crate::body::{Body, Group};
use crate::digit::Chain;
use crate::shapes::Shape;
use crate::{MAX_DIGITS, fix, profile};

/// Lengths of the four smaller toes, second to little.
const TOE_LENGTH: [f32; 4] = [0.19, 0.16, 0.135, 0.11];
const TOE_RADIUS: [f32; 4] = [0.034, 0.031, 0.029, 0.026];
/// Where the ankle joint sits, before the foot is moved to put it at the origin.
const ANKLE: Vec3 = Vec3::new(0.0, 0.22, 0.25);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FootShape {
    pub toes: u32,
    pub toe_length: f32,
    pub toe_thickness: f32,
    pub big_toe: f32,
    pub taper: f32,
    pub spread: f32,
    pub width: f32,
    pub heel: f32,
    pub arch: f32,
    /// How much leg there is above the ankle, 0 to 1.
    pub ankle: f32,
    /// The leg leaning forward (+) or back (-), -1 to 1. Back points the foot.
    pub ankle_bend: f32,
    pub flat_sole: bool,
    pub knuckles: f32,
    pub softness: f32,
    pub claws: f32,
    /// How far each toe curls under, big toe first: -0.6 lifted to 1 clenched.
    pub curl: [f32; MAX_DIGITS],
}

impl Default for FootShape {
    fn default() -> FootShape {
        FootShape {
            toes: 5,
            toe_length: 1.0,
            toe_thickness: 1.0,
            big_toe: 1.0,
            taper: 0.25,
            spread: 0.2,
            width: 1.0,
            heel: 1.0,
            arch: 0.5,
            ankle: 0.35,
            ankle_bend: 0.0,
            flat_sole: true,
            knuckles: 0.4,
            softness: 0.5,
            claws: 0.0,
            curl: [0.0; MAX_DIGITS],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FootPose {
    Standing,
    Pointed,
    ToesUp,
    Gripping,
    Splayed,
}

impl FootPose {
    pub const ALL: [FootPose; 5] =
        [FootPose::Standing, FootPose::Pointed, FootPose::ToesUp, FootPose::Gripping, FootPose::Splayed];

    pub fn name(self) -> &'static str {
        match self {
            FootPose::Standing => "Standing",
            FootPose::Pointed => "Pointed",
            FootPose::ToesUp => "Toes Up",
            FootPose::Gripping => "Gripping",
            FootPose::Splayed => "Splayed",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            FootPose::Standing => "Flat on the ground, toes resting.",
            FootPose::Pointed => "The foot pointed down away from the leg, toes curled a little.",
            FootPose::ToesUp => "The toes lifted off the ground.",
            FootPose::Gripping => "The toes curled under, as if gripping.",
            FootPose::Splayed => "The toes spread wide.",
        }
    }

    /// Sets the curls, the spread, the ankle and the sole; the build is left alone.
    pub fn apply(self, shape: &mut FootShape) {
        let (curl, spread, bend, flat) = match self {
            FootPose::Standing => (0.0, 0.2, 0.0, true),
            FootPose::Pointed => (0.4, 0.15, -1.0, false),
            FootPose::ToesUp => (-0.55, 0.3, 0.0, true),
            FootPose::Gripping => (0.95, 0.05, 0.0, true),
            FootPose::Splayed => (-0.1, 1.0, 0.0, true),
        };
        shape.curl = [curl; MAX_DIGITS];
        shape.spread = spread;
        shape.ankle_bend = bend;
        shape.flat_sole = flat;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FootStyle {
    Human,
    Slender,
    Chunky,
    Cartoon,
    Creature,
}

impl FootStyle {
    pub const ALL: [FootStyle; 5] =
        [FootStyle::Human, FootStyle::Slender, FootStyle::Chunky, FootStyle::Cartoon, FootStyle::Creature];

    pub fn name(self) -> &'static str {
        match self {
            FootStyle::Human => "Human",
            FootStyle::Slender => "Slender",
            FootStyle::Chunky => "Chunky",
            FootStyle::Cartoon => "Cartoon",
            FootStyle::Creature => "Creature",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            FootStyle::Human => "An ordinary adult foot.",
            FootStyle::Slender => "A narrow foot with a high arch and long toes.",
            FootStyle::Chunky => "A broad, flat, heavy foot with short toes.",
            FootStyle::Cartoon => "Three round toes on a soft, simple foot.",
            FootStyle::Creature => "Four long knobbly toes with claws.",
        }
    }

    /// Sets the build, keeping the pose.
    pub fn apply(self, shape: &mut FootShape) {
        let (curl, spread, bend, flat) = (shape.curl, shape.spread, shape.ankle_bend, shape.flat_sole);
        *shape = FootShape::default();
        match self {
            FootStyle::Human => {}
            FootStyle::Slender => {
                shape.width = 0.85;
                shape.arch = 0.85;
                shape.toe_length = 1.2;
                shape.toe_thickness = 0.85;
                shape.heel = 0.9;
            }
            FootStyle::Chunky => {
                shape.width = 1.3;
                shape.arch = 0.15;
                shape.toe_length = 0.8;
                shape.toe_thickness = 1.3;
                shape.heel = 1.2;
            }
            FootStyle::Cartoon => {
                shape.toes = 3;
                shape.width = 1.15;
                shape.toe_length = 0.75;
                shape.toe_thickness = 1.6;
                shape.big_toe = 1.0;
                shape.arch = 0.2;
                shape.taper = 0.0;
                shape.knuckles = 0.0;
                shape.softness = 1.0;
            }
            FootStyle::Creature => {
                shape.toes = 4;
                shape.toe_length = 1.6;
                shape.toe_thickness = 0.85;
                shape.arch = 0.8;
                shape.knuckles = 0.9;
                shape.claws = 0.8;
                shape.taper = 0.5;
            }
        }
        (shape.curl, shape.spread, shape.ankle_bend, shape.flat_sole) = (curl, spread, bend, flat);
    }
}

impl FootShape {
    pub fn sanitized(&self) -> FootShape {
        let d = FootShape::default();
        FootShape {
            toes: self.toes.clamp(1, MAX_DIGITS as u32),
            toe_length: fix(self.toe_length, 0.4, 2.0, d.toe_length),
            toe_thickness: fix(self.toe_thickness, 0.5, 2.0, d.toe_thickness),
            big_toe: fix(self.big_toe, 0.6, 1.6, d.big_toe),
            taper: fix(self.taper, 0.0, 1.0, d.taper),
            spread: fix(self.spread, 0.0, 1.0, d.spread),
            width: fix(self.width, 0.6, 1.6, d.width),
            heel: fix(self.heel, 0.6, 1.5, d.heel),
            arch: fix(self.arch, 0.0, 1.0, d.arch),
            ankle: fix(self.ankle, 0.0, 1.0, d.ankle),
            ankle_bend: fix(self.ankle_bend, -1.0, 1.0, d.ankle_bend),
            flat_sole: self.flat_sole,
            knuckles: fix(self.knuckles, 0.0, 1.0, d.knuckles),
            softness: fix(self.softness, 0.0, 1.0, d.softness),
            claws: fix(self.claws, 0.0, 1.0, d.claws),
            curl: self.curl.map(|c| fix(c, -0.6, 1.0, 0.0)),
        }
    }

    /// The right foot these settings make.
    pub fn body(&self) -> Body {
        let s = self.sanitized();
        let ball = 0.37 * s.width;
        let heel_width = 0.25 * (0.5 + 0.5 * s.width) * s.heel;
        let mut body = Body {
            web: 0.025 + 0.04 * s.softness,
            core: Group::new(0.04 + 0.06 * s.softness),
            cut_blend: 0.03,
            ..Body::default()
        };

        // Where each toe leaves the foot, and how thick it is there.
        let n = s.toes as usize;
        let lesser = n.saturating_sub(1).max(1);
        let lesser_spacing = ball * 0.55 / (lesser.max(2) - 1) as f32;
        let squeeze = if n >= 3 { (lesser_spacing / 0.066).min(1.0) } else { 1.0 };
        let mut heads = Vec::with_capacity(n);
        for j in 0..n {
            let big = j == 0;
            let along = if n == 1 { 0.0 } else { j as f32 / (n - 1) as f32 };
            let small = if n <= 2 { 0.0 } else { (j.max(1) - 1) as f32 / (n - 2).max(1) as f32 };
            let (x, z, radius) = if n == 1 {
                (0.03, 0.72, 0.065 * s.toe_thickness)
            } else if big {
                (ball * 0.37, 0.72, 0.05 * s.big_toe * s.toe_thickness)
            } else {
                let x = ball * 0.15 - small * ball * 0.55;
                (x, 0.7 - 0.1 * small.powf(1.2), profile(TOE_RADIUS, small) * s.toe_thickness * squeeze)
            };
            heads.push((Vec3::new(x, radius + 0.004, z), radius, big, along, small));
        }

        // Heel, ankle, the long bones and pads of the sole, the leg.
        body.core.push(Shape::ellipsoid(
            Vec3::new(-0.01, 0.1 * s.heel, 0.135),
            Vec3::new(heel_width * 0.5, 0.1 * s.heel, 0.13 * s.heel),
        ));
        body.core.push(Shape::sphere(ANKLE, 0.095));
        let (first, last) = (heads[0], heads[n - 1]);
        let outside = if n == 1 { Vec3::new(-ball * 0.3, 0.045, 0.66) } else { last.0 };
        body.core.push(Shape::cone(Vec3::new(-heel_width * 0.3, 0.055, 0.17), Vec3::new(outside.x * 0.92, 0.042, outside.z - 0.03), 0.055, 0.042));
        body.core.push(Shape::cone(
            Vec3::new(heel_width * 0.15, 0.1 + 0.05 * s.arch, 0.24),
            Vec3::new(first.0.x * 0.9, first.0.y, first.0.z - 0.04),
            0.065,
            first.1,
        ));
        for &(head, radius, _, along, _) in &heads {
            let root = Vec3::new(head.x * 0.35, 0.1 + 0.05 * s.arch * (1.0 - along), 0.36);
            body.core.push(Shape::cone(root, head, 0.05, radius * 1.02));
        }
        body.core.push(Shape::cone(Vec3::new(0.01, 0.2, 0.3), Vec3::new(ball * 0.05, 0.08, 0.58), 0.085, 0.06));
        let across = Vec3::new(first.0.x - outside.x, 0.0, first.0.z - outside.z);
        let middle = (first.0 + outside) * 0.5;
        body.core.push(Shape::Ellipsoid {
            center: Vec3::new(middle.x, 0.04, middle.z),
            axes: Mat3::from_rotation_y(-across.z.atan2(across.x)),
            radii: Vec3::new(across.length() * 0.5 + 0.04, 0.04, 0.06),
        });
        // The sole under both sides of the midfoot. The arch is cut out of the inside one.
        body.core.push(Shape::ellipsoid(Vec3::new(-heel_width * 0.2, 0.04, 0.42), Vec3::new(0.075, 0.04, 0.2)));
        body.core.push(Shape::ellipsoid(Vec3::new(ball * 0.25, 0.04, 0.42), Vec3::new(0.07, 0.04, 0.2)));

        let leg = Quat::from_rotation_x(s.ankle_bend * 0.6);
        let up = leg * Vec3::Y;
        let top = ANKLE + up * (0.06 + s.ankle * 0.75);
        body.core.push(Shape::cone(ANKLE + up * 0.02, top, 0.095, 0.11));
        body.core.push(Shape::cone(Vec3::new(0.0, 0.16, 0.06), top + leg * Vec3::new(0.0, 0.0, -0.05), 0.05, 0.065));
        if s.knuckles > 0.05 {
            let r = 0.03 + 0.02 * s.knuckles;
            body.core.push(Shape::sphere(ANKLE + Vec3::new(0.075, 0.005, 0.0), r));
            body.core.push(Shape::sphere(ANKLE + Vec3::new(-0.085, -0.03, -0.02), r));
        }

        for (j, &(head, radius, big, along, small)) in heads.iter().enumerate() {
            let fan = if n == 1 { 0.0 } else { (0.5 - along) * s.spread * 0.7 };
            let tip = radius * (1.0 - 0.35 * s.taper);
            let c = s.curl[j];
            let (lengths, radii, angles) = if big || n == 1 {
                let reach = 0.21 * s.toe_length * s.big_toe.sqrt();
                (
                    vec![reach * 0.55, (reach * 0.45 - tip).max(0.02)],
                    vec![radius, radius * 0.97, tip],
                    vec![-0.08 + c * 0.6, 0.12 + c * 0.6],
                )
            } else {
                let reach = profile(TOE_LENGTH, small) * s.toe_length;
                (
                    vec![reach * 0.45, reach * 0.25, (reach * 0.3 - tip).max(0.01)],
                    vec![radius, radius * 0.95, radius * 0.9, tip],
                    vec![-0.12 + c * 0.7, 0.32 + c * 0.9, 0.1 + c * 0.5],
                )
            };
            let toe = Chain {
                start: head,
                dir: Quat::from_rotation_y(fan) * Vec3::Z,
                bend: -Vec3::Y,
                lengths,
                radii,
                angles,
                knuckles: s.knuckles,
                claw: s.claws,
                blend: radius * 0.3,
            };
            body.digits.push(toe.group());
        }

        if s.arch > 0.02 {
            body.cuts.push(Shape::ellipsoid(Vec3::new(ball * 0.32 + 0.02, -0.01, 0.43), Vec3::new(0.085, 0.03 + 0.07 * s.arch, 0.17)));
        }
        if s.flat_sole {
            body.floor = Some((0.0, 0.012));
        }
        body.translate(-ANKLE);
        body
    }
}

#[cfg(test)]
mod tests {
    use sculpt_geom::EdgeCensus;

    use super::*;

    const CELL: f32 = 1.0 / 90.0;

    #[test]
    fn the_default_foot_is_one_closed_mesh_about_one_long() {
        let mesh = FootShape::default().body().mesh(CELL).unwrap();
        assert!(EdgeCensus::of(&mesh).is_closed());
        let size = mesh.bounds().size();
        assert!(size.z > 0.9 && size.z < 1.2, "heel to toe {size:?}");
        assert!(size.x > 0.28 && size.x < 0.55, "across {size:?}");
    }

    #[test]
    fn every_toe_count_builds_closed() {
        for toes in 0..=7 {
            let mesh = FootShape { toes, ..FootShape::default() }.body().mesh(CELL).unwrap();
            assert!(EdgeCensus::of(&mesh).is_closed(), "{toes} toes");
        }
    }

    #[test]
    fn a_flat_sole_stands_level_on_the_ground() {
        let foot = FootShape::default().body();
        let ground = foot.floor.unwrap().0;
        assert!((ground + ANKLE.y).abs() < 1e-6, "the ground is below the ankle");
        let bounds = foot.mesh(CELL).unwrap().bounds();
        assert!((bounds.min.y - ground).abs() < CELL * 1.5);
    }

    #[test]
    fn the_big_toe_is_on_the_plus_x_side_of_a_right_foot() {
        let bounds = FootShape::default().body().field().bounds();
        // The front of the foot leans toward the big toe.
        let field = FootShape::default().body().field();
        let near_tip = |x: f32| field.distance(Vec3::new(x, -ANKLE.y + 0.04, 0.7 - ANKLE.z), 1.0);
        assert!(near_tip(0.13) < 0.0, "big toe");
        assert!(bounds.max.x > 0.12);
    }

    #[test]
    fn the_arch_lifts_the_inside_of_the_sole() {
        let probe = Vec3::new(0.37 * 0.27, 0.02, 0.43) - ANKLE;
        let low = FootShape { arch: 0.0, ..FootShape::default() }.body().field();
        let high = FootShape { arch: 1.0, ..FootShape::default() }.body().field();
        assert!(low.distance(probe, 1.0) < 0.0, "a flat foot is solid there");
        assert!(high.distance(probe, 1.0) > 0.0, "a high arch leaves room there");
    }

    #[test]
    fn every_pose_and_style_builds() {
        for style in FootStyle::ALL {
            for pose in FootPose::ALL {
                let mut shape = FootShape::default();
                style.apply(&mut shape);
                pose.apply(&mut shape);
                let mesh = shape.body().mesh(1.0 / 60.0).unwrap();
                assert!(EdgeCensus::of(&mesh).is_closed(), "{} {}", style.name(), pose.name());
            }
        }
    }
}
