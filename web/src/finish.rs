//! Material finishes: Clay, Metal and Wood, the desktop's own (its
//! `finish_clay`, `finish_metal` and `finish_wood` in sculpt_pbr.wgsl, and
//! their presets and settings in src/look.rs), lit by a lighter rig than the
//! desktop's so they run in any browser: three lights that turn with the
//! camera (the desktop's Studio preset, which also follows the camera by
//! default), its sky, equator and ground light all round, and a reflected
//! sky worked out in the shader instead of an HDRI, and no softboxes. The
//! key light's soft shadow and ambient occlusion come with WebGPU only
//! (shadows.rs); the mesh's own curvature puts the wash in the recesses and
//! the wear on the ridges, as on the desktop.
//!
//! The preset tables below are copied from src/look.rs (it cannot be
//! compiled here: it reaches into the desktop's files, scene and renderer),
//! so a preset changed there should be changed here too.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use super::lighting::{Effects, Lighting, light_angle, rig};

pub type Rgb = [f32; 3];

fn lin1(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

/// An sRGB colour as the shader works with it: linear.
pub fn linear(c: Rgb) -> Rgb {
    [lin1(c[0]), lin1(c[1]), lin1(c[2])]
}

fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    let c = linear(c);
    [c[0], c[1], c[2], a]
}

const fn idx(i: usize, len: usize) -> usize {
    if i < len { i } else { len - 1 }
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

/// What the model is shaded as.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Finish {
    /// The matcap: a photo of a lit sphere, no lights.
    #[default]
    Matcap,
    Clay,
    Metal,
    Wood,
    /// One colour, smooth or rough, metal or not: the lights alone.
    Plain,
}

impl Finish {
    pub const ALL: [Finish; 5] = [Finish::Matcap, Finish::Plain, Finish::Clay, Finish::Metal, Finish::Wood];

    pub fn name(self) -> &'static str {
        match self {
            Finish::Matcap => "Matcap",
            Finish::Clay => "Clay",
            Finish::Metal => "Metal",
            Finish::Wood => "Wood",
            Finish::Plain => "Plain",
        }
    }

    /// The finish's number in the shader (`FINISH_*` in finish.wgsl).
    fn code(self) -> f32 {
        match self {
            Finish::Matcap => 0.0,
            Finish::Clay => 1.0,
            Finish::Metal => 2.0,
            Finish::Wood => 3.0,
            Finish::Plain => 4.0,
        }
    }

    /// Whether it reads the mesh's curvature (the recess wash, the wear).
    pub fn needs_curvature(self) -> bool {
        !matches!(self, Finish::Matcap | Finish::Plain)
    }
}

// ---- the presets (copied from src/look.rs) ---------------------------------------

/// One aged-metal finish: a bare metal, a coat over it that gathers in
/// recesses and wears off raised edges, and a dark wash in the deepest ones.
pub struct MetalPreset {
    #[allow(dead_code)] // the desktop names presets in files by it
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub metal: Rgb,
    pub metal_smoothness: f32,
    pub metal_metallic: f32,
    pub coat_a: Rgb,
    pub coat_b: Rgb,
    pub coat_metallic: f32,
    pub coat_smoothness: f32,
    pub wash_color: Rgb,
    pub wash: f32,
    pub coverage: f32,
    pub edge_wear: f32,
    pub patch_size: f32,
    pub patch_contrast: f32,
    pub patchiness: f32,
    pub follows_recesses: f32,
    pub grain: f32,
    pub grain_size: f32,
    pub patch_breakup: f32,
    pub sparkle: f32,
    pub undulation: f32,
    pub undulation_size: f32,
}

