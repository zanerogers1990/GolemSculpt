//! The random part of a dab: how far Scatter throws it off the stroke's
//! path, how much smaller Size Jitter makes it, and how far Random Rotation
//! turns its alpha.
//!
//! Nothing here keeps a state that moves on from one draw to the next. A
//! dab's numbers are worked out from the stroke's seed and the dab's place in
//! the stroke (its first dab, its second...), so the same stroke made again
//! gives the same numbers, and a dab that is made more than once (a print
//! being dragged out is remade every frame) gets the same ones each time.
//!
//! The seed is where the stroke lands on the surface ([`seed`]). A stroke
//! replayed onto the same surface therefore comes out the same, on any
//! machine and at any frame rate, while two strokes side by side do not look
//! alike, and neither do two prints stamped on one spot: the first has moved
//! the surface the second lands on.
//!
//! The numbers are drawn once for a dab, before it is copied under symmetry,
//! and the copies are made from the dab they belong to. So mirror images stay
//! mirror images however random the stroke is.

use glam::{Vec2, Vec3};

/// One dab's draw.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Jitter {
    /// A point of the unit disc: which way along the surface Scatter throws
    /// the dab, and how much of the furthest it may go.
    pub scatter: Vec2,
    /// 0 to 1: how much of Size Jitter this dab takes.
    pub shrink: f32,
    /// -1 to 1: how much of Random Rotation, and which way round.
    pub turn: f32,
}

/// A stroke's seed: the point of the surface it landed on, to the bit.
pub(super) fn seed(landed: Vec3) -> u32 {
    let [x, y, z] = landed.to_array().map(f32::to_bits);
    mix(x ^ mix(y ^ mix(z)))
}

/// Scrambles 32 bits so that inputs one apart come out unrelated (the
/// "lowbias32" integer hash).
fn mix(mut x: u32) -> u32 {
    x ^= x >> 16;
    x = x.wrapping_mul(0x7feb_352d);
    x ^= x >> 15;
    x = x.wrapping_mul(0x846c_a68b);
    x ^ (x >> 16)
}

impl Jitter {
    /// The draw for dab number `dab` of the stroke with this `seed`.
    pub fn of(seed: u32, dab: u32) -> Jitter {
        // Four numbers from 0 up to (not including) 1, 24 bits each: every
        // one of them a float exactly.
        let unit = |channel: u32| {
            let bits = mix(seed ^ mix(dab.wrapping_mul(4).wrapping_add(channel)));
            (bits >> 8) as f32 / (1u32 << 24) as f32
        };
        // Evenly over the disc: by area, not bunched at the middle.
        let (angle, reach) = (unit(0) * std::f32::consts::TAU, unit(1).sqrt());
        Jitter { scatter: Vec2::from_angle(angle) * reach, shrink: unit(2), turn: unit(3) * 2.0 - 1.0 }
    }
}
