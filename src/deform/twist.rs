//! Twist and Taper: Barr's two deformations of the cross-section along an
//! axis. Each point keeps its place along the axis; across it, a twist turns
//! it about the axis line by an angle that builds up along the range, and a
//! taper scales it toward or away from the line by a size that goes from the
//! start's to the end's. Both follow the profile's ramp, so with no softness
//! the angle and the size change at an even rate (Barr's linear twist and
//! taper) and with softness they ease in and out at the ends of the range.

use super::profile::Profile;
use super::{Anchor, Params};

pub struct Twist {
    /// The whole turn over the range, in radians.
    total: f64,
    /// The ramp at the anchor: the cross-section there does not turn.
    anchor_ramp: f64,
}

impl Twist {
    pub fn new(p: &Params, profile: &Profile) -> Twist {
        let anchor_t = match p.anchor {
            Anchor::Start => 0.0,
            Anchor::Middle => 0.5,
            Anchor::End => 1.0,
        };
        let total = (p.angle as f64).to_radians();
        Twist { total, anchor_ramp: profile.ramp_t(anchor_t) }
    }

    #[inline]
    pub fn apply(&self, profile: &Profile, s: f64, u: f64, v: f64) -> Option<(f64, f64, f64)> {
        let turn = self.total * (profile.ramp_t(profile.t(s)) - self.anchor_ramp);
        if turn == 0.0 {
            return None;
        }
        let (sin, cos) = turn.sin_cos();
        Some((s, u * cos - v * sin, u * sin + v * cos))
    }
}

pub struct Taper {
    from: f64,
    to: f64,
}

impl Taper {
    pub fn new(p: &Params) -> Taper {
        Taper { from: p.scales[0] as f64, to: p.scales[1] as f64 }
    }

    #[inline]
    pub fn apply(&self, profile: &Profile, s: f64, u: f64, v: f64) -> Option<(f64, f64, f64)> {
        let f = profile.ramp_t(profile.t(s));
        let size = self.from + (self.to - self.from) * f;
        if size == 1.0 {
            return None;
        }
        Some((s, u * size, v * size))
    }
}
