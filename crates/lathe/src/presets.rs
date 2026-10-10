//! Starting shapes for the lathe. Each is a unit-height profile (radii as fractions of the height),
//! scaled to the scene when applied.
//!
//! Every closed preset traces the whole cross-section from axis to axis, the way a turner would
//! draw it, so the result is a solid with real wall thickness rather than a surface: the bowl and
//! cup are hollow because their profile goes up the outside, over the rim and back down the inside,
//! not because the surface is open.

use glam::Vec2;

use crate::profile::{LatheProfile, ProfilePoint};

/// (radius, height) as fractions of the height, and whether the point is a corner.
type Entry = (f32, f32, bool);

const fn p(radius: f32, height: f32) -> Entry {
    (radius, height, false)
}

const fn c(radius: f32, height: f32) -> Entry {
    (radius, height, true)
}

/// Flat foot, full belly, narrow neck, flared lip - the open top is capped unless `cap_ends` is
/// turned off.
const VASE: [Entry; 7] =
    [c(0.0, 0.0), c(0.20, 0.0), p(0.30, 0.18), p(0.32, 0.38), p(0.20, 0.66), p(0.13, 0.84), p(0.19, 1.0)];

/// Foot ring, outer wall, rounded rim, inner wall back down to the axis.
const BOWL: [Entry; 9] = [
    c(0.0, 0.0),
    c(0.22, 0.0),
    p(0.24, 0.08),
    p(0.48, 0.34),
    p(0.66, 0.64),
    p(0.70, 0.72),
    p(0.64, 0.70),
    p(0.42, 0.36),
    p(0.0, 0.20),
];

const CUP: [Entry; 8] =
    [c(0.0, 0.0), c(0.30, 0.0), p(0.34, 0.50), p(0.38, 1.0), p(0.33, 1.0), p(0.29, 0.50), c(0.25, 0.10), p(0.0, 0.10)];

const BOTTLE: [Entry; 9] = [
    c(0.0, 0.0),
    c(0.24, 0.0),
    p(0.26, 0.08),
    p(0.26, 0.52),
    p(0.20, 0.66),
    p(0.09, 0.78),
    p(0.08, 0.94),
    c(0.10, 1.0),
    p(0.0, 1.0),
];

const EGG: [Entry; 5] = [p(0.0, 0.0), p(0.30, 0.14), p(0.36, 0.42), p(0.26, 0.80), p(0.0, 1.0)];

/// A rounded, slightly squared section well off the axis - a torus.
const RING: [Entry; 8] = [
    p(0.34, 0.40),
    p(0.44, 0.36),
    p(0.54, 0.40),
    p(0.58, 0.50),
    p(0.54, 0.60),
    p(0.44, 0.64),
    p(0.34, 0.60),
    p(0.30, 0.50),
];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LathePreset {
    Vase,
    Bowl,
    Cup,
    Bottle,
    Egg,
    Ring,
}

impl LathePreset {
    pub const ALL: [LathePreset; 6] = [
        LathePreset::Vase,
        LathePreset::Bowl,
        LathePreset::Cup,
        LathePreset::Bottle,
        LathePreset::Egg,
        LathePreset::Ring,
    ];

    pub fn name(self) -> &'static str {
        match self {
            LathePreset::Vase => "Vase",
            LathePreset::Bowl => "Bowl",
            LathePreset::Cup => "Cup",
            LathePreset::Bottle => "Bottle",
            LathePreset::Egg => "Egg",
            LathePreset::Ring => "Ring",
        }
    }

    /// Profile points for the preset at `height`, plus whether it is a closed loop.
    pub fn points(self, height: f32) -> (Vec<ProfilePoint>, bool) {
        let (table, closed_loop): (&[Entry], bool) = match self {
            LathePreset::Vase => (&VASE, false),
            LathePreset::Bowl => (&BOWL, false),
            LathePreset::Cup => (&CUP, false),
            LathePreset::Bottle => (&BOTTLE, false),
            LathePreset::Egg => (&EGG, false),
            LathePreset::Ring => (&RING, true),
        };
        let points = table
            .iter()
            .map(|&(r, h, sharp)| ProfilePoint { position: Vec2::new(r * height, h * height), sharp })
            .collect();
        (points, closed_loop)
    }

    /// The preset as a profile, with the axis rules applied.
    pub fn profile(self, height: f32) -> LatheProfile {
        let (points, closed_loop) = self.points(height);
        LatheProfile::from_points(&points, closed_loop)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_scale_every_preset_and_only_the_ring_is_a_loop() {
        for preset in LathePreset::ALL {
            let (unit, loop_unit) = preset.points(1.0);
            let (tall, loop_tall) = preset.points(3.0);
            assert_eq!(unit.len(), tall.len());
            assert_eq!(loop_unit, preset == LathePreset::Ring);
            assert_eq!(loop_unit, loop_tall);
            for (a, b) in unit.iter().zip(&tall) {
                assert!((a.position * 3.0 - b.position).length() < 1e-6 && a.sharp == b.sharp);
            }
        }
    }

    #[test]
    fn closed_presets_run_from_axis_to_axis() {
        for preset in [LathePreset::Vase, LathePreset::Bowl, LathePreset::Cup, LathePreset::Egg] {
            let (points, _) = preset.points(1.0);
            assert_eq!(points[0].position.x, 0.0, "{}", preset.name());
        }
        for preset in [LathePreset::Bowl, LathePreset::Cup, LathePreset::Bottle, LathePreset::Egg] {
            let (points, _) = preset.points(1.0);
            assert_eq!(points[points.len() - 1].position.x, 0.0, "{}", preset.name());
        }
    }
}