pub const METALS: [MetalPreset; 7] = [
    MetalPreset {
        id: "rust",
        name: "Rust",
        description: "Ochre-orange rust over dark iron, worn back to grey metal on the high points.",
        metal: [0.40, 0.41, 0.44],
        metal_smoothness: 0.42,
        metal_metallic: 0.8,
        coat_a: [0.70, 0.44, 0.14],
        coat_b: [0.38, 0.18, 0.06],
        coat_metallic: 0.0,
        coat_smoothness: 0.15,
        wash_color: [0.10, 0.055, 0.03],
        wash: 0.65,
        coverage: 0.62,
        edge_wear: 0.9,
        patch_size: 0.09,
        patch_contrast: 4.0,
        patchiness: 1.0,
        follows_recesses: 0.5,
        grain: 0.55,
        grain_size: 0.004,
        patch_breakup: 0.0,
        sparkle: 0.0,
        undulation: 0.0,
        undulation_size: 0.2,
    },
    MetalPreset {
        id: "patina",
        name: "Patina",
        description: "Chalky blue-green verdigris over bronze, dark in the grooves, copper on the edges.",
        metal: [0.70, 0.45, 0.25],
        metal_smoothness: 0.55,
        metal_metallic: 0.85,
        coat_a: [0.36, 0.64, 0.58],
        coat_b: [0.17, 0.38, 0.36],
        coat_metallic: 0.0,
        coat_smoothness: 0.12,
        wash_color: [0.11, 0.07, 0.045],
        wash: 0.8,
        coverage: 0.8,
        edge_wear: 0.6,
        patch_size: 0.07,
        patch_contrast: 3.0,
        patchiness: 0.8,
        follows_recesses: 0.6,
        grain: 0.35,
        grain_size: 0.005,
        patch_breakup: 0.0,
        sparkle: 0.0,
        undulation: 0.0,
        undulation_size: 0.2,
    },
    MetalPreset {
        id: "red_wash",
        name: "Red Wash",
        description: "Deep oxblood red with a black wash in the recesses and a coppery sheen on the highs.",
        metal: [0.74, 0.30, 0.20],
        metal_smoothness: 0.6,
        metal_metallic: 0.6,
        coat_a: [0.40, 0.07, 0.045],
        coat_b: [0.26, 0.045, 0.03],
        coat_metallic: 0.35,
        coat_smoothness: 0.55,
        wash_color: [0.03, 0.015, 0.012],
        wash: 0.9,
        coverage: 0.9,
        edge_wear: 0.55,
        patch_size: 0.1,
        patch_contrast: 2.0,
        patchiness: 0.5,
        follows_recesses: 0.8,
        grain: 0.1,
        grain_size: 0.004,
        patch_breakup: 0.0,
        sparkle: 0.0,
        undulation: 0.0,
        undulation_size: 0.2,
    },
    MetalPreset {
        id: "antique_silver",
        name: "Antique Silver",
        description: "Blackened base with bright silver dry-brushed onto every raised detail.",
        metal: [0.86, 0.86, 0.88],
        metal_smoothness: 0.55,
        metal_metallic: 0.8,
        coat_a: [0.06, 0.06, 0.065],
        coat_b: [0.035, 0.035, 0.04],
        coat_metallic: 0.6,
        coat_smoothness: 0.45,
        wash_color: [0.015, 0.015, 0.018],
        wash: 0.9,
        coverage: 0.5,
        edge_wear: 1.2,
        patch_size: 0.08,
        patch_contrast: 2.0,
        patchiness: 0.25,
        follows_recesses: 2.0,
        grain: 0.05,
        grain_size: 0.004,
        patch_breakup: 0.0,
        sparkle: 0.0,
        undulation: 0.0,
        undulation_size: 0.2,
    },
    MetalPreset {
        id: "antique_gold",
        name: "Antique Gold",
        description: "Polished brass-gold with black worked into the recesses.",
        metal: [0.86, 0.65, 0.30],
        metal_smoothness: 0.6,
        metal_metallic: 0.85,
        coat_a: [0.08, 0.06, 0.035],
        coat_b: [0.04, 0.03, 0.02],
        coat_metallic: 0.5,
        coat_smoothness: 0.5,
        wash_color: [0.02, 0.015, 0.01],
        wash: 0.9,
        coverage: 0.25,
        edge_wear: 1.0,
        patch_size: 0.08,
        patch_contrast: 2.0,
        patchiness: 0.2,
        follows_recesses: 2.0,
        grain: 0.05,
        grain_size: 0.004,
        patch_breakup: 0.0,
        sparkle: 0.0,
        undulation: 0.0,
        undulation_size: 0.2,
    },
    MetalPreset {
        id: "worn_steel",
        name: "Worn Steel",
        description: "Dark gunmetal steel, glossy but hand-worn: dull etched blotches, fine sparkle and a hammered ripple in the reflections.",
        metal: [0.39, 0.385, 0.39],
        metal_smoothness: 0.69,
        metal_metallic: 0.85,
        coat_a: [0.34, 0.335, 0.34],
        coat_b: [0.29, 0.285, 0.29],
        coat_metallic: 0.75,
        coat_smoothness: 0.55,
        wash_color: [0.07, 0.068, 0.07],
        wash: 0.35,
        coverage: 0.2,
        edge_wear: 0.5,
        patch_size: 0.045,
        patch_contrast: 2.0,
        patchiness: 1.0,
        follows_recesses: 0.3,
        grain: 0.3,
        grain_size: 0.0025,
        patch_breakup: 1.0,
        sparkle: 0.4,
        undulation: 0.3,
        undulation_size: 0.18,
    },
    MetalPreset {
        id: "gothic_pewter",
        name: "Gothic Pewter",
        description: "Dark cast pewter: cool blue-slate across the forms, warm polished silver on the crests, black in the deep carving.",
        metal: [0.60, 0.56, 0.50],
        metal_smoothness: 0.62,
        metal_metallic: 0.75,
        coat_a: [0.30, 0.345, 0.37],
        coat_b: [0.16, 0.185, 0.205],
        coat_metallic: 0.65,
        coat_smoothness: 0.5,
        wash_color: [0.02, 0.022, 0.026],
        wash: 0.9,
        coverage: 0.9,
        edge_wear: 1.1,
        patch_size: 0.1,
        patch_contrast: 1.5,
        patchiness: 0.15,
        follows_recesses: 1.4,
        grain: 0.15,
        grain_size: 0.002,
        patch_breakup: 0.0,
        sparkle: 0.15,
        undulation: 0.0,
        undulation_size: 0.2,
    },
];

