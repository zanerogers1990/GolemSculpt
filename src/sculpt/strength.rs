//! What the Strength slider means to each kernel.
//!
//! Until 2026-10-21 the slider went straight through: a kernel worked at
//! the number it showed. For the brushes that build up or wear down a
//! little with every dab that put all the useful range in the bottom tenth
//! of the slider: Standard and Inflate built a quarter of the brush's
//! radius in one pass at 10% and two and a half radii at 100%, so the rest
//! of the slider was one blob or another.
//!
//! Now those kernels take the slider to the power 1.5 times a gain of
//! their own, as Blender squares its own (it says, to make the low values
//! more sensitive). 1.5 rather than 2 because it is what puts one pass of
//! Standard, Inflate and Layer, at full pressure, where the plan for the
//! brushes asked: about a hundredth of the radius at 10%, a tenth at 50%
//! and a third at 100% (measured with `--brush-feel`). The kernels still
//! work in the old units: [`effective`] is what they are handed, so a
//! number they were tuned with means what it did.
//!
//! The Clay and Crease kernels are not mapped: Clay Strips and Dam Standard
//! are made of them and stay exactly as they are. Nor are Move, Pose, Snake
//! Hook, Erase Layer, Paint and mask painting, whose Strength is a share of
//! something (the pen's travel, a layer, the paint's opacity) and already
//! spreads across its slider. A dragged print of an alpha is not either: its
//! Strength is its height.

use super::Brush;

/// The power the slider is raised to for the kernels it is mapped for.
pub const POWER: f32 = 1.5;

/// The gain a kernel's mapped Strength is multiplied by, in the old units
/// (what the slider meant before), or `None` for a kernel whose Strength
/// goes straight through.
///
/// Standard and Inflate: 100% is what about 13% and 10% were (a third of a
/// radius in one pass). Layer: its own build (`kernels::layer`) at 100%
/// reaches nearly its full height in one pass. Smooth, Flatten, Pinch and
/// Nudge already ran across their whole slider; a gain of 1 keeps 100% as
/// it was and gives the low end finer steps.
pub fn gain(brush: Brush) -> Option<f32> {
    match brush {
        Brush::Standard => Some(0.13),
        Brush::Inflate => Some(0.1),
        Brush::Layer => Some(0.5),
        Brush::Smooth | Brush::Flatten | Brush::Pinch | Brush::Nudge => Some(1.0),
        _ => None,
    }
}

/// What a kernel works at for the slider at `ui`: in the old units.
#[inline]
pub fn effective(brush: Brush, ui: f32) -> f32 {
    match gain(brush) {
        Some(gain) => gain * ui.max(0.0).powf(POWER),
        None => ui,
    }
}

/// The slider value that does what `old` did before the slider was mapped
/// (a brush saved by an earlier version), as near as the slider can: past
/// what 100% now does it is 100%.
///
/// Layer's build changed with the mapping (it now eases up to its height,
/// `kernels::layer`), so an old Layer number is taken by what it did: 10%
/// and over reached the full height in one pass, which 100% does now.
pub fn from_old(brush: Brush, old: f32) -> f32 {
    let old = old.max(0.0);
    let new = match brush {
        Brush::Layer => (old / OLD_LAYER_FULL).powf(1.0 / POWER),
        _ => match gain(brush) {
            Some(gain) => (old / gain).powf(1.0 / POWER),
            None => return old,
        },
    };
    new.clamp(0.01, 1.0)
}

/// The old Layer Strength that reached the layer's full height in one pass.
const OLD_LAYER_FULL: f32 = 0.1;

/// The slider value at which Standard without Accumulate builds one pass
/// about as tall as Strength `old` (in the old units, what the slider meant
/// before 2026-10-21) built without it then: a brush saved by an earlier
/// version with Accumulate off.
///
/// Then each dab closed four tenths of its Strength of the way to a third of
/// a radius, so one pass came to `OLD_SEEK_HEIGHT * (1 - e^(-OLD_SEEK_RATE *
/// old))` of a radius under its middle (measured with `--brush-feel`). Now a
/// pass comes to [`SEEK_PASS`] of a radius times the slider to the power
/// [`POWER`] (`kernels::raised`).
pub fn from_old_seeking(old: f32) -> f32 {
    let built = OLD_SEEK_HEIGHT * (1.0 - (-OLD_SEEK_RATE * old.max(0.0)).exp());
    (built / SEEK_PASS).powf(1.0 / POWER).clamp(0.01, 1.0)
}

/// What one pass of Standard without Accumulate built at old Strength `s`,
/// in radii, was about `OLD_SEEK_HEIGHT * (1 - e^(-OLD_SEEK_RATE * s))`.
const OLD_SEEK_HEIGHT: f32 = 0.3;
const OLD_SEEK_RATE: f32 = 2.0;

/// What one pass of Standard without Accumulate builds at 100% now, in
/// radii (`--brush-feel`).
const SEEK_PASS: f32 = 0.29;

#[cfg(test)]
mod tests {
    use super::*;

    /// The kernels that are not mapped get the slider to the bit; the
    /// mapped ones grow with it, and an old number brought over does what
    /// it did wherever the slider reaches.
    #[test]
    fn the_slider_maps_and_old_numbers_come_over() {
        for brush in Brush::ALL {
            for i in 0..=100 {
                let ui = i as f32 / 100.0;
                if gain(brush).is_none() {
                    assert_eq!(effective(brush, ui).to_bits(), ui.to_bits());
                    assert_eq!(from_old(brush, ui).to_bits(), ui.to_bits());
                } else if i > 0 {
                    assert!(effective(brush, ui) > effective(brush, ui - 0.01));
                }
            }
        }
        for brush in [Brush::Standard, Brush::Inflate, Brush::Smooth, Brush::Flatten, Brush::Pinch, Brush::Nudge] {
            for old in [0.02f32, 0.05, 0.1] {
                let new = from_old(brush, old);
                if new < 1.0 && new > 0.01 {
                    assert!((effective(brush, new) - old).abs() < 1e-5, "{} {old}", brush.name());
                }
            }
            assert_eq!(from_old(brush, 1.0).min(1.0), from_old(brush, 1.0));
        }
        // A gain of 1: the old 100% is the new, and the old 30% is near the middle.
        assert_eq!(from_old(Brush::Smooth, 1.0), 1.0);
        assert!((from_old(Brush::Smooth, 0.3) - 0.448).abs() < 1e-3);
        // Zane's Standard at 40% built a radius a pass: past what 100% now does.
        assert_eq!(from_old(Brush::Standard, 0.4), 1.0);
        assert_eq!(from_old(Brush::Layer, 0.8), 1.0);
        assert!(from_old(Brush::Layer, 0.05) < 0.7);
    }
}
