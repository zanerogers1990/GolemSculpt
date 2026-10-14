//! The brushes that ship with the app besides the fourteen base ones: the
//! handful a sculptor reaches for by name in ZBrush or Nomad, each a record
//! over one of the kernels.
//!
//! Every number here was set by drawing the stroke through the app and
//! looking at the picture, several variants side by side, on a ball
//! remeshed fine enough to carry the detail. A brush is only here if its
//! stroke read as what its name promises. What the looking found is beside
//! the numbers it led to.

use super::Record;
use crate::sculpt::alpha::{self, AlphaSettings};
use crate::sculpt::{Brush, FalloffCurve, FalloffPreset, FlattenMode, SmoothMode};

/// One shipped brush: its name, its kernel, what it is for (the panel's
/// tip), and what it changes of its kernel's base brush.
struct Shipped {
    name: &'static str,
    kernel: Brush,
    about: &'static str,
    tune: fn(&mut Record),
}

/// The picture called `name`, laid as `how` says on top of the plain way.
fn alpha(name: &str, how: AlphaSettings) -> AlphaSettings {
    AlphaSettings { image: alpha::find(name), ..how }
}

const PLAIN: AlphaSettings = AlphaSettings {
    image: None,
    rotation: 0.0,
    scale: 1.0,
    invert: false,
    tile: false,
    follow: false,
    drag: false,
    random_rotation: 0.0,
};