/// One sculptor's-clay look: a satin body, a wet highlight on top, tinted
/// light bleeding past the shadow line and a richer colour in the recesses.
pub struct ClayPreset {
    #[allow(dead_code)] // the desktop names presets in files by it
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub color: Rgb,
    pub recess_color: Rgb,
    pub recess_depth: f32,
    pub scatter_color: Rgb,
    pub subsurface: f32,
    pub smoothness: f32,
    pub wetness: f32,
    pub wet_sharpness: f32,
    pub ridge_burnish: f32,
    pub mottle: f32,
    pub mottle_size: f32,
    pub grain: f32,
    pub grain_size: f32,
}

pub const CLAYS: [ClayPreset; 5] = [
    ClayPreset {
        id: "grey_plasteline",
        name: "Grey Plasteline",
        description: "Mid-grey oil clay with a wet, greasy shine that catches every tool stroke.",
        color: [0.40, 0.40, 0.39],
        recess_color: [0.13, 0.13, 0.13],
        recess_depth: 0.85,
        scatter_color: [0.55, 0.50, 0.46],
        subsurface: 0.25,
        smoothness: 0.5,
        wetness: 1.0,
        wet_sharpness: 350.0,
        ridge_burnish: 0.35,
        mottle: 0.04,
        mottle_size: 0.12,
        grain: 0.25,
        grain_size: 0.0015,
    },
    ClayPreset {
        id: "charcoal",
        name: "Charcoal Clay",
        description: "Near-black clay: dark satin body, bright wet glints on the forms, lighter where it's worked.",
        color: [0.14, 0.14, 0.145],
        recess_color: [0.035, 0.035, 0.038],
        recess_depth: 0.7,
        scatter_color: [0.25, 0.24, 0.24],
        subsurface: 0.15,
        smoothness: 0.52,
        wetness: 1.2,
        wet_sharpness: 260.0,
        ridge_burnish: 0.5,
        mottle: 0.1,
        mottle_size: 0.1,
        grain: 0.4,
        grain_size: 0.0015,
    },
    ClayPreset {
        id: "warm_grey",
        name: "Warm Grey",
        description: "Soft taupe-grey clay, low shine, deep shadows in every fold.",
        color: [0.36, 0.34, 0.31],
        recess_color: [0.09, 0.08, 0.07],
        recess_depth: 1.0,
        scatter_color: [0.60, 0.45, 0.35],
        subsurface: 0.3,
        smoothness: 0.38,
        wetness: 0.4,
        wet_sharpness: 200.0,
        ridge_burnish: 0.3,
        mottle: 0.06,
        mottle_size: 0.12,
        grain: 0.4,
        grain_size: 0.0015,
    },
    ClayPreset {
        id: "terracotta",
        name: "Terracotta",
        description: "Orange-brown modelling clay (Chavant-style) with a satin sheen and a warm glow at the shadow edge.",
        color: [0.55, 0.28, 0.16],
        recess_color: [0.25, 0.09, 0.045],
        recess_depth: 0.75,
        scatter_color: [0.85, 0.32, 0.16],
        subsurface: 0.55,
        smoothness: 0.42,
        wetness: 0.45,
        wet_sharpness: 220.0,
        ridge_burnish: 0.3,
        mottle: 0.07,
        mottle_size: 0.12,
        grain: 0.35,
        grain_size: 0.0015,
    },
    ClayPreset {
        id: "red_oil",
        name: "Red Oil Clay",
        description: "Deep red-brown oil clay, waxy and warm, with rich dark-red recesses.",
        color: [0.44, 0.20, 0.12],
        recess_color: [0.18, 0.055, 0.03],
        recess_depth: 0.8,
        scatter_color: [0.80, 0.28, 0.14],
        subsurface: 0.5,
        smoothness: 0.48,
        wetness: 0.35,
        wet_sharpness: 180.0,
        ridge_burnish: 0.35,
        mottle: 0.06,
        mottle_size: 0.12,
        grain: 0.3,
        grain_size: 0.0015,
    },
];

