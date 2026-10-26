//! A hand from its sliders.
//!
//! Built as a right hand in hand units: the wrist joint at the origin, the fingers pointing up +Y,
//! the back of the hand facing +Z and the thumb on the -X side, so a camera on +Z sees the back of a
//! raised right hand. A hand of ordinary build is about 1 long from the wrist to the tip of the middle
//! finger. The left hand is the mirror image.
//!
//! The proportions follow a real adult hand: the palm about half the hand's length and a little less
//! wide, the middle finger longest, the little finger's knuckle set lower than the rest, a thumb about
//! as long as the palm with its root down by the wrist. A count of fingers other than four spreads
//! those four over however many there are, so three fingers are an index, a middle-ring and a little
//! finger.

use glam::{Mat3, Quat, Vec3};

use crate::body::{Body, Group};
use crate::digit::Chain;
use crate::shapes::Shape;
use crate::{MAX_DIGITS, fix, profile};

/// Finger length against the middle finger's, index to little.
const LENGTH: [f32; 4] = [0.88, 1.0, 0.94, 0.75];
/// Knuckle height against the palm's length.
const KNUCKLE: [f32; 4] = [0.97, 1.0, 0.965, 0.885];
/// Finger radius at the base.
const RADIUS: [f32; 4] = [0.046, 0.048, 0.045, 0.039];
/// How far each knuckle stands toward the back: the knuckles arch across the hand.
const ARCH: [f32; 4] = [0.0, 0.012, 0.004, -0.014];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HandShape {
    pub fingers: u32,
    pub thumb: bool,
    pub finger_length: f32,
    pub finger_thickness: f32,
    /// How much thinner the fingertips are than the bases, 0 to 1.
    pub taper: f32,
    /// How far the fingers fan apart, 0 to 1.
    pub spread: f32,
    pub palm_width: f32,
    pub palm_length: f32,
    pub palm_thickness: f32,
    pub thumb_length: f32,
    /// How far the thumb swings out from the hand, 0 to 1.
    pub thumb_angle: f32,
    pub knuckles: f32,
    /// How much forearm there is past the wrist, 0 to 1.
    pub wrist: f32,
    /// How round the joins are, 0 to 1.
    pub softness: f32,
    pub claws: f32,
    /// How far each finger curls, index first, 0 straight to 1 a fist.
    pub curl: [f32; MAX_DIGITS],
    pub thumb_curl: f32,
}

impl Default for HandShape {
    fn default() -> HandShape {
        HandShape {
            fingers: 4,
            thumb: true,
            finger_length: 1.0,
            finger_thickness: 1.0,
            taper: 0.35,
            spread: 0.25,
            palm_width: 1.0,
            palm_length: 1.0,
            palm_thickness: 1.0,
            thumb_length: 1.0,
            thumb_angle: 0.4,
            knuckles: 0.35,
            wrist: 0.2,
            softness: 0.5,
            claws: 0.0,
            curl: [0.0; MAX_DIGITS],
            thumb_curl: 0.0,
        }
    }
}

/// The poses offered as buttons.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandPose {
    Flat,
    Relaxed,
    Spread,
    Grip,
    Fist,
    Point,
}

impl HandPose {
    pub const ALL: [HandPose; 6] =
        [HandPose::Flat, HandPose::Relaxed, HandPose::Spread, HandPose::Grip, HandPose::Fist, HandPose::Point];

    pub fn name(self) -> &'static str {
        match self {
            HandPose::Flat => "Flat",
            HandPose::Relaxed => "Relaxed",
            HandPose::Spread => "Spread",
            HandPose::Grip => "Grip",
            HandPose::Fist => "Fist",
            HandPose::Point => "Point",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            HandPose::Flat => "Fingers straight and together, thumb alongside.",
            HandPose::Relaxed => "The hand at rest: fingers gently curled, more toward the little finger.",
            HandPose::Spread => "Fingers and thumb stretched wide apart.",
            HandPose::Grip => "Curled as if round a handle.",
            HandPose::Fist => "Fingers closed tight, thumb folded over.",
            HandPose::Point => "First finger straight, the others closed.",
        }
    }

    /// Sets the curls, the spread and the thumb; the hand's build is left alone.
    pub fn apply(self, shape: &mut HandShape) {
        let n = shape.fingers.clamp(1, MAX_DIGITS as u32) as usize;
        let along = |i: usize| if n == 1 { 0.0 } else { i as f32 / (n - 1) as f32 };
        let (thumb_curl, spread, thumb_angle) = match self {
            HandPose::Flat => (0.0, 0.12, 0.3),
            HandPose::Relaxed => (0.25, 0.25, 0.4),
            HandPose::Spread => (0.0, 1.0, 1.0),
            HandPose::Grip => (0.45, 0.08, 0.7),
            HandPose::Fist => (0.75, 0.0, 0.25),
            HandPose::Point => (0.7, 0.05, 0.3),
        };
        for (i, c) in shape.curl.iter_mut().enumerate() {
            *c = match self {
                HandPose::Flat => 0.0,
                HandPose::Relaxed => 0.2 + 0.22 * along(i).min(1.0),
                HandPose::Spread => 0.04,
                HandPose::Grip => 0.55,
                HandPose::Fist => 1.0,
                HandPose::Point if i == 0 => 0.0,
                HandPose::Point => 1.0,
            };
        }
        shape.thumb_curl = thumb_curl;
        shape.spread = spread;
        shape.thumb_angle = thumb_angle;
    }
}

