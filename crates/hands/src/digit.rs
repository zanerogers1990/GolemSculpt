//! One finger, thumb or toe: a chain of bones that bends at each joint toward one side.

use glam::{Quat, Vec3};

use crate::body::Group;
use crate::shapes::Shape;

#[derive(Clone, Debug)]
pub struct Chain {
    /// The first joint (the knuckle where the digit leaves the palm or foot).
    pub start: Vec3,
    /// The way the digit points held straight. Unit length.
    pub dir: Vec3,
    /// The way it curls: toward the palm, or the sole. Unit length, square to `dir`.
    pub bend: Vec3,
    /// Bone lengths, joint to joint. The last ends at the centre of the round tip.
    pub lengths: Vec<f32>,
    /// One radius per joint, so one more than there are bones.
    pub radii: Vec<f32>,
    /// How far each joint bends toward `bend`, in radians, one per bone.
    pub angles: Vec<f32>,
    /// 0 to 1: how much the joints stand out on the back.
    pub knuckles: f32,
    /// 0 to 1: how long a claw grows from the tip; 0 is none.
    pub claw: f32,
    /// Fillet between the bones.
    pub blend: f32,
}

/// Where each joint ends up, and the way the bone after it points and bends.
pub struct Posed {
    pub joints: Vec<Vec3>,
    pub dirs: Vec<Vec3>,
    pub bends: Vec<Vec3>,
}

impl Chain {
    pub fn pose(&self) -> Posed {
        let (mut p, mut d, mut b) = (self.start, self.dir, self.bend);
        let mut posed = Posed { joints: vec![p], dirs: Vec::new(), bends: Vec::new() };
        for (i, &length) in self.lengths.iter().enumerate() {
            // Turning about d x b carries d toward b.
            let axis = d.cross(b).normalize_or_zero();
            if axis != Vec3::ZERO {
                let turn = Quat::from_axis_angle(axis, self.angles.get(i).copied().unwrap_or(0.0));
                d = (turn * d).normalize();
                b = (turn * b).normalize();
            }
            p += d * length;
            posed.joints.push(p);
            posed.dirs.push(d);
            posed.bends.push(b);
        }
        posed
    }

    /// The round end of the digit: the centre of the last joint.
    pub fn tip(&self) -> Vec3 {
        *self.pose().joints.last().unwrap()
    }

    pub fn group(&self) -> Group {
        let mut group = Group::new(self.blend);
        let posed = self.pose();
        let radius = |i: usize| self.radii.get(i).copied().unwrap_or(*self.radii.last().unwrap_or(&0.01));
        for i in 0..self.lengths.len() {
            group.push(Shape::cone(posed.joints[i], posed.joints[i + 1], radius(i), radius(i + 1)));
        }
        // A knuckle is a lump on the back of each joint along the digit.
        if self.knuckles > 0.05 {
            for i in 1..self.lengths.len() {
                let back = -(posed.bends[i - 1] + posed.bends[i]).normalize_or_zero();
                let r = radius(i);
                group.push(Shape::sphere(posed.joints[i] + back * r * 0.35, r * (0.5 + 0.45 * self.knuckles)));
            }
        }
        if self.claw > 0.01 && !self.lengths.is_empty() {
            let last = self.lengths.len();
            let (p, d, b, r) = (posed.joints[last], posed.dirs[last - 1], posed.bends[last - 1], radius(last));
            let length = self.claw * r * 4.0;
            let base = p - b * r * 0.25 + d * r * 0.1;
            let middle = base + d * (length * 0.5 + r * 0.5);
            let point = middle + (d * 0.8 + b * 0.6).normalize() * length * 0.55;
            group.push(Shape::cone(base, middle, r * 0.62, r * 0.38));
            group.push(Shape::cone(middle, point, r * 0.38, r * 0.04));
        }
        group
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn finger(angles: Vec<f32>) -> Chain {
        Chain {
            start: Vec3::ZERO,
            dir: Vec3::Y,
            bend: -Vec3::Z,
            lengths: vec![0.2, 0.13, 0.1],
            radii: vec![0.05, 0.047, 0.043, 0.04],
            angles,
            knuckles: 0.4,
            claw: 0.0,
            blend: 0.01,
        }
    }

    #[test]
    fn a_straight_chain_ends_its_full_length_along_its_direction() {
        let tip = finger(vec![0.0; 3]).tip();
        assert!(tip.distance(Vec3::new(0.0, 0.43, 0.0)) < 1e-5);
    }

    #[test]
    fn curling_bends_toward_the_bend_side_and_shortens_the_reach() {
        let tip = finger(vec![1.4, 1.7, 1.2]).tip();
        assert!(tip.z < -0.1, "curled toward the palm: {tip:?}");
        assert!(tip.y < 0.25);
    }

    #[test]
    fn a_claw_reaches_past_the_tip() {
        let mut clawed = finger(vec![0.0; 3]);
        clawed.claw = 1.0;
        let plain = finger(vec![0.0; 3]).group().bounds();
        assert!(clawed.group().bounds().max.y > plain.max.y + 0.05);
    }
}