/// One carved-wood look: growth rings and fibre streaks around a log axis, a
/// dark patina in the recesses, worn edges and a dull wax sheen.
pub struct WoodPreset {
    #[allow(dead_code)] // the desktop names presets in files by it
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub early: Rgb,
    pub late: Rgb,
    pub worn: Rgb,
    pub recess: Rgb,
    pub scatter: Rgb,
    pub subsurface: f32,
    pub ring_size: f32,
    pub ring_contrast: f32,
    pub latewood: f32,
    pub wobble: f32,
    pub figure_size: f32,
    pub streaks: f32,
    pub streak_size: f32,
    pub pores: f32,
    pub end_grain: f32,
    pub patina: f32,
    pub edge_wear: f32,
    pub smoothness: f32,
    pub wax: f32,
    pub wax_sharpness: f32,
    pub checks: f32,
}

pub const WOODS: [WoodPreset; 6] = [
    WoodPreset {
        id: "antique_fruitwood",
        name: "Antique Fruitwood",
        description: "Old waxed carving: warm red-brown, near-black grime in the recesses, edges rubbed back to glowing orange, a dull shine and the odd seasoning crack.",
        early: [0.36, 0.16, 0.07],
        late: [0.25, 0.10, 0.042],
        worn: [0.76, 0.38, 0.14],
        recess: [0.04, 0.018, 0.008],
        scatter: [0.85, 0.35, 0.12],
        subsurface: 0.2,
        ring_size: 0.012,
        ring_contrast: 0.4,
        latewood: 0.3,
        wobble: 1.4,
        figure_size: 0.25,
        streaks: 0.35,
        streak_size: 0.0012,
        pores: 0.0,
        end_grain: 0.3,
        patina: 1.0,
        edge_wear: 1.2,
        smoothness: 0.45,
        wax: 0.25,
        wax_sharpness: 30.0,
        checks: 0.3,
    },
    WoodPreset {
        id: "walnut",
        name: "Black Walnut",
        description: "Deep chocolate walnut with strong wavy figure and a soft oiled sheen.",
        early: [0.30, 0.18, 0.10],
        late: [0.17, 0.09, 0.05],
        worn: [0.50, 0.30, 0.16],
        recess: [0.04, 0.025, 0.015],
        scatter: [0.60, 0.30, 0.15],
        subsurface: 0.15,
        ring_size: 0.018,
        ring_contrast: 0.8,
        latewood: 0.35,
        wobble: 1.2,
        figure_size: 0.3,
        streaks: 0.5,
        streak_size: 0.0015,
        pores: 0.3,
        end_grain: 0.3,
        patina: 0.8,
        edge_wear: 0.8,
        smoothness: 0.45,
        wax: 0.22,
        wax_sharpness: 35.0,
        checks: 0.0,
    },
    WoodPreset {
        id: "oak",
        name: "Golden Oak",
        description: "Honey-coloured oak: bold rings, open pores, a matte oiled finish.",
        early: [0.62, 0.42, 0.22],
        late: [0.45, 0.28, 0.13],
        worn: [0.75, 0.55, 0.32],
        recess: [0.12, 0.07, 0.035],
        scatter: [0.80, 0.50, 0.25],
        subsurface: 0.15,
        ring_size: 0.025,
        ring_contrast: 0.9,
        latewood: 0.4,
        wobble: 1.0,
        figure_size: 0.3,
        streaks: 0.5,
        streak_size: 0.002,
        pores: 1.0,
        end_grain: 0.35,
        patina: 0.7,
        edge_wear: 0.6,
        smoothness: 0.4,
        wax: 0.2,
        wax_sharpness: 30.0,
        checks: 0.15,
    },
    WoodPreset {
        id: "mahogany",
        name: "Mahogany",
        description: "Red-brown mahogany, fine and even, with a French-polished glow.",
        early: [0.42, 0.14, 0.08],
        late: [0.30, 0.09, 0.05],
        worn: [0.65, 0.25, 0.12],
        recess: [0.07, 0.02, 0.012],
        scatter: [0.80, 0.20, 0.10],
        subsurface: 0.35,
        ring_size: 0.02,
        ring_contrast: 0.35,
        latewood: 0.35,
        wobble: 0.6,
        figure_size: 0.3,
        streaks: 0.5,
        streak_size: 0.0015,
        pores: 0.25,
        end_grain: 0.3,
        patina: 0.8,
        edge_wear: 0.7,
        smoothness: 0.6,
        wax: 0.4,
        wax_sharpness: 70.0,
        checks: 0.0,
    },
    WoodPreset {
        id: "limewood",
        name: "Limewood",
        description: "Pale, close-grained limewood (linden) - the classic carver's wood - left nearly bare.",
        early: [0.78, 0.66, 0.48],
        late: [0.72, 0.58, 0.40],
        worn: [0.86, 0.74, 0.55],
        recess: [0.35, 0.25, 0.15],
        scatter: [0.90, 0.70, 0.45],
        subsurface: 0.3,
        ring_size: 0.02,
        ring_contrast: 0.35,
        latewood: 0.3,
        wobble: 0.6,
        figure_size: 0.3,
        streaks: 0.2,
        streak_size: 0.0012,
        pores: 0.0,
        end_grain: 0.25,
        patina: 0.6,
        edge_wear: 0.4,
        smoothness: 0.3,
        wax: 0.1,
        wax_sharpness: 25.0,
        checks: 0.0,
    },
    WoodPreset {
        id: "ebonized",
        name: "Ebonized",
        description: "Blackened wood, waxed to a dull gleam, with warm brown showing through on the worn edges.",
        early: [0.07, 0.05, 0.04],
        late: [0.04, 0.03, 0.025],
        worn: [0.28, 0.15, 0.07],
        recess: [0.015, 0.012, 0.01],
        scatter: [0.30, 0.12, 0.05],
        subsurface: 0.05,
        ring_size: 0.02,
        ring_contrast: 0.5,
        latewood: 0.3,
        wobble: 0.8,
        figure_size: 0.3,
        streaks: 0.3,
        streak_size: 0.0015,
        pores: 0.2,
        end_grain: 0.2,
        patina: 0.6,
        edge_wear: 1.0,
        smoothness: 0.52,
        wax: 0.35,
        wax_sharpness: 45.0,
        checks: 0.0,
    },
];