/// Builds offered as buttons: everything but the pose.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HandStyle {
    Human,
    Slender,
    Chunky,
    Cartoon,
    Creature,
}

impl HandStyle {
    pub const ALL: [HandStyle; 5] =
        [HandStyle::Human, HandStyle::Slender, HandStyle::Chunky, HandStyle::Cartoon, HandStyle::Creature];

    pub fn name(self) -> &'static str {
        match self {
            HandStyle::Human => "Human",
            HandStyle::Slender => "Slender",
            HandStyle::Chunky => "Chunky",
            HandStyle::Cartoon => "Cartoon",
            HandStyle::Creature => "Creature",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            HandStyle::Human => "An ordinary adult hand.",
            HandStyle::Slender => "Long, fine fingers on a narrow palm.",
            HandStyle::Chunky => "Short, thick fingers on a broad, heavy palm.",
            HandStyle::Cartoon => "Three thick, round fingers and a thumb, with soft joins.",
            HandStyle::Creature => "Long bony fingers with big knuckles and claws.",
        }
    }

    /// Sets the build, keeping the pose.
    pub fn apply(self, shape: &mut HandShape) {
        let (curl, thumb_curl, spread, thumb_angle) = (shape.curl, shape.thumb_curl, shape.spread, shape.thumb_angle);
        *shape = HandShape::default();
        match self {
            HandStyle::Human => {}
            HandStyle::Slender => {
                shape.finger_length = 1.15;
                shape.finger_thickness = 0.82;
                shape.palm_width = 0.92;
                shape.palm_thickness = 0.85;
                shape.taper = 0.45;
                shape.knuckles = 0.6;
            }
            HandStyle::Chunky => {
                shape.finger_length = 0.85;
                shape.finger_thickness = 1.3;
                shape.palm_width = 1.15;
                shape.palm_thickness = 1.3;
                shape.taper = 0.15;
                shape.knuckles = 0.3;
            }
            HandStyle::Cartoon => {
                shape.fingers = 3;
                shape.finger_length = 0.9;
                shape.finger_thickness = 1.55;
                shape.palm_length = 0.85;
                shape.palm_thickness = 1.35;
                shape.thumb_length = 0.9;
                shape.taper = 0.0;
                shape.knuckles = 0.0;
                shape.softness = 1.0;
            }
            HandStyle::Creature => {
                shape.finger_length = 1.45;
                shape.finger_thickness = 0.8;
                shape.palm_length = 1.05;
                shape.taper = 0.55;
                shape.knuckles = 0.9;
                shape.claws = 0.7;
                shape.thumb_length = 1.15;
            }
        }
        (shape.curl, shape.thumb_curl, shape.spread, shape.thumb_angle) = (curl, thumb_curl, spread, thumb_angle);
    }
}

impl HandShape {
    /// Every value brought into the range its slider has, whatever it was.
    pub fn sanitized(&self) -> HandShape {
        let d = HandShape::default();
        HandShape {
            fingers: self.fingers.clamp(1, MAX_DIGITS as u32),
            thumb: self.thumb,
            finger_length: fix(self.finger_length, 0.4, 1.8, d.finger_length),
            finger_thickness: fix(self.finger_thickness, 0.5, 2.0, d.finger_thickness),
            taper: fix(self.taper, 0.0, 1.0, d.taper),
            spread: fix(self.spread, 0.0, 1.0, d.spread),
            palm_width: fix(self.palm_width, 0.6, 1.6, d.palm_width),
            palm_length: fix(self.palm_length, 0.6, 1.5, d.palm_length),
            palm_thickness: fix(self.palm_thickness, 0.5, 1.8, d.palm_thickness),
            thumb_length: fix(self.thumb_length, 0.5, 1.6, d.thumb_length),
            thumb_angle: fix(self.thumb_angle, 0.0, 1.0, d.thumb_angle),
            knuckles: fix(self.knuckles, 0.0, 1.0, d.knuckles),
            wrist: fix(self.wrist, 0.0, 1.0, d.wrist),
            softness: fix(self.softness, 0.0, 1.0, d.softness),
            claws: fix(self.claws, 0.0, 1.0, d.claws),
            curl: self.curl.map(|c| fix(c, 0.0, 1.0, 0.0)),
            thumb_curl: fix(self.thumb_curl, 0.0, 1.0, 0.0),
        }
    }

