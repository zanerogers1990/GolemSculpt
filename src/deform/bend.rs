//! Bend and Bend Arc: Barr's global bend, generalised to a soft range.
//!
//! The axis line through the middle of the box is the bend's spine. Over
//! the range the spine curves round in one plane (the axis and the bend's
//! direction across it), its curvature following the profile's window, so
//! that it turns through the whole angle by the end of the range; beyond the
//! range it goes on straight. Every cross-section stays square to the spine
//! and keeps its shape: a point a distance `x` from the spine, in the bend's
//! plane, sits that far from the bent spine along its normal, and its
//! distance out of the plane is left as it is. Material on the inside of the
//! bend is pressed together and on the outside drawn out, as clay bends.
//!
//! Where the spine goes is the integral of the direction it is turned to,
//! `(cos θ(t), sin θ(t))`. With no softness that is a circular arc, worked out
//! in closed form by Barr. With softness the curvature is not constant and
//! the integral has none, so it is tabulated once per setting (Simpson's rule
//! over each of `CELLS` cells) and read back with cubic Hermite through the
//! cells' ends, whose slopes are known exactly. Its error is of the order of
//! the cell's length to the fourth power: a millionth of a millionth of the
//! range, the hard bend's arc included. A table is also the cheaper way of
//! the two: one lookup a vertex, not a closed form with a division.
//!
//! **Which end stays.** The anchor's own cross-section stays put and square to
//! the axis, and the bend is measured outward from it both ways: from the
//! start of the range, from its middle (both halves bend, mirror images of
//! each other), or from its end, which is the start with the axis turned
//! round, so that the end stays and the start swings.
//!
//! **Bend Arc** is the same bend, set by where its tip handle is instead of by
//! an angle: the spine is laid along the one arc that leaves the anchor along
//! the axis and ends at the tip handle, stretched or shortened to fit it. The
//! profile's window is symmetric about the middle of the range, so the bent
//! spine is too, and the line from its start to its end (the chord) runs in
//! the direction the spine has at its middle, half the angle; so the angle
//! that reaches the handle is twice the chord's angle off the axis, for any
//! softness. The arc's length then follows from the table.

use super::profile::Profile;
use super::{Anchor, Axes, Params};

/// Cells in the spine's table. 2048 keeps the table to 32 KB and its error
/// to about a billionth of the range on a bend of a whole turn, far below a
/// float's resolution on the model.
const CELLS: usize = 2048;

pub struct Bend {
    /// The whole angle turned over the range, in radians.
    total: f64,
    /// The bend's direction across the axis, `(u, v)`: the spine curves toward it.
    dir: (f64, f64),
    /// How much longer the spine is laid along its curve inside the range (Bend Arc).
    scale: f64,
    /// Where in the range the anchor is: 0 (start) or 0.5 (middle). An anchor
    /// at the end is the start with the axis turned round (`flip`).
    anchor_t: f64,
    flip: bool,
    /// The spine at `t = k / CELLS`, for a range of length 1 and no stretch,
    /// and the cosine and sine of its angle there (so that a lookup needs no
    /// trigonometry of its own).
    table: Vec<[f64; 4]>,
    /// The spine and its angle at the anchor, which everything is measured from.
    at_anchor: ([f64; 2], f64),
}

impl Bend {
    /// A Bend: through the angle the settings give, toward their direction.
    pub fn turned(p: &Params, profile: &Profile) -> Bend {
        let a = (p.direction as f64).to_radians();
        Bend::new((p.angle as f64).to_radians(), (a.cos(), a.sin()), 1.0, p.anchor, profile)
    }

    /// A Bend Arc: whatever angle, direction and stretch take the far end of
    /// the range to the tip handle. `None` if the handle has been pulled back
    /// onto the anchor, where no arc can end.
    pub fn to_tip(p: &Params, axes: &Axes, profile: &Profile) -> Option<Bend> {
        let anchor = if p.anchor == Anchor::End { Anchor::End } else { Anchor::Start };
        let flip = anchor == Anchor::End;
        let length = profile.length();
        // Where the handle is from the anchor, along the axis and across it,
        // with the axis turned round for an anchor at the end.
        let offset = |i: usize| p.tip[i] as f64;
        let along = length * (1.0 + offset(axes.along) * if flip { -1.0 } else { 1.0 });
        let (u, v) = (offset(axes.across[0]) * length, offset(axes.across[1]) * length);
        let across = u.hypot(v);
        let reach = along.hypot(across);
        if reach < length * 1e-6 {
            return None;
        }
        if across < length * 1e-9 {
            // Straight out (or back) along the axis: no bend, only the length.
            return Some(Bend::new(0.0, (1.0, 0.0), (along / length).max(0.01), anchor, profile));
        }
        let total = 2.0 * across.atan2(along);
        let mut bend = Bend::new(total, (u / across, v / across), 1.0, anchor, profile);
        let [x, y, ..] = bend.table[CELLS];
        bend.scale = reach / (length * x.hypot(y)).max(1e-12);
        Some(bend)
    }