// ---- the settings -------------------------------------------------------------
//
// As the desktop's: a preset, and sliders that scale what the preset has
// (1 = as the preset has it), so a new preset keeps the sculptor's taste.
// `reference` is the model size the preset's sizes are measured against,
// frozen when a finish or preset is picked so the pattern does not crawl
// while the model is sculpted; 0 means "measure it".

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MetalSettings {
    pub preset: usize,
    pub exposure: f32,
    pub edge_wear: f32,
    pub wash: f32,
    pub detail: f32,
    pub pattern_size: f32,
    pub gloss: f32,
    pub seed: f32,
    pub reference: f32,
}

impl Default for MetalSettings {
    fn default() -> Self {
        MetalSettings {
            preset: 0,
            exposure: 1.0,
            edge_wear: 1.0,
            wash: 1.0,
            detail: 1.0,
            pattern_size: 1.0,
            gloss: 1.0,
            seed: 0.0,
            reference: 0.0,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct ClaySettings {
    pub preset: usize,
    pub gloss: f32,
    pub wetness: f32,
    pub subsurface: f32,
    pub recess: f32,
    pub detail: f32,
    pub grain: f32,
    pub reference: f32,
}

/// The Plain finish: a colour (sRGB), how metal it is and how smooth.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PlainSettings {
    pub color: Rgb,
    pub metallic: f32,
    pub smoothness: f32,
}

impl Default for PlainSettings {
    fn default() -> Self {
        PlainSettings { color: [0.78, 0.76, 0.74], metallic: 0.0, smoothness: 0.35 }
    }
}

impl Default for ClaySettings {
    fn default() -> Self {
        ClaySettings { preset: 3, gloss: 1.0, wetness: 1.0, subsurface: 1.0, recess: 1.0, detail: 1.0, grain: 1.0, reference: 0.0 }
    }
}

#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WoodSettings {
    pub preset: usize,
    /// 0 vertical, 1 left-right, 2 front-back: the log the model was carved from.
    pub axis: usize,
    pub grain: f32,
    pub grain_scale: f32,
    pub edge_wear: f32,
    pub patina: f32,
    pub detail: f32,
    pub sheen: f32,
    pub cracks: f32,
    pub seed: f32,
    pub reference: f32,
}

impl Default for WoodSettings {
    fn default() -> Self {
        WoodSettings {
            preset: 0,
            axis: 0,
            grain: 1.0,
            grain_scale: 1.0,
            edge_wear: 1.0,
            patina: 1.0,
            detail: 1.0,
            sheen: 1.0,
            cracks: 1.0,
            seed: 0.0,
            reference: 0.0,
        }
    }
}

/// The finish in use and every finish's settings: what the Look tab edits,
/// kept between visits.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Finishes {
    pub finish: Finish,
    pub clay: ClaySettings,
    pub metal: MetalSettings,
    pub wood: WoodSettings,
    pub plain: PlainSettings,
    /// How bright the lights are (the desktop's Brightness).
    pub brightness: f32,
    /// The Lighting card: the preset rig and how it is turned.
    pub lighting: Lighting,
    /// The Effects card: exposure, tone curve, colour, vignette, grain.
    pub effects: Effects,
}