const SHIPPED: [Shipped; 16] = [
    Shipped {
        name: "Clay Strips",
        kernel: Brush::Clay,
        about: "Lays flat-topped, square-ended strips that overlap: the classic blocking-in brush.",
        tune: |r| {
            r.alpha = alpha("Square", AlphaSettings { follow: true, ..PLAIN });
            // Accumulate off: a strip comes up to one thickness and stays
            // flat on top, and the next stroke lays its strip over it. With
            // it on, or any thicker, the strip's walls stand taller than the
            // mesh can draw and its edges tear.
            r.settings.accumulate = false;
            r.settings.strength = 0.5;
            r.settings.focal_shift = -0.5;
            r.settings.spacing = 0.3;
            (r.numbers.clay_depth, r.numbers.clay_softness, r.numbers.clay_relax) = (0.2, 0.1, 0.1);
        },
    },
    Shipped {
        name: "Clay Tubes",
        kernel: Brush::Clay,
        about: "Lays a rounded bead of clay, like a rolled coil pressed on.",
        tune: |r| {
            // The domed tip at a fixed height is the bead. Accumulate on
            // piles it into a fin with the dabs showing down its sides, and
            // so do dabs any further apart than this.
            r.settings.accumulate = false;
            r.settings.strength = 0.6;
            r.settings.spacing = 0.12;
            (r.numbers.clay_depth, r.numbers.clay_softness) = (0.7, 1.0);
        },
    },
    Shipped {
        name: "Dam Standard",
        kernel: Brush::Crease,
        about: "Cuts a clean, sharp line that thins to nothing as the pen lifts.",
        tune: |r| {
            r.settings.strength = 0.7;
            // The pen's weight is the width of the cut: a light touch at
            // either end tapers it to a point.
            r.settings.pressure_size = 0.85;
            r.settings.pressure_strength = 0.6;
            // The Sharp curve is what keeps the walls of the cut clean; with
            // Crease's own falloff at this depth they came out ragged.
            r.falloff = Some(FalloffCurve::preset(FalloffPreset::Sharp));
            (r.numbers.crease_pinch, r.numbers.crease_depth) = (0.25, 0.7);
        },
    },
    Shipped {
        name: "Trim",
        kernel: Brush::Flatten,
        about: "Shaves off what stands above the surface, with a hard edge, and leaves hollows alone.",
        tune: |r| {
            r.settings.flatten_mode = FlattenMode::Scrape;
            r.settings.strength = 0.6;
            r.settings.focal_shift = -0.7;
        },
    },
    Shipped {
        name: "Fill",
        kernel: Brush::Flatten,
        about: "Fills hollows up to the surface around them and leaves what stands proud alone.",
        tune: |r| {
            r.settings.flatten_mode = FlattenMode::Fill;
            r.settings.strength = 0.6;
        },
    },
    Shipped {
        name: "Planar",
        kernel: Brush::Flatten,
        about: "Levels the whole stroke to the plane it started on: one flat facet.",
        tune: |r| {
            r.settings.lock_plane = true;
            r.settings.strength = 0.8;
            r.settings.focal_shift = -0.6;
        },
    },
    Shipped {
        name: "Polish",
        kernel: Brush::Flatten,
        about: "A gentle flatten for evening out a surface without losing its shape.",
        tune: |r| {
            // Half of Trim's strength and a soft edge: a few passes even a
            // surface out, and no pass leaves a mark of its own.
            r.settings.strength = 0.25;
            r.settings.focal_shift = 0.3;
        },
    },
    Shipped {
        name: "Smooth Peaks",
        kernel: Brush::Smooth,
        about: "Smooths only what sticks out; hollows and cuts keep their depth.",
        tune: |r| {
            r.settings.smooth_mode = SmoothMode::Peaks;
            r.settings.strength = 0.3;
        },
    },
    Shipped {
        name: "Smooth Valleys",
        kernel: Brush::Smooth,
        about: "Smooths only the hollows; what sticks out keeps its height.",
        tune: |r| {
            r.settings.smooth_mode = SmoothMode::Valleys;
            r.settings.strength = 0.3;
        },
    },
    Shipped {
        name: "Relax",
        kernel: Brush::Smooth,
        about: "Evens out the triangles without changing the shape.",
        tune: |r| {
            r.settings.smooth_mode = SmoothMode::Relax;
            r.settings.strength = 0.5;
        },
    },
    Shipped {
        // Not "Smooth Keep Volume": that is too long for its row in the
        // list at any size of letter worth reading, and was cut short. The
        // row's icon says it is a smoothing brush.
        name: "Keep Volume",
        kernel: Brush::Smooth,
        about: "Smooths without thinning: fingers and horns keep their thickness.",
        tune: |r| {
            r.settings.keep_volume = 1.0;
            r.settings.strength = 0.3;
        },
    },
    Shipped {
        name: "Rake",
        kernel: Brush::Standard,
        about: "Scratches fine parallel lines that follow the stroke, like a rake tool across clay.",
        tune: |r| {
            r.alpha = alpha("Rake", AlphaSettings { follow: true, ..PLAIN });
            // Accumulate off: every stroke scratches to the same light
            // depth, and strokes crossed over each other build up stroke by
            // stroke. With it on, where two strokes met they ploughed.
            r.settings.accumulate = false;
            r.settings.strength = 0.06;
            r.settings.focal_shift = -0.5;
            // Close dabs: the picture turns a little with each on a curve,
            // and further apart every turn showed as a tick on the lines.
            r.settings.spacing = 0.06;
        },
    },
    Shipped {
        name: "Surface Noise",
        kernel: Brush::Standard,
        about: "Roughens the surface with scattered, turned prints of a noise picture.",
        tune: |r| {
            // The picture bigger than the ring, so no print shows its round
            // edge, and each turned any way, so no two look alike.
            r.alpha = alpha("Noise", AlphaSettings { random_rotation: 180.0, scale: 1.6, ..PLAIN });
            // Light and close: at twice the spacing the prints stood apart
            // as clumps with bare surface between them.
            r.settings.strength = 0.045;
            r.settings.spacing = 0.3;
            r.settings.scatter = 0.8;
            r.settings.size_jitter = 0.3;
        },
    },
    Shipped {
        name: "Scales",
        kernel: Brush::Layer,
        about: "Press and drag out one print of scales, sized and turned by the drag.",
        tune: |r| {
            r.alpha = alpha("Scales", AlphaSettings { drag: true, ..PLAIN });
            r.settings.strength = 0.5;
            r.settings.focal_shift = -0.6;
        },
    },
    Shipped {
        name: "Spread",
        kernel: Brush::Pinch,
        about: "Pinch turned round: opens a crease up and pushes the surface apart from the stroke.",
        tune: |r| r.settings.positive = false,
    },
    Shipped {
        name: "Airbrush",
        kernel: Brush::Paint,
        about: "Paints in soft, light mists that build up the longer you go over a place: shading and blushes.",
        tune: |r| {
            // A tenth of the Opacity a dab, soft right out to the rim, so
            // one pass is a mist and a third is a coat with no edge to it.
            r.settings.strength = 0.1;
            r.settings.focal_shift = 0.6;
            r.settings.spacing = 0.1;
            r.settings.pressure_strength = 1.0;
        },
    },
];

/// Every shipped brush, in the order the panel lists them.
pub(super) fn all() -> Vec<(&'static str, Brush, Record)> {
    SHIPPED
        .iter()
        .map(|brush| {
            let mut record = Record::base(brush.kernel);
            (brush.tune)(&mut record);
            (brush.name, brush.kernel, record.in_range())
        })
        .collect()
}

/// What the shipped brush called `name` is for.
pub fn about(name: &str) -> Option<&'static str> {
    SHIPPED.iter().find(|brush| brush.name == name).map(|brush| brush.about)
}
