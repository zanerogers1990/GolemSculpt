//! How much of a deformation each point along the axis gets: the range and
//! its soft ends.
//!
//! Along the axis, `t` runs from 0 at the start of the range to 1 at its
//! end. The **window** `w(t)` is 1 inside the range and 0 outside it, and
//! with some softness it rises from 0 to 1 over the first stretch of the
//! range and falls back over the last, along a smoothstep, so that what is
//! done inside fades out with no crease at either end. Inflate and Lattice
//! use the window as it is.
//!
//! The deformations that build up along the axis (bend, twist, taper,
//! flatten, stretch) use the window as a rate: a bend's curvature, a twist's
//! turn per unit length, a stretch's extra length per unit length. What they
//! have built up by `t` is the window's integral, `I(t)`, which is worked out
//! exactly (`∫ smoothstep = x³ - x⁴/2`), and the **ramp** `F(t) = I(t) / I(1)`
//! is that as a share of the whole: 0 before the range, 1 after it. With no
//! softness the window is a box and the ramp is `t` itself, clamped: Barr's
//! linear deformations.

/// The range along the axis and how softly its ends ease in.
#[derive(Clone, Copy, Debug)]
pub struct Profile {
    /// Where the range starts and ends along the axis.
    pub lo: f64,
    pub hi: f64,
    /// The share of the range (in `t`) each end takes to ease in: half the
    /// softness, so that a softness of 1 eases all the way to the middle.
    ease: f64,
}

/// `∫₀ˣ smoothstep`.
#[inline]
fn eased_area(x: f64) -> f64 {
    x * x * x - 0.5 * x * x * x * x
}

#[inline]
fn smoothstep(x: f64) -> f64 {
    x * x * (3.0 - 2.0 * x)
}

impl Profile {
    pub fn new(lo: f64, hi: f64, softness: f64) -> Profile {
        Profile { lo, hi, ease: (softness * 0.5).clamp(0.0, 0.5) }
    }

    /// The range's length along the axis.
    pub fn length(&self) -> f64 {
        self.hi - self.lo
    }

    /// Where `s` along the axis is in the range: 0 at its start, 1 at its end.
    #[inline]
    pub fn t(&self, s: f64) -> f64 {
        (s - self.lo) / (self.hi - self.lo)
    }

    /// The window at `s`.
    #[inline]
    pub fn window(&self, s: f64) -> f64 {
        self.window_t(self.t(s))
    }

    #[inline]
    pub fn window_t(&self, t: f64) -> f64 {
        let e = self.ease;
        if !(0.0..=1.0).contains(&t) {
            0.0
        } else if t < e {
            smoothstep(t / e)
        } else if t > 1.0 - e {
            smoothstep((1.0 - t) / e)
        } else {
            1.0
        }
    }

    /// `I(t)`: the window's integral from the start of the range, in units of `t`.
    #[inline]
    pub fn integral_t(&self, t: f64) -> f64 {
        let e = self.ease;
        if t <= 0.0 {
            0.0
        } else if t >= 1.0 {
            1.0 - e
        } else if t < e {
            e * eased_area(t / e)
        } else if t > 1.0 - e {
            (1.0 - e) - e * eased_area((1.0 - t) / e)
        } else {
            0.5 * e + (t - e)
        }
    }

    /// `I(1)`: the window's whole area.
    pub fn area(&self) -> f64 {
        1.0 - self.ease
    }

    /// The ramp at `t`: how much of the whole deformation has built up there.
    #[inline]
    pub fn ramp_t(&self, t: f64) -> f64 {
        self.integral_t(t) / self.area()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_and_its_integral_agree_and_the_ramp_runs_from_zero_to_one() {
        for softness in [0.0, 0.3, 0.6, 1.0] {
            let p = Profile::new(2.0, 6.0, softness);
            assert_eq!(p.ramp_t(-0.5), 0.0);
            assert_eq!(p.ramp_t(1.5), 1.0);
            assert!((p.ramp_t(1.0) - 1.0).abs() < 1e-15);
            // The integral against a fine sum of the window.
            let n = 200_000;
            let mut sum = 0.0;
            for k in 0..n {
                let t = (k as f64 + 0.5) / n as f64;
                sum += p.window_t(t) / n as f64;
                if k % 20_000 == 0 {
                    assert!((p.integral_t((k + 1) as f64 / n as f64) - sum).abs() < 1e-6, "{softness} {k}");
                }
            }
            assert!((sum - p.area()).abs() < 1e-6);
            // Never falls back, and is level at both ends when it eases.
            let mut last = 0.0;
            for k in 0..=1000 {
                let f = p.ramp_t(k as f64 / 1000.0);
                assert!(f >= last);
                last = f;
            }
            if softness > 0.0 {
                assert!(p.window_t(1e-9) < 1e-6 && p.window_t(1.0 - 1e-9) < 1e-6);
            }
        }
        // With no softness the ramp is Barr's linear one.
        let hard = Profile::new(0.0, 1.0, 0.0);
        for t in [0.0, 0.1, 0.25, 0.5, 0.9, 1.0] {
            assert_eq!(hard.ramp_t(t), t);
        }
    }
}