/// The settings a finish's shader function reads: twelve slots, the
/// finish's own layout (see `finish_clay` and the others in finish.wgsl).
pub type Params = [[f32; 4]; 12];

impl Finishes {
    pub fn new() -> Finishes {
        Finishes { brightness: 1.0, ..Default::default() }
    }

    /// The settings kept in the browser, or the defaults.
    pub fn from_pref(text: &str) -> Finishes {
        let mut f: Finishes = serde_json::from_str(text).unwrap_or_else(|_| Finishes::new());
        if !(f.brightness > 0.0) {
            f.brightness = 1.0;
        }
        f
    }

    pub fn to_pref(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    /// The model size the finish in use was frozen against, or 0.
    pub fn reference(&self) -> f32 {
        match self.finish {
            Finish::Matcap | Finish::Plain => 0.0,
            Finish::Clay => self.clay.reference,
            Finish::Metal => self.metal.reference,
            Finish::Wood => self.wood.reference,
        }
    }

    /// Freezes the finish in use against a model `size` across.
    pub fn freeze(&mut self, size: f32) {
        match self.finish {
            Finish::Matcap | Finish::Plain => {}
            Finish::Clay => self.clay.reference = size,
            Finish::Metal => self.metal.reference = size,
            Finish::Wood => self.wood.reference = size,
        }
    }

    /// The finish in use as its shader function reads it, its patterns at
    /// `size` (the model's largest extent, when it was not frozen).
    pub fn params(&self, size: f32) -> Params {
        let size = if self.reference() > 0.0 { self.reference() } else { size.max(1e-4) };
        let mut out = [[0.0; 4]; 12];
        let slots: Vec<[f32; 4]> = match self.finish {
            Finish::Matcap => Vec::new(),
            Finish::Clay => self.pack_clay(size).to_vec(),
            Finish::Metal => self.pack_metal(size).to_vec(),
            Finish::Wood => self.pack_wood(size).to_vec(),
            Finish::Plain => {
                let p = &self.plain;
                vec![rgba(p.color, 1.0), [clamp01(p.metallic), clamp01(p.smoothness), 0.0, 0.0]]
            }
        };
        out[..slots.len()].copy_from_slice(&slots);
        out
    }

    /// The aged metal (see `finish_metal`), its pattern at `size`: the
    /// desktop's `Look::pack_metal`.
    fn pack_metal(&self, size: f32) -> [[f32; 4]; 10] {
        const EXPOSURE_REACH: f32 = 0.6;
        const PATTERN_PATH_LENGTH: f32 = 40.0;
        let s = &self.metal;
        let p = &METALS[idx(s.preset, METALS.len())];
        let path = Vec3::new(0.53, 0.29, 0.80).normalize() * (clamp01(s.seed) * PATTERN_PATH_LENGTH);
        [
            rgba(p.metal, 1.0),
            rgba(p.coat_a, 1.0),
            rgba(p.coat_b, 1.0),
            rgba(p.wash_color, 1.0),
            [path.x, path.y, path.z, 0.0],
            [clamp01(p.metal_smoothness * s.gloss), p.metal_metallic, p.coat_metallic, clamp01(p.coat_smoothness * s.gloss)],
            [
                clamp01(p.wash * s.wash),
                p.coverage - (s.exposure - 1.0) * EXPOSURE_REACH,
                p.edge_wear * s.edge_wear,
                p.patch_size * size * s.pattern_size,
            ],
            [p.patch_contrast, p.patchiness, p.follows_recesses, p.grain_size * size * s.pattern_size],
            [p.grain, s.detail, p.patch_breakup, p.sparkle],
            [p.undulation, p.undulation_size * size * s.pattern_size, 0.0, 0.0],
        ]
    }

    /// The sculptor's clay (see `finish_clay`): the desktop's `Look::pack_clay`.
    fn pack_clay(&self, size: f32) -> [[f32; 4]; 6] {
        let s = &self.clay;
        let p = &CLAYS[idx(s.preset, CLAYS.len())];
        [
            rgba(p.color, 1.0),
            rgba(p.recess_color, 1.0),
            rgba(p.scatter_color, 1.0),
            [
                clamp01(p.smoothness * s.gloss),
                // Gloss takes the wet film with it: a matte clay has no wet shine either.
                p.wetness * s.wetness * s.gloss.min(1.0),
                p.wet_sharpness,
                p.subsurface * s.subsurface,
            ],
            [p.recess_depth * s.recess, p.ridge_burnish, p.mottle, p.mottle_size * size],
            [p.grain * s.grain, p.grain_size * size, s.detail, 0.0],
        ]
    }

    /// The carved wood (see `finish_wood`): the desktop's `Look::pack_wood`.
    fn pack_wood(&self, size: f32) -> [[f32; 4]; 12] {
        const PITH_START: f32 = 0.12;
        const PITH_PATH_LENGTH: f32 = 0.9;
        let s = &self.wood;
        let p = &WOODS[idx(s.preset, WOODS.len())];
        let scale = size * s.grain_scale;
        let axis = match s.axis {
            1 => [1.0, 0.0, 0.0, 0.0],
            2 => [0.0, 0.0, 1.0, 0.0],
            _ => [0.0, 1.0, 0.0, 0.0],
        };
        let pith = Vec3::new(0.61, 0.17, -0.77).normalize() * ((PITH_START + clamp01(s.seed) * PITH_PATH_LENGTH) * size);
        [
            rgba(p.early, 1.0),
            rgba(p.late, 1.0),
            rgba(p.worn, 1.0),
            rgba(p.recess, 1.0),
            rgba(p.scatter, 1.0),
            axis,
            [pith.x, pith.y, pith.z, 0.0],
            [p.ring_size * scale, p.ring_contrast * s.grain, p.latewood, p.wobble],
            [p.figure_size * scale, p.streaks * s.grain, p.streak_size * scale, p.pores * s.grain],
            [p.end_grain, p.patina * s.patina, p.edge_wear * s.edge_wear, clamp01(p.smoothness * s.sheen)],
            // Sheen takes the wax with it; the cracks slider adds to a preset that has none.
            [p.wax * s.sheen, p.wax_sharpness, p.subsurface, clamp01(p.checks + (s.cracks - 1.0) * 0.5)],
            [s.detail, 0.0, 0.0, 0.0],
        ]
    }

    /// The uniform the shader reads: the finish, the lights turned to a view
    /// whose axes are `right`, `up` and `forward`, the tone settings and the
    /// finish's own settings.
    pub fn uniform(&self, right: Vec3, up: Vec3, forward: Vec3, size: f32) -> Uniform {
        let rig = rig(&self.lighting, self.brightness.clamp(0.0, 3.0), right, up, forward);
        Uniform {
            kind: [self.finish.code(), rig.reflection, rig.count as f32, light_angle(&self.lighting)],
            light_dir: rig.dir,
            light_color: rig.color,
            sky: rig.ambient[0],
            equator: rig.ambient[1],
            ground: rig.ambient[2],
            tone: self.effects.tone(),
            v: self.params(size),
        }
    }
}

/// The finish's part of the shader's globals (`Finish` in finish.wgsl).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Uniform {
    /// x: the finish (0 matcap, 1 clay, 2 metal, 3 wood, 4 plain), y: how
    /// strongly the sky is reflected, z: how many lights, w: how wide they
    /// look (radians).
    pub kind: [f32; 4],
    pub light_dir: [[f32; 4]; 5],
    pub light_color: [[f32; 4]; 5],
    pub sky: [f32; 4],
    pub equator: [f32; 4],
    pub ground: [f32; 4],
    /// x: exposure (times), y: the tone curve, z: film grain.
    pub tone: [f32; 4],
    pub v: Params,
}

impl Default for Uniform {
    fn default() -> Self {
        bytemuck::Zeroable::zeroed()
    }
}

/// The preset names of a finish, for its menu.
pub fn preset_names(finish: Finish) -> Vec<&'static str> {
    match finish {
        Finish::Matcap | Finish::Plain => Vec::new(),
        Finish::Clay => CLAYS.iter().map(|p| p.name).collect(),
        Finish::Metal => METALS.iter().map(|p| p.name).collect(),
        Finish::Wood => WOODS.iter().map(|p| p.name).collect(),
    }
}

/// What a preset of a finish looks like, in a sentence.
pub fn preset_about(finish: Finish, i: usize) -> &'static str {
    match finish {
        Finish::Matcap | Finish::Plain => "",
        Finish::Clay => CLAYS[idx(i, CLAYS.len())].description,
        Finish::Metal => METALS[idx(i, METALS.len())].description,
        Finish::Wood => WOODS[idx(i, WOODS.len())].description,
    }
}