    fn new(total: f64, dir: (f64, f64), scale: f64, anchor: Anchor, profile: &Profile) -> Bend {
        let (anchor_t, flip) = match anchor {
            Anchor::Start => (0.0, false),
            Anchor::Middle => (0.5, false),
            Anchor::End => (0.0, true),
        };
        let angle = |t: f64| total * profile.ramp_t(t);
        let h = 1.0 / CELLS as f64;
        let mut table = Vec::with_capacity(CELLS + 1);
        let mut at = [0.0f64; 2];
        table.push([0.0, 0.0, angle(0.0).cos(), angle(0.0).sin()]);
        for k in 0..CELLS {
            let t = k as f64 * h;
            let (a, m, b) = (angle(t), angle(t + 0.5 * h), angle(t + h));
            at[0] += h / 6.0 * (a.cos() + 4.0 * m.cos() + b.cos());
            at[1] += h / 6.0 * (a.sin() + 4.0 * m.sin() + b.sin());
            table.push([at[0], at[1], b.cos(), b.sin()]);
        }
        let mut bend = Bend { total, dir, scale, anchor_t, flip, table, at_anchor: ([0.0; 2], 0.0) };
        bend.at_anchor = (bend.spine(anchor_t, profile, 1.0), angle(anchor_t));
        bend
    }

    /// The spine at `t` inside the range, for a range of length 1 and no stretch.
    #[inline]
    fn table_at(&self, t: f64) -> [f64; 2] {
        let h = 1.0 / CELLS as f64;
        let k = ((t * CELLS as f64) as usize).min(CELLS - 1);
        let tau = t * CELLS as f64 - k as f64;
        let (c0, c1) = (self.table[k], self.table[k + 1]);
        // Cubic Hermite: the cell's two ends and the spine's direction at each.
        let (t2, t3) = (tau * tau, tau * tau * tau);
        let (h00, h10, h01, h11) = (2.0 * t3 - 3.0 * t2 + 1.0, t3 - 2.0 * t2 + tau, -2.0 * t3 + 3.0 * t2, t3 - t2);
        [
            h00 * c0[0] + h10 * h * c0[2] + h01 * c1[0] + h11 * h * c1[2],
            h00 * c0[1] + h10 * h * c0[3] + h01 * c1[1] + h11 * h * c1[3],
        ]
    }

    /// The spine at `t`, in units along the axis from the start of the range,
    /// for a range stretched `scale` times. Beyond the range it goes on
    /// straight, unstretched, the way it was heading at the range's end.
    fn spine(&self, t: f64, profile: &Profile, scale: f64) -> [f64; 2] {
        let length = profile.length();
        let inside = scale * length;
        if t <= 0.0 {
            [t * length, 0.0]
        } else if t >= 1.0 {
            let [x, y, cos, sin] = self.table[CELLS];
            let past = (t - 1.0) * length;
            [x * inside + past * cos, y * inside + past * sin]
        } else {
            let [x, y] = self.table_at(t);
            [x * inside, y * inside]
        }
    }

    #[inline]
    pub fn apply(&self, profile: &Profile, s: f64, u: f64, v: f64) -> Option<(f64, f64, f64)> {
        let (lo, hi) = (profile.lo, profile.hi);
        let s = if self.flip { lo + hi - s } else { s };
        let t = profile.t(s);
        if self.anchor_t == 0.0 && t <= 0.0 {
            return None;
        }
        // The spine and its turn at this point, measured from the anchor's.
        let (anchor_at, anchor_angle) = self.at_anchor;
        let angle = self.total * profile.ramp_t(t) - anchor_angle;
        let [x, y] = self.spine(t, profile, self.scale);
        let (dx, dy) = (x - anchor_at[0] * self.scale, y - anchor_at[1] * self.scale);
        let (ca, sa) = (anchor_angle.cos(), anchor_angle.sin());
        let (along, out) = (dx * ca + dy * sa, -dx * sa + dy * ca);
        // The point's place across the spine: in the bend's plane, and out of it.
        let (du, dv) = self.dir;
        let (inward, side) = (u * du + v * dv, -u * dv + v * du);
        let (cos, sin) = (angle.cos(), angle.sin());
        let s2 = lo + self.anchor_t * profile.length() + along - inward * sin;
        let across = out + inward * cos;
        let (u2, v2) = (across * du - side * dv, across * dv + side * du);
        Some((if self.flip { lo + hi - s2 } else { s2 }, u2, v2))
    }
}