    /// The right hand these settings make.
    pub fn body(&self) -> Body {
        let s = self.sanitized();
        let width = 0.44 * s.palm_width;
        let length = 0.5 * s.palm_length;
        let thick = 0.13 * s.palm_thickness;
        let mut body = Body { web: 0.03 + 0.05 * s.softness, core: Group::new(0.035 + 0.06 * s.softness), ..Body::default() };

        // The palm: broad across the knuckles, narrowing over the bones of the wrist. Then the pads on
        // its inside, and the forearm.
        body.core.push(Shape::Box {
            center: Vec3::new(0.0, length * 0.6, -thick * 0.03),
            axes: Mat3::IDENTITY,
            half: Vec3::new(width * 0.45, length * 0.27, thick * 0.5),
            round: thick * 0.45,
        });
        body.core.push(Shape::Ellipsoid {
            center: Vec3::new(0.0, length * 0.24, -thick * 0.02),
            axes: Mat3::IDENTITY,
            radii: Vec3::new(width * 0.37, length * 0.24, thick * 0.52),
        });
        body.core.push(Shape::Ellipsoid {
            center: Vec3::new(width * 0.3, length * 0.42, -thick * 0.22),
            axes: Mat3::IDENTITY,
            radii: Vec3::new(0.065 * s.palm_width, 0.17 * s.palm_length, 0.055 * s.palm_thickness),
        });
        body.core.push(Shape::Ellipsoid {
            center: Vec3::new(0.0, length * 0.86, -thick * 0.22),
            axes: Mat3::IDENTITY,
            radii: Vec3::new(width * 0.47, 0.07, 0.05 * s.palm_thickness),
        });
        // Wider than it is thick, with a flat end to join an arm to.
        let (top, bottom) = (length * 0.12, -(0.03 + s.wrist * 0.6));
        let deep = 0.4 + 0.6 * s.palm_thickness;
        body.core.push(Shape::Box {
            center: Vec3::new(0.0, (top + bottom) * 0.5, 0.0),
            axes: Mat3::IDENTITY,
            half: Vec3::new(0.145 * (0.5 + 0.5 * s.palm_width), (top - bottom) * 0.5, 0.09 * deep),
            round: 0.085 * deep,
        });

        let n = s.fingers as usize;
        let spacing = width / n as f32;
        // Many fingers on a narrow palm come out thinner, so they still fit side by side.
        let squeeze = (spacing / 0.105).min(1.0);
        for i in 0..n {
            let along = if n == 1 { 0.4 } else { i as f32 / (n - 1) as f32 };
            let x = (i as f32 - (n - 1) as f32 * 0.5) * spacing;
            let radius = profile(RADIUS, along) * s.finger_thickness * squeeze;
            let knuckle = Vec3::new(x, length * profile(KNUCKLE, along), thick * 0.12 + profile(ARCH, along));

            // The bone in the palm, ending in the big knuckle on the back of the hand.
            let root = Vec3::new(x * 0.55, length * 0.12, 0.0);
            body.core.push(Shape::cone(root, knuckle, thick * 0.42, (thick * 0.5).max(radius * 1.05)));
            if s.knuckles > 0.05 {
                body.core.push(Shape::sphere(knuckle + Vec3::Z * thick * 0.32, radius * (0.45 + 0.45 * s.knuckles)));
            }

            let fan = if n == 1 { 0.0 } else { (along - 0.5) * s.spread * 0.9 };
            let reach = 0.44 * s.finger_length * profile(LENGTH, along);
            let tip = radius * (1.0 - 0.45 * s.taper);
            let curl = s.curl[i];
            let finger = Chain {
                start: knuckle,
                dir: Quat::from_rotation_z(-fan) * Vec3::Y,
                bend: -Vec3::Z,
                lengths: vec![reach * 0.47, reach * 0.29, (reach * 0.24 - tip).max(reach * 0.06)],
                radii: vec![radius * 1.03, radius + (tip - radius) * 0.4, radius + (tip - radius) * 0.75, tip],
                angles: vec![curl * 1.45, curl * 1.75, curl * 1.2],
                knuckles: s.knuckles,
                claw: s.claws,
                blend: radius * 0.3,
            };
            body.digits.push(finger.group());
        }

        if s.thumb {
            let out = 0.2 + s.thumb_angle * 0.8;
            let tilt = 0.3 + s.thumb_angle * 0.3;
            let raised = Quat::from_rotation_z(out) * Vec3::Y;
            let tilt_axis = raised.cross(-Vec3::Z).normalize();
            let dir = Quat::from_axis_angle(tilt_axis, tilt) * raised;
            // The thumb folds across the palm, toward the little finger.
            let across = Vec3::new(1.0, 0.0, -0.35);
            let bend = (across - dir * dir.dot(across)).normalize();
            let reach = s.thumb_length;
            let radius = 0.058 * s.finger_thickness;
            let tip = radius * (1.0 - 0.35 * s.taper);
            let curl = s.thumb_curl;
            let root = Vec3::new(-width * 0.3, length * 0.16, -thick * 0.18);
            let thumb = Chain {
                start: root,
                dir,
                bend,
                lengths: vec![0.21 * reach, 0.165 * reach, (0.15 * reach - tip).max(0.03)],
                radii: vec![radius * 1.15, radius * 0.98, radius * 0.95, tip],
                angles: vec![curl * 0.6, curl * 0.85, curl * 1.0],
                knuckles: s.knuckles,
                claw: s.claws,
                blend: radius * 0.3,
            };
            body.digits.push(thumb.group());
            // The ball of the thumb.
            body.core.push(Shape::Ellipsoid {
                center: Vec3::new(-width * 0.24, length * 0.3, -thick * 0.28),
                axes: Mat3::from_rotation_z(out * 0.6),
                radii: Vec3::new(0.085 * s.palm_width, 0.14 * s.palm_length, 0.06 * s.palm_thickness),
            });
        }
        body
    }
}

