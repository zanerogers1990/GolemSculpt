//! The ways a kernel can be set to work: what Smooth evens out, and which
//! side of its plane Flatten levels.
//!
//! These are options on a kernel and not brushes of their own: a brush in
//! the panel is a kernel with its settings (`BrushSettings`), so "a Smooth
//! that only knocks the bumps off" or "a Flatten that only scrapes" is the
//! same kernel set another way. Each is kept in the settings file by name.

use serde::{Deserialize, Serialize};

/// What a Smooth dab does to the surface under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmoothMode {
    /// Every vertex moves toward the middle of its neighbours: bumps go
    /// down, pits come up, and the triangles even out.
    #[default]
    All,
    /// Every vertex moves toward the middle of its neighbours and is held to
    /// where the stroke found it (the HC smoothing Blender's Surface Smooth
    /// is): fine roughness goes, but a form keeps its size where plain
    /// smoothing would thin it, and so does a lump the size of the brush.
    Surface,
    /// Only what stands above its neighbours is lowered: bumps are knocked
    /// off and the creases beside them are left as deep as they were.
    Peaks,
    /// Only what lies below its neighbours is raised: pits and creases fill
    /// and the peaks are left standing.
    Valleys,
    /// Vertices slide within the surface until the triangles are even; the
    /// shape stays as it is. For cleaning up what Move or Snake Hook has
    /// stretched.
    Relax,
}

impl SmoothMode {
    pub const ALL: [SmoothMode; 5] =
        [SmoothMode::All, SmoothMode::Surface, SmoothMode::Peaks, SmoothMode::Valleys, SmoothMode::Relax];

    pub fn name(self) -> &'static str {
        match self {
            SmoothMode::All => "All",
            SmoothMode::Surface => "Surface",
            SmoothMode::Peaks => "Peaks",
            SmoothMode::Valleys => "Valleys",
            SmoothMode::Relax => "Relax",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            SmoothMode::All => "Smooths everything under the ring: bumps go down and pits come up.",
            SmoothMode::Surface => {
                "Takes fine roughness off and keeps every form its size: fingers, horns and thin edges do not get \
                 thinner. It leaves lumps as big as the brush mostly alone; All takes those off."
            }
            SmoothMode::Peaks => "Only lowers what stands up: knocks bumps off and leaves the creases beside them as deep as they were.",
            SmoothMode::Valleys => "Only fills what lies low: pits and creases come up and the peaks are left standing.",
            SmoothMode::Relax => "Slides the mesh within the surface to even out stretched triangles, without changing the shape. For after Move or Snake Hook.",
        }
    }

    /// The mode called `name`, whatever the case of its letters.
    pub fn named(name: &str) -> Option<SmoothMode> {
        Self::ALL.into_iter().find(|mode| mode.name().eq_ignore_ascii_case(name.trim()))
    }
}

/// Which side of its plane a Flatten dab works on.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FlattenMode {
    /// What is above the plane comes down to it and what is below comes up.
    #[default]
    Both,
    /// Only what stands above the plane is brought down (ZBrush's Trim
    /// Dynamic, Nomad's Trim).
    Scrape,
    /// Only what lies below the plane is brought up.
    Fill,
}

impl FlattenMode {
    pub const ALL: [FlattenMode; 3] = [FlattenMode::Both, FlattenMode::Scrape, FlattenMode::Fill];

    pub fn name(self) -> &'static str {
        match self {
            FlattenMode::Both => "Both",
            FlattenMode::Scrape => "Scrape",
            FlattenMode::Fill => "Fill",
        }
    }

    pub fn tip(self) -> &'static str {
        match self {
            FlattenMode::Both => "Levels the surface to the plane under the ring: high parts come down and low parts come up. Ctrl pushes them apart instead.",
            FlattenMode::Scrape => "Only brings down what stands above the plane, like scraping with a flat tool. Ctrl fills instead.",
            FlattenMode::Fill => "Only raises what lies below the plane, filling hollows up to it. Ctrl scrapes instead.",
        }
    }

    /// The mode called `name`, whatever the case of its letters.
    pub fn named(name: &str) -> Option<FlattenMode> {
        Self::ALL.into_iter().find(|mode| mode.name().eq_ignore_ascii_case(name.trim()))
    }

    /// The mode a stroke works in: its own, or with the brush inverted
    /// (Ctrl, the right button, Sub) the other one-sided mode. Both has its
    /// own inverse, which is not a mode: see `kernels::flatten`.
    pub fn inverted(self, inverted: bool) -> FlattenMode {
        match (self, inverted) {
            (FlattenMode::Scrape, true) => FlattenMode::Fill,
            (FlattenMode::Fill, true) => FlattenMode::Scrape,
            (mode, _) => mode,
        }
    }
}
