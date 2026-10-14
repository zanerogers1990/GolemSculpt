//! Flatten and Stretch: the range made shorter or longer along the axis.
//!
//! Both are one deformation. Inside the range each stretch of the axis is
//! made `k` times its length (Stretch's factor, or one less Flatten's amount,
//! so that a Flatten of 100% presses the range to nothing), easing back to
//! its own length over the soft ends: the local factor is `1 + (k - 1) w(t)`.
//! A point's new place along the axis is where that adds up to from the
//! anchor, which is the window's integral and so exact; beyond the range the
//! rest of the shape is carried along whole.
//!
//! Stretch can also keep the shape's bulk: a slice made `λ` times thicker is
//! made `1 / √λ` times as wide each way across, so its volume stays the same,
//! as clay's does when it is pulled. Flatten never does: it is for pressing a
//! shape flatter, as onto a table.

use super::profile::Profile;
use super::{Anchor, Kind, Params};

pub struct Axial {
    /// How many times its length the middle of the range becomes.
    k: f64,
    /// Whether the cross-section thins as the range lengthens.
    keep_volume: bool,
    /// The window's integral at the anchor, which stays where it is.
    anchor_integral: f64,
}

impl Axial {
    pub fn new(p: &Params, profile: &Profile) -> Axial {
        let k = match p.kind {
            Kind::Flatten => 1.0 - p.flatten as f64,
            _ => p.stretch as f64,
        };
        let anchor_t = match p.anchor {
            Anchor::Start => 0.0,
            Anchor::Middle => 0.5,
            Anchor::End => 1.0,
        };
        Axial {
            k,
            keep_volume: p.kind == Kind::Stretch && p.keep_volume,
            anchor_integral: profile.integral_t(anchor_t),
        }
    }

    #[inline]
    pub fn apply(&self, profile: &Profile, s: f64, u: f64, v: f64) -> Option<(f64, f64, f64)> {
        let t = profile.t(s);
        let shift = profile.length() * (self.k - 1.0) * (profile.integral_t(t) - self.anchor_integral);
        let across = if self.keep_volume { 1.0 / (1.0 + (self.k - 1.0) * profile.window_t(t)).sqrt() } else { 1.0 };
        if shift == 0.0 && across == 1.0 {
            return None;
        }
        Some((s + shift, u * across, v * across))
    }
}