#[cfg(test)]
mod tests {
    use sculpt_geom::EdgeCensus;

    use super::*;

    const CELL: f32 = 1.0 / 90.0;

    #[test]
    fn the_default_hand_is_one_closed_mesh_about_one_long() {
        let mesh = HandShape::default().body().mesh(CELL).unwrap();
        assert!(EdgeCensus::of(&mesh).is_closed());
        let size = mesh.bounds().size();
        assert!(size.y > 0.95 && size.y < 1.6, "wrist to fingertip {size:?}");
        assert!(size.x > 0.4 && size.x < 0.9, "across {size:?}");
        assert!(size.z < 0.4, "a hand is flat {size:?}");
    }

    #[test]
    fn every_finger_count_with_and_without_a_thumb_builds_closed() {
        for fingers in 0..=7 {
            for thumb in [true, false] {
                let shape = HandShape { fingers, thumb, ..HandShape::default() };
                let mesh = shape.body().mesh(CELL).unwrap();
                assert!(EdgeCensus::of(&mesh).is_closed(), "{fingers} fingers, thumb {thumb}");
            }
        }
    }

    #[test]
    fn a_fist_is_shorter_than_an_open_hand_and_a_spread_hand_wider() {
        let mut fist = HandShape::default();
        HandPose::Fist.apply(&mut fist);
        let mut spread = HandShape::default();
        HandPose::Spread.apply(&mut spread);
        let mut flat = HandShape::default();
        HandPose::Flat.apply(&mut flat);
        let size = |s: &HandShape| s.body().field().bounds().size();
        assert!(size(&fist).y < size(&flat).y - 0.2);
        assert!(size(&spread).x > size(&flat).x + 0.1);
    }

    #[test]
    fn the_thumb_is_on_the_minus_x_side_of_a_right_hand() {
        let with = HandShape::default().body().field().bounds();
        let without = HandShape { thumb: false, ..HandShape::default() }.body().field().bounds();
        assert!(with.min.x < without.min.x - 0.1);
        assert!((with.max.x - without.max.x).abs() < 0.02);
    }

    #[test]
    fn a_style_keeps_the_pose_and_a_pose_keeps_the_build() {
        let mut shape = HandShape::default();
        HandPose::Fist.apply(&mut shape);
        HandStyle::Cartoon.apply(&mut shape);
        assert_eq!(shape.fingers, 3);
        assert_eq!(shape.curl[0], 1.0);
        HandPose::Point.apply(&mut shape);
        assert_eq!(shape.fingers, 3);
        assert_eq!((shape.curl[0], shape.curl[1]), (0.0, 1.0));
    }

    #[test]
    fn nonsense_values_are_brought_into_range() {
        let shape = HandShape { fingers: 1000, finger_length: f32::NAN, spread: -4.0, ..HandShape::default() };
        let fixed = shape.sanitized();
        assert_eq!(fixed.fingers, MAX_DIGITS as u32);
        assert_eq!(fixed.finger_length, 1.0);
        assert_eq!(fixed.spread, 0.0);
        assert_eq!(HandShape { fingers: 0, ..shape }.sanitized().fingers, 1);
    }
}
