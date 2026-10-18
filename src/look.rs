//! The look of the viewport, ported from the Unity app: the surface finishes
//! (lure plastic, aged metal, clay, wood), the glow layer, the lighting
//! presets, the HDRI environment and the post effects, plus the block of
//! shader inputs they turn into each frame.
//!
//! Colours here are written as they are in the Unity presets (sRGB) and are
//! converted to linear when the shader inputs are built. Sizes in the finish
//! presets are fractions of the model's size, measured when a preset is
//! picked, so a finish looks the same on a small sphere and a large import.

use std::path::PathBuf;

use bytemuck::{Pod, Zeroable};
use glam::{Mat4, Vec3};

pub type Rgb = [f32; 3];

fn lin1(c: f32) -> f32 {
    if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }
}

pub fn linear(c: Rgb) -> Rgb {
    [lin1(c[0]), lin1(c[1]), lin1(c[2])]
}

fn rgba(c: Rgb, a: f32) -> [f32; 4] {
    let c = linear(c);
    [c[0], c[1], c[2], a]
}

// ---- lighting presets ------------------------------------------------------

/// One light of a preset, the way a photographer would describe it. Azimuth
/// is measured from the viewer: 0 is straight from the camera, +90 from
/// screen-right, 180 from behind the subject. Elevation is degrees above the
/// horizon.
#[derive(Clone, Copy)]
pub struct PresetLight {
    pub azimuth: f32,
    pub elevation: f32,
    pub color: Rgb,
    pub intensity: f32,
}

const fn light(azimuth: f32, elevation: f32, r: f32, g: f32, b: f32, intensity: f32) -> PresetLight {
    PresetLight { azimuth, elevation, color: [r, g, b], intensity }
}

/// A complete lighting look: key, fill and rim (3-point), plus kicker and top
/// (5-point), and the sky / equator / ground ambient they sit in.
pub struct LightingPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub lights: [PresetLight; 5],
    pub ambient_sky: Rgb,
    pub ambient_equator: Rgb,
    pub ambient_ground: Rgb,
    /// How dark the key light's shadow gets.
    pub shadow_strength: f32,
    /// Strength of the environment reflection.
    pub reflection: f32,
    /// How wide the key light looks from the subject, in degrees: a small
    /// hard source casts crisp shadows, a big softbox soft ones. The shadow
    /// is crisp where it meets what casts it and softens with distance.
    pub light_size: f32,
}

pub const LIGHTING: [LightingPreset; 8] = [
    LightingPreset {
        id: "studio",
        name: "Studio",
        description: "Balanced neutral studio light - the all-round default for judging form.",
        lights: [
            light(-40.0, 40.0, 1.00, 0.97, 0.92, 1.56),
            light(50.0, 10.0, 0.86, 0.91, 1.00, 0.4),
            light(150.0, 35.0, 1.00, 1.00, 1.00, 1.08),
            light(-140.0, 5.0, 1.00, 0.97, 0.92, 0.54),
            light(0.0, 85.0, 1.00, 1.00, 1.00, 0.3),
        ],
        ambient_sky: [0.42, 0.44, 0.48],
        ambient_equator: [0.30, 0.30, 0.31],
        ambient_ground: [0.16, 0.15, 0.14],
        shadow_strength: 0.75,
        reflection: 0.6,
        light_size: 6.0,
    },
    LightingPreset {
        id: "softclay",
        name: "Soft Clay",
        description: "Low contrast and soft shadows - easy on the eyes for long detailing sessions.",
        lights: [
            light(-30.0, 45.0, 1.00, 0.98, 0.95, 1.14),
            light(60.0, 20.0, 0.95, 0.97, 1.00, 0.66),
            light(170.0, 40.0, 1.00, 1.00, 1.00, 0.6),
            light(-120.0, 10.0, 1.00, 0.98, 0.95, 0.36),
            light(0.0, 85.0, 1.00, 1.00, 1.00, 0.42),
        ],
        ambient_sky: [0.42, 0.43, 0.45],
        ambient_equator: [0.33, 0.33, 0.34],
        ambient_ground: [0.21, 0.20, 0.19],
        shadow_strength: 0.35,
        reflection: 0.4,
        light_size: 14.0,
    },
    LightingPreset {
        id: "highkey",
        name: "High Key",
        description: "Bright, airy and nearly shadowless - clean product-shot presentation.",
        lights: [
            light(-25.0, 35.0, 1.00, 1.00, 1.00, 1.32),
            light(45.0, 5.0, 1.00, 1.00, 1.00, 0.7),
            light(180.0, 30.0, 1.00, 1.00, 1.00, 1.32),
            light(-130.0, 0.0, 1.00, 1.00, 1.00, 0.72),
            light(0.0, 80.0, 1.00, 1.00, 1.00, 0.6),
        ],
        ambient_sky: [0.58, 0.59, 0.61],
        ambient_equator: [0.48, 0.48, 0.49],
        ambient_ground: [0.36, 0.35, 0.34],
        shadow_strength: 0.25,
        reflection: 0.8,
        light_size: 16.0,
    },
    LightingPreset {
        id: "vibrant",
        name: "Vibrant",
        description: "Saturated complementary colours - warm key, blue fill, magenta and teal edges.",
        lights: [
            light(-45.0, 35.0, 1.00, 0.82, 0.55, 1.56),
            light(55.0, 5.0, 0.30, 0.55, 1.00, 0.84),
            light(145.0, 25.0, 1.00, 0.25, 0.65, 1.8),
            light(-140.0, 0.0, 0.15, 0.95, 0.85, 1.32),
            light(0.0, 80.0, 0.75, 0.55, 1.00, 0.48),
        ],
        ambient_sky: [0.35, 0.30, 0.50],
        ambient_equator: [0.25, 0.22, 0.32],
        ambient_ground: [0.15, 0.10, 0.15],
        shadow_strength: 0.7,
        reflection: 0.7,
        light_size: 6.0,
    },
    LightingPreset {
        id: "cinematic",
        name: "Cinematic",
        description: "Teal and orange film look - warm key against cool rim and shadows.",
        lights: [
            light(-50.0, 30.0, 1.00, 0.72, 0.42, 1.68),
            light(60.0, 0.0, 0.25, 0.60, 0.70, 0.42),
            light(160.0, 25.0, 0.35, 0.85, 1.00, 1.56),
            light(-135.0, 5.0, 0.30, 0.75, 0.85, 0.84),
            light(0.0, 80.0, 0.90, 0.80, 0.70, 0.18),
        ],
        ambient_sky: [0.18, 0.26, 0.30],
        ambient_equator: [0.12, 0.16, 0.18],
        ambient_ground: [0.06, 0.06, 0.06],
        shadow_strength: 0.9,
        reflection: 0.5,
        light_size: 4.0,
    },
    LightingPreset {
        id: "golden",
        name: "Golden Hour",
        description: "Low warm sun raking across the form, with a cool sky fill.",
        lights: [
            light(-70.0, 12.0, 1.00, 0.62, 0.30, 1.8),
            light(60.0, 25.0, 0.45, 0.50, 0.85, 0.48),
            light(140.0, 15.0, 1.00, 0.78, 0.45, 1.44),
            light(-150.0, 5.0, 1.00, 0.55, 0.35, 0.72),
            light(0.0, 80.0, 0.55, 0.60, 0.95, 0.3),
        ],
        ambient_sky: [0.40, 0.42, 0.62],
        ambient_equator: [0.35, 0.27, 0.30],
        ambient_ground: [0.18, 0.12, 0.08],
        shadow_strength: 0.85,
        reflection: 0.6,
        light_size: 3.0,
    },
    LightingPreset {
        id: "moody",
        name: "Moody",
        description: "Cool low-key light - deep shadows, a bright blue rim, very little fill.",
        lights: [
            light(-60.0, 45.0, 0.85, 0.90, 1.00, 1.56),
            light(60.0, 0.0, 0.35, 0.45, 0.70, 0.11),
            light(155.0, 30.0, 0.55, 0.75, 1.00, 1.44),
            light(-140.0, 0.0, 0.45, 0.55, 0.85, 0.48),
            light(0.0, 85.0, 0.70, 0.80, 1.00, 0.15),
        ],
        ambient_sky: [0.10, 0.12, 0.18],
        ambient_equator: [0.05, 0.06, 0.08],
        ambient_ground: [0.02, 0.02, 0.02],
        shadow_strength: 1.0,
        reflection: 0.25,
        light_size: 5.0,
    },
    LightingPreset {
        id: "dramatic",
        name: "Dramatic",
        description: "Hard side key and a hot rim out of near-black - chiaroscuro.",
        lights: [
            light(-85.0, 30.0, 1.00, 0.95, 0.88, 2.04),
            light(80.0, 10.0, 0.80, 0.85, 1.00, 0.05),
            light(165.0, 35.0, 1.00, 1.00, 1.00, 1.8),
            light(-150.0, 0.0, 1.00, 0.90, 0.80, 0.6),
            light(0.0, 85.0, 1.00, 1.00, 1.00, 0.12),
        ],
        ambient_sky: [0.06, 0.06, 0.07],
        ambient_equator: [0.03, 0.03, 0.03],
        ambient_ground: [0.01, 0.01, 0.01],
        shadow_strength: 1.0,
        reflection: 0.2,
        light_size: 2.0,
    },
];

pub const MAX_BRIGHTNESS: f32 = 2.0;

// ---- finish presets --------------------------------------------------------

/// One soft-plastic lure colourway: the plastic at its thinnest and thickest
/// and the glitter cast into it.
pub struct LurePreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    pub thin: Rgb,
    pub thick: Rgb,
    /// Thickness over which thin colour gives way to thick.
    pub depth: f32,
    pub transmission: f32,
    /// Glitter colours; the fourth number is each colour's share of the flakes.
    pub flakes: [[f32; 4]; 3],
    pub flake_density: f32,
    pub flake_size: f32,
    /// 0 = hexagonal glitter, 1 = square.
    pub flake_shape: f32,
    pub micro: Rgb,
    pub micro_density: f32,
    pub micro_size: f32,
}

/// Tilt of flakes away from the skin.
const FLAKE_TILT: f32 = 0.35;

pub const LURES: [LurePreset; 6] = [
    LurePreset {
        id: "black_blue_flake",
        name: "Black & Blue Flake",
        description: "Smoky black plastic packed with bright blue hex glitter.",
        thin: [0.13, 0.15, 0.22],
        thick: [0.035, 0.04, 0.055],
        depth: 0.05,
        transmission: 0.35,
        flakes: [[0.16, 0.52, 1.00, 0.6], [0.06, 0.32, 0.95, 0.4], [0.0, 0.0, 0.0, 0.0]],
        flake_density: 0.55,
        flake_size: 0.014,
        flake_shape: 0.0,
        micro: [0.10, 0.42, 0.95],
        micro_density: 0.3,
        micro_size: 0.0045,
    },
    LurePreset {
        id: "pumpkin_flake",
        name: "Pumpkin Flake",
        description: "Green pumpkin: olive body, amber claws, green and purple glitter.",
        thin: [0.80, 0.60, 0.16],
        thick: [0.33, 0.30, 0.14],
        depth: 0.05,
        transmission: 0.9,
        flakes: [[0.45, 0.95, 0.30, 0.55], [0.55, 0.25, 0.95, 0.30], [0.95, 0.30, 0.55, 0.15]],
        flake_density: 0.5,
        flake_size: 0.012,
        flake_shape: 0.0,
        micro: [0.95, 0.82, 0.50],
        micro_density: 0.15,
        micro_size: 0.004,
    },
    LurePreset {
        id: "blue_flake",
        name: "Blue Flake",
        description: "Translucent purple-blue plastic with teal glitter.",
        thin: [0.52, 0.52, 1.00],
        thick: [0.20, 0.10, 0.64],
        depth: 0.07,
        transmission: 1.0,
        flakes: [[0.10, 0.85, 0.75, 0.7], [0.05, 0.55, 0.50, 0.3], [0.0, 0.0, 0.0, 0.0]],
        flake_density: 0.55,
        flake_size: 0.012,
        flake_shape: 0.0,
        micro: [0.08, 0.06, 0.30],
        micro_density: 0.2,
        micro_size: 0.004,
    },
    LurePreset {
        id: "amber_flake",
        name: "Amber Flake",
        description: "Amber plastic with black square flake and fine red pepper.",
        thin: [0.88, 0.74, 0.22],
        thick: [0.28, 0.24, 0.10],
        depth: 0.05,
        transmission: 0.9,
        flakes: [[0.02, 0.02, 0.02, 1.0], [0.0, 0.0, 0.0, 0.0], [0.0, 0.0, 0.0, 0.0]],
        flake_density: 0.4,
        flake_size: 0.013,
        flake_shape: 1.0,
        micro: [0.85, 0.16, 0.08],
        micro_density: 0.35,
        micro_size: 0.0035,
    },
    LurePreset {
        id: "watermelon_red_flake",
        name: "Watermelon Red Flake",
        description: "Pale translucent watermelon green with red and black flake.",
        thin: [0.68, 0.82, 0.52],
        thick: [0.36, 0.49, 0.27],
        depth: 0.07,
        transmission: 1.0,
        flakes: [[0.95, 0.35, 0.35, 0.6], [0.03, 0.04, 0.03, 0.4], [0.0, 0.0, 0.0, 0.0]],
        flake_density: 0.35,
        flake_size: 0.013,
        flake_shape: 0.5,
        micro: [0.12, 0.20, 0.10],
        micro_density: 0.08,
        micro_size: 0.004,
    },
    LurePreset {
        id: "red_flake",
        name: "Red Flake",
        description: "Red plastisol with gold, black and silver flake.",
        thin: [0.95, 0.28, 0.20],
        thick: [0.50, 0.05, 0.06],
        depth: 0.06,
        transmission: 0.9,
        flakes: [[1.00, 0.80, 0.30, 0.45], [0.03, 0.03, 0.03, 0.30], [0.85, 0.87, 0.92, 0.25]],
        flake_density: 0.5,
        flake_size: 0.012,
        flake_shape: 0.0,
        micro: [0.30, 0.02, 0.03],
        micro_density: 0.25,
        micro_size: 0.004,
    },
];

/// One aged-metal finish: a bare metal, a coat over it that gathers in
/// recesses and wears off raised edges, and a dark wash in the deepest ones.
pub struct MetalPreset {
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

/// One cast-resin look, for seeing a print or a casting as a buyer will: a
/// glossy surface that reflects at a glance and lets the world through head
/// on, bent by the resin and tinted by as much of it as the view crosses.
pub struct ResinPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// The colour light takes on through `depth` of resin.
    pub color: Rgb,
    /// How much resin, as a fraction of the model's size, gives that colour.
    pub depth: f32,
    /// Milkiness: how much of the light scatters inside instead of passing
    /// through (0 water-clear, 1 opaque).
    pub haze: f32,
    pub smoothness: f32,
    /// Light from behind glowing through.
    pub transmission: f32,
}

pub const RESINS: [ResinPreset; 7] = [
    ResinPreset {
        id: "clear",
        name: "Clear",
        description: "Water-clear casting resin: all reflections and the world seen through it.",
        color: [0.96, 0.98, 1.0],
        depth: 0.6,
        haze: 0.02,
        smoothness: 0.97,
        transmission: 1.0,
    },
    ResinPreset {
        id: "smoky",
        name: "Smoky",
        description: "Smoky grey resin: dark where it is thick, see-through at the edges.",
        color: [0.42, 0.40, 0.38],
        depth: 0.15,
        haze: 0.05,
        smoothness: 0.95,
        transmission: 0.8,
    },
    ResinPreset {
        id: "amber",
        name: "Amber",
        description: "Warm amber, golden at the edges and deep honey in the thick parts.",
        color: [1.0, 0.62, 0.18],
        depth: 0.25,
        haze: 0.04,
        smoothness: 0.96,
        transmission: 1.0,
    },
    ResinPreset {
        id: "ruby",
        name: "Ruby",
        description: "Deep transparent red.",
        color: [0.9, 0.06, 0.14],
        depth: 0.2,
        haze: 0.03,
        smoothness: 0.97,
        transmission: 1.0,
    },
    ResinPreset {
        id: "emerald",
        name: "Emerald",
        description: "Transparent green.",
        color: [0.12, 0.78, 0.38],
        depth: 0.2,
        haze: 0.03,
        smoothness: 0.97,
        transmission: 1.0,
    },
    ResinPreset {
        id: "sapphire",
        name: "Sapphire",
        description: "Transparent blue.",
        color: [0.12, 0.32, 0.95],
        depth: 0.2,
        haze: 0.03,
        smoothness: 0.97,
        transmission: 1.0,
    },
    ResinPreset {
        id: "frosted",
        name: "Frosted",
        description: "Milky translucent white, like a frosted print: light glows through it softly.",
        color: [0.95, 0.96, 1.0],
        depth: 0.4,
        haze: 0.55,
        smoothness: 0.6,
        transmission: 1.0,
    },
];

/// One glow look: an emissive layer over whatever shades the surface. The
/// pattern number is what the shader switches on.
pub struct GlowPreset {
    pub id: &'static str,
    pub name: &'static str,
    pub description: &'static str,
    /// 0 neon, 1 aura, 2 lava, 3 veins, 4 forge, 5 hologram, 6 spots, 7 inner light.
    pub pattern: u32,
    pub hot: Rgb,
    pub cool: Rgb,
    pub intensity: f32,
    pub coverage: f32,
    pub rim: f32,
    pub pattern_size: f32,
    pub speed: f32,
    pub pulse: f32,
    pub pulse_rate: f32,
    pub dim: f32,
}

pub const GLOWS: [GlowPreset; 8] = [
    GlowPreset {
        id: "lava",
        name: "Lava Cracks",
        description: "Dark cooled crust split by glowing molten cracks, with lava welling up in the recesses. The plates slowly shift.",
        pattern: 2,
        hot: [1.0, 0.85, 0.35],
        cool: [1.0, 0.18, 0.02],
        intensity: 4.0,
        coverage: 0.5,
        rim: 0.0,
        pattern_size: 0.12,
        speed: 1.0,
        pulse: 0.0,
        pulse_rate: 0.4,
        dim: 0.8,
    },
    GlowPreset {
        id: "neon",
        name: "Neon",
        description: "The whole model lit up like a neon tube - a pale hot core where it faces you, saturated colour at the edges.",
        pattern: 0,
        hot: [1.0, 0.78, 0.95],
        cool: [1.0, 0.1, 0.6],
        intensity: 2.5,
        coverage: 1.0,
        rim: 0.0,
        pattern_size: 0.2,
        speed: 1.0,
        pulse: 0.0,
        pulse_rate: 0.5,
        dim: 0.6,
    },
    GlowPreset {
        id: "aura",
        name: "Spirit Aura",
        description: "A cold glow hugging the silhouette, breathing slowly - ghosts, force fields, magic.",
        pattern: 1,
        hot: [0.75, 1.0, 1.0],
        cool: [0.1, 0.55, 1.0],
        intensity: 3.0,
        coverage: 0.5,
        rim: 0.0,
        pattern_size: 0.2,
        speed: 1.0,
        pulse: 0.25,
        pulse_rate: 0.3,
        dim: 0.3,
    },
    GlowPreset {
        id: "veins",
        name: "Energy Veins",
        description: "Branching lines of electric light crawling across the surface.",
        pattern: 3,
        hot: [0.85, 0.95, 1.0],
        cool: [0.25, 0.4, 1.0],
        intensity: 4.0,
        coverage: 0.4,
        rim: 0.15,
        pattern_size: 0.25,
        speed: 1.0,
        pulse: 0.0,
        pulse_rate: 0.5,
        dim: 0.5,
    },
    GlowPreset {
        id: "forge",
        name: "Forge Heat",
        description: "Metal fresh from the forge: raised forms and thin edges glow yellow-hot, cooling to dull red down the sides.",
        pattern: 4,
        hot: [1.0, 0.8, 0.35],
        cool: [0.8, 0.08, 0.01],
        intensity: 3.0,
        coverage: 0.5,
        rim: 0.0,
        pattern_size: 0.3,
        speed: 0.5,
        pulse: 0.0,
        pulse_rate: 0.5,
        dim: 0.5,
    },
    GlowPreset {
        id: "hologram",
        name: "Hologram",
        description: "A flickering projection: bright rim, scanlines rolling up the model, the body nearly see-through dark.",
        pattern: 5,
        hot: [0.6, 1.0, 0.95],
        cool: [0.0, 0.75, 0.9],
        intensity: 2.0,
        coverage: 0.4,
        rim: 0.0,
        pattern_size: 0.02,
        speed: 1.0,
        pulse: 0.0,
        pulse_rate: 0.5,
        dim: 0.9,
    },
    GlowPreset {
        id: "bio",
        name: "Bioluminescent",
        description: "Soft glowing spots scattered over the skin, each pulsing on its own - deep-sea creature.",
        pattern: 6,
        hot: [0.7, 1.0, 0.85],
        cool: [0.05, 0.9, 0.6],
        intensity: 3.0,
        coverage: 0.5,
        rim: 0.2,
        pattern_size: 0.06,
        speed: 1.0,
        pulse: 0.0,
        pulse_rate: 0.5,
        dim: 0.4,
    },
    GlowPreset {
        id: "inner",
        name: "Inner Light",
        description: "Golden light from inside the model, shining out through every crease and recess.",
        pattern: 7,
        hot: [1.0, 0.95, 0.7],
        cool: [1.0, 0.55, 0.1],
        intensity: 3.0,
        coverage: 0.5,
        rim: 0.0,
        pattern_size: 0.2,
        speed: 1.0,
        pulse: 0.0,
        pulse_rate: 0.5,
        dim: 0.3,
    },
];

// ---- settings --------------------------------------------------------------

/// The surface finish that replaces plain base-colour shading. At most one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Finish {
    None,
    LurePlastic,
    Metal,
    Clay,
    Wood,
    Resin,
}

impl Finish {
    pub const ALL: [Finish; 6] =
        [Finish::None, Finish::LurePlastic, Finish::Metal, Finish::Clay, Finish::Wood, Finish::Resin];

    pub fn name(self) -> &'static str {
        match self {
            Finish::None => "None",
            Finish::LurePlastic => "Lure Plastic",
            Finish::Metal => "Metal",
            Finish::Clay => "Clay",
            Finish::Wood => "Wood",
            Finish::Resin => "Resin",
        }
    }
}

/// The tone curve that brings the scene's light into what a screen shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, serde::Serialize, serde::Deserialize)]
pub enum Tonemap {
    None,
    /// The Unity pipeline's neutral curve.
    Neutral,
    Aces,
    /// Khronos PBR Neutral: colours as they are up to near white, only the
    /// brightest rolled off, so a product's colours come out true. The curve
    /// new scenes start with.
    PbrNeutral,
    /// Troy Sobotka's AgX: a filmic roll-off that keeps bright saturated
    /// colours from turning flat and garish.
    Agx,
}

impl Tonemap {
    /// As the choice in the Backdrop & Effects card shows them, in its order.
    pub const ALL: [Tonemap; 5] = [Tonemap::PbrNeutral, Tonemap::Agx, Tonemap::Aces, Tonemap::Neutral, Tonemap::None];

    pub fn label(self) -> &'static str {
        match self {
            Tonemap::PbrNeutral => "PBR Neutral",
            Tonemap::Agx => "AgX",
            Tonemap::Aces => "ACES",
            Tonemap::Neutral => "Neutral",
            Tonemap::None => "None",
        }
    }

    /// The number the post shader switches on (`post.grade.x`).
    pub fn shader_index(self) -> f32 {
        match self {
            Tonemap::None => 0.0,
            Tonemap::Neutral => 1.0,
            Tonemap::Aces => 2.0,
            Tonemap::PbrNeutral => 3.0,
            Tonemap::Agx => 4.0,
        }
    }
}

// The sliders below are adjustments on top of whichever preset is picked
// (1 = as the preset has it), so switching preset keeps the user's taste.
// `reference` is the model size the preset's fractions are measured against,
// frozen when a preset is picked so the pattern does not crawl while
// sculpting; 0 means "measure it".

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct LureSettings {
    #[serde(with = "crate::io::preset::lure")]
    pub preset: usize,
    pub flake_size: f32,
    pub flake_amount: f32,
    pub sparkle: f32,
    pub translucency: f32,
    pub gloss: f32,
    pub reference: f32,
}

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct MetalSettings {
    #[serde(with = "crate::io::preset::metal")]
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

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct ClaySettings {
    #[serde(with = "crate::io::preset::clay")]
    pub preset: usize,
    pub gloss: f32,
    pub wetness: f32,
    pub subsurface: f32,
    pub recess: f32,
    pub detail: f32,
    pub grain: f32,
    pub reference: f32,
}

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct WoodSettings {
    #[serde(with = "crate::io::preset::wood")]
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

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct ResinSettings {
    #[serde(with = "crate::io::preset::resin")]
    pub preset: usize,
    /// How see-through it is: more lets light further in before its colour
    /// takes over.
    pub clarity: f32,
    pub haze: f32,
    pub gloss: f32,
    pub reference: f32,
}

impl Default for ResinSettings {
    fn default() -> Self {
        Self { preset: 0, clarity: 1.0, haze: 1.0, gloss: 1.0, reference: 0.0 }
    }
}

#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct GlowSettings {
    pub on: bool,
    #[serde(with = "crate::io::preset::glow")]
    pub preset: usize,
    pub intensity: f32,
    pub coverage: f32,
    pub rim: f32,
    pub pattern_size: f32,
    pub speed: f32,
    pub pulse: f32,
    pub dim: f32,
    pub hue_shift: f32,
    pub reference: f32,
}

/// The floor under the model: the backdrop itself, catching the key light's
/// shadow and the shading where the model meets it, and if glossy a soft
/// reflection of the model. It sits under the lowest point of what is shown
/// and fades out into the backdrop all round, so it has no edge.
#[derive(Clone, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct FloorSettings {
    pub on: bool,
    /// How dark the model's shadow on it gets (0..1).
    pub shadow: f32,
    /// How strongly it reflects the model (0 for not at all).
    pub reflection: f32,
    /// How blurred that reflection is: 0 a mirror, 1 a satin.
    pub roughness: f32,
}

impl Default for FloorSettings {
    fn default() -> Self {
        Self { on: false, shadow: 0.85, reflection: 0.0, roughness: 0.3 }
    }
}

/// Colour grading by a .cube look-up table (render/lut.rs). The table itself
/// is not in the scene file, only where it was, like the HDRI.
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct LutSettings {
    pub path: Option<PathBuf>,
    /// How much of the table's grading shows (0 none, 1 all).
    pub strength: f32,
    /// Set once a table is loaded (the renderer holds it).
    #[serde(skip)]
    pub loaded: bool,
    #[serde(skip)]
    pub status: String,
    #[serde(skip)]
    pub failed: bool,
}

impl Default for LutSettings {
    fn default() -> Self {
        Self { path: None, strength: 1.0, loaded: false, status: "No colour table loaded.".into(), failed: false }
    }
}

/// Image-based lighting from an equirectangular picture.
#[derive(serde::Serialize, serde::Deserialize)]
pub struct HdriSettings {
    pub enabled: bool,
    /// Set once a picture is loaded (the renderer holds the pixels).
    #[serde(skip)]
    pub loaded: bool,
    pub path: Option<PathBuf>,
    #[serde(skip)]
    pub status: String,
    #[serde(skip)]
    pub failed: bool,
    /// Degrees of yaw, 0-360: turns the light around the sculpt.
    pub rotation: f32,
    pub exposure: f32,
    pub ambient: f32,
    pub reflection: f32,
}

impl HdriSettings {
    /// True only when a picture is both loaded and switched on.
    pub fn active(&self) -> bool {
        self.enabled && self.loaded
    }
}

#[derive(serde::Serialize, serde::Deserialize)]
pub struct Look {
    pub metallic: f32,
    /// Procedural surface micro-bump: depth and scale.
    pub normal_strength: f32,
    pub normal_scale: f32,
    pub matcap_intensity: f32,
    pub matcap_tint: f32,
    pub finish: Finish,
    pub lure: LureSettings,
    pub metal: MetalSettings,
    pub clay: ClaySettings,
    pub wood: WoodSettings,
    #[serde(default)]
    pub resin: ResinSettings,
    pub glow: GlowSettings,

    #[serde(with = "crate::io::preset::lighting")]
    pub lighting: usize,
    pub five_point: bool,
    pub brightness: f32,
    /// Spins the whole rig around the subject's vertical axis, in degrees.
    pub rotation: f32,
    /// Lights locked to the camera (the key always from the upper left of the
    /// view) or frozen in the world.
    pub follow_camera: bool,
    /// The level direction the camera faced when the rig was unlocked.
    #[serde(with = "crate::io::vec3")]
    pub world_forward: Vec3,
    pub shadows: bool,
    /// How big the lights are against the preset's sizes (1 = as the preset
    /// has them, 0 = points): the key's shadow and every softbox's highlight.
    #[serde(default = "one")]
    pub shadow_softness: f32,
    /// Light the model with softboxes (rectangles facing it, render/ltc.rs)
    /// rather than plain directions: long soft highlights on gloss.
    #[serde(default = "yes")]
    pub softboxes: bool,
    pub hdri: HdriSettings,
    /// The shadow-catching floor (off unless asked for).
    #[serde(default)]
    pub floor: FloorSettings,
    /// Show the HDRI behind the model (when one is on).
    pub hdri_background: bool,
    pub gradient_bias: f32,

    pub ambient_occlusion: bool,
    /// Crisp shadows in fine creases, toward every light, where the shadow
    /// map is too coarse (drawn when the picture holds still).
    #[serde(default = "yes")]
    pub contact_shadows: bool,
    pub bloom: bool,
    pub bloom_intensity: f32,
    pub bloom_threshold: f32,
    pub vignette: bool,
    pub vignette_intensity: f32,
    pub vignette_smoothness: f32,
    pub grading: bool,
    pub saturation: f32,
    pub contrast: f32,
    /// Film grain over the finished picture (0 none, 1 heavy).
    #[serde(default)]
    pub film_grain: f32,
    /// Crisper edges (0 none, 1 strong).
    #[serde(default)]
    pub sharpen: f32,
    /// A colour table graded over the finished picture.
    #[serde(default)]
    pub lut: LutSettings,
    /// Brightness of the whole picture in photographic stops: +1 doubles
    /// the light, -1 halves it. Applied before the tone curve.
    #[serde(default)]
    pub exposure: f32,
    pub tonemap: Tonemap,
    /// Blur what is nearer or farther than the focus distance, as a camera
    /// lens does.
    pub depth_of_field: bool,
    /// Distance from the camera that stays sharp, on the Unity app's 0.1-10
    /// scale (see `render/dof.rs` for how that maps to distances here).
    pub focus_distance: f32,
    /// The lens's f-number, 1-32: lower blurs more.
    pub aperture: f32,
}

/// For settings added after scenes were first saved: what a file without
/// them reads as.
fn one() -> f32 {
    1.0
}

fn yes() -> bool {
    true
}

impl Default for Look {
    fn default() -> Self {
        Self {
            metallic: 0.0,
            normal_strength: 0.0,
            normal_scale: 60.0,
            matcap_intensity: 1.0,
            matcap_tint: 0.0,
            finish: Finish::None,
            lure: LureSettings {
                preset: 0,
                flake_size: 1.0,
                flake_amount: 1.0,
                sparkle: 1.0,
                translucency: 1.0,
                gloss: 0.8,
                reference: 0.0,
            },
            metal: MetalSettings {
                preset: 0,
                exposure: 1.0,
                edge_wear: 1.0,
                wash: 1.0,
                detail: 1.0,
                pattern_size: 1.0,
                gloss: 1.0,
                seed: 0.0,
                reference: 0.0,
            },
            clay: ClaySettings {
                preset: 3,
                gloss: 1.0,
                wetness: 1.0,
                subsurface: 1.0,
                recess: 1.0,
                detail: 1.0,
                grain: 1.0,
                reference: 0.0,
            },
            wood: WoodSettings {
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
            },
            resin: ResinSettings::default(),
            glow: GlowSettings {
                on: false,
                preset: 0,
                intensity: 1.0,
                coverage: 1.0,
                rim: 1.0,
                pattern_size: 1.0,
                speed: 1.0,
                pulse: 1.0,
                dim: 1.0,
                hue_shift: 0.0,
                reference: 0.0,
            },
            lighting: 0,
            five_point: false,
            brightness: 1.0,
            rotation: 0.0,
            follow_camera: true,
            world_forward: Vec3::NEG_Z,
            shadows: true,
            shadow_softness: 1.0,
            softboxes: true,
            hdri: HdriSettings {
                enabled: false,
                loaded: false,
                path: None,
                status: "No HDRI loaded.".into(),
                failed: false,
                rotation: 0.0,
                exposure: 1.0,
                ambient: 1.0,
                reflection: 1.0,
            },
            floor: FloorSettings::default(),
            hdri_background: false,
            gradient_bias: 1.0,
            ambient_occlusion: true,
            contact_shadows: true,
            bloom: true,
            bloom_intensity: 0.25,
            bloom_threshold: 1.0,
            vignette: true,
            vignette_intensity: 0.2,
            vignette_smoothness: 0.2,
            grading: false,
            saturation: 0.0,
            contrast: 0.0,
            film_grain: 0.0,
            sharpen: 0.0,
            lut: LutSettings::default(),
            exposure: 0.0,
            tonemap: Tonemap::PbrNeutral,
            // Off, at the Unity pipeline's own starting focus and aperture.
            depth_of_field: false,
            focus_distance: 10.0,
            aperture: 5.6,
        }
    }
}

impl Look {
    /// Whether the picture changes with time, so the viewport must keep
    /// redrawing.
    pub fn animated(&self) -> bool {
        let g = &self.glow;
        let p = &GLOWS[g.preset.min(GLOWS.len() - 1)];
        g.on && (p.speed * g.speed > 0.0 || (p.pulse + (g.pulse - 1.0) * 0.5) > 0.0)
    }

    /// Whether the shader will read per-vertex curvature (recess / ridge).
    pub fn needs_curvature(&self, matcap: bool) -> bool {
        self.glow.on || (!matcap && matches!(self.finish, Finish::Metal | Finish::Clay | Finish::Wood))
    }

    /// Picks a finish. `size` is the model's size now, which the finish's
    /// pattern is frozen against the first time it is used.
    pub fn set_finish(&mut self, finish: Finish, size: f32) {
        self.finish = finish;
        let reference = match finish {
            Finish::None => return,
            Finish::LurePlastic => &mut self.lure.reference,
            Finish::Metal => &mut self.metal.reference,
            Finish::Clay => &mut self.clay.reference,
            Finish::Wood => &mut self.wood.reference,
            Finish::Resin => &mut self.resin.reference,
        };
        if *reference <= 0.0 {
            *reference = size;
        }
    }

    /// Picks a preset of a finish and turns that finish on, measuring the
    /// model afresh: picking a preset is the moment the user is looking at
    /// what they want it on.
    pub fn pick_preset(&mut self, finish: Finish, preset: usize, size: f32) {
        match finish {
            Finish::None => {}
            Finish::LurePlastic => (self.lure.preset, self.lure.reference) = (preset, size),
            Finish::Metal => (self.metal.preset, self.metal.reference) = (preset, size),
            Finish::Clay => (self.clay.preset, self.clay.reference) = (preset, size),
            Finish::Wood => (self.wood.preset, self.wood.reference) = (preset, size),
            Finish::Resin => (self.resin.preset, self.resin.reference) = (preset, size),
        }
        self.finish = finish;
    }

    pub fn pick_glow(&mut self, preset: usize, size: f32) {
        self.glow.preset = preset;
        self.glow.reference = size;
        self.glow.on = true;
    }

    pub fn set_glow(&mut self, on: bool, size: f32) {
        self.glow.on = on;
        if on && self.glow.reference <= 0.0 {
            self.glow.reference = size;
        }
    }

    /// Call every frame with the camera's axes. While the rig follows the
    /// camera this remembers its heading, so that unlocking freezes the
    /// lights where they are on screen right then. Only the heading is kept:
    /// a world rig stays level whatever angle it was unlocked at.
    pub fn track_camera(&mut self, cam_forward: Vec3, cam_up: Vec3) {
        if !self.follow_camera {
            return;
        }
        let mut level = Vec3::new(cam_forward.x, 0.0, cam_forward.z);
        if level.length_squared() < 1e-6 {
            // Looking straight down or up: the heading is where the top of the view points.
            level = Vec3::new(cam_up.x, 0.0, cam_up.z) * -cam_forward.y.signum();
        }
        self.world_forward = level.normalize_or(Vec3::NEG_Z);
    }
}

// ---- shader inputs ---------------------------------------------------------

pub const MAX_LIGHTS: usize = 5;

/// Material, lighting and environment inputs of the viewport shader; mirrors
/// `Look` in sculpt_pbr.wgsl. Every member is four floats wide so the two
/// layouts cannot drift apart through padding.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct LookUniform {
    /// World direction toward each light.
    pub light_dir: [[f32; 4]; MAX_LIGHTS],
    pub light_color: [[f32; 4]; MAX_LIGHTS],
    /// x count, y shadows on, z shadow strength, w time in seconds
    pub light_params: [f32; 4],
    pub shadow_view_proj: [[f32; 4]; 4],
    /// x one shadow texel in uv, y depth bias, z normal bias (world units),
    /// w 1 while the shadow changes frame after frame (the renderer sets it:
    /// its soft edge then takes half the taps)
    pub shadow_params: [f32; 4],
    /// x the map's width in world units (world per uv), y world units per
    /// unit of map depth, z tangent of the key light's angular radius (how
    /// fast its shadow softens with distance), w the part of the texture the
    /// map fills (1; the renderer makes it less while it draws the map small)
    pub shadow_soft: [f32; 4],
    /// Ambient light as nine spherical harmonic terms, ready to evaluate.
    pub sh: [[f32; 4]; 9],
    pub ambient_sky: [f32; 4],
    pub ambient_equator: [f32; 4],
    pub ambient_ground: [f32; 4],
    /// x ambient from sh (1) or sky/equator/ground (0), y reflection
    /// strength, z ambient occlusion on, w its strength on direct light
    pub env_params: [f32; 4],
    /// x orthographic half height (0 = perspective), y how far back an
    /// orthographic camera is parked (for depth of field; the surface shader
    /// does not read it)
    pub cam_params: [f32; 4],
    /// x backdrop (0 flat, 1 gradient, 2 HDRI), y HDRI turn in radians,
    /// z HDRI exposure, w gradient bias
    pub bg_params: [f32; 4],
    /// z normal detail strength, w normal detail scale
    pub base_params: [f32; 4],
    /// x intensity, y tint by base colour, z finish (0 none, 1 plastic,
    /// 2 metal, 3 clay, 4 wood; the renderer picks the finish's pipeline by
    /// it), w back-face depth buffer is valid
    pub matcap_params: [f32; 4],
    pub mask_tint: [f32; 4],
    /// The finish's own settings (see [`FinishParams`]), and the glow's.
    pub finish: FinishParams,
    pub glow: FinishParams,
    /// The floor: x 1 when it shows, y its height, z how far it reaches from
    /// its middle (it has faded out there), w how dark shadows on it get
    pub floor: [f32; 4],
    /// x z its middle, z how strongly it reflects the model, w how blurred
    pub floor_more: [f32; 4],
    /// Each light as a softbox (render/ltc.rs): its middle, w 1 for a
    /// softbox (0 for a plain direction); half its width as a vector, w pi
    /// over its solid angle from the subject (the radiance that gives its
    /// intensity there); half its height, w its distance from the subject
    /// squared (its diffuse light falls off from there).
    pub light_pos: [[f32; 4]; MAX_LIGHTS],
    pub light_side: [[f32; 4]; MAX_LIGHTS],
    pub light_up: [[f32; 4]; MAX_LIGHTS],
    /// x how far the contact shadows march toward each light, as a fraction
    /// of the view's height at the surface's depth (0 for none; the renderer
    /// clears it while the picture moves)
    pub contact: [f32; 4],
    /// Ray traced (render/ray.rs): x 1 when the shadows and the light from
    /// around come from the rays, y 1 when the view through resin does too.
    /// The renderer sets it.
    pub rays: [f32; 4],
}

/// A finish's settings as its shader function reads them: sixteen groups of
/// four numbers, laid out by each finish its own way (the `pack_` functions
/// below, which the `finish_` functions in sculpt_pbr.wgsl mirror). A saved
/// material can be a finish and one of these.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq, Debug)]
pub struct FinishParams {
    pub v: [[f32; 4]; 16],
}

impl FinishParams {
    /// Settings made of `slots`, the rest zero. `N` is the finish's slot
    /// count (`PLASTIC_SLOTS` and the others), so a packer that fills more
    /// or fewer than its finish reads does not compile.
    fn of<const N: usize>(slots: [[f32; 4]; N]) -> FinishParams {
        let mut out = FinishParams::zeroed();
        out.v[..N].copy_from_slice(&slots);
        out
    }
}

/// How many of the sixteen slots each finish uses (the shader test checks
/// its functions read no further).
pub const PBR_SLOTS: usize = 1;
pub const PLASTIC_SLOTS: usize = 9;
pub const METAL_SLOTS: usize = 10;
pub const CLAY_SLOTS: usize = 6;
pub const WOOD_SLOTS: usize = 12;
pub const GLOW_SLOTS: usize = 5;
pub const RESIN_SLOTS: usize = 2;

/// What the uniform builder needs to know about the frame.
pub struct Frame {
    pub cam_right: Vec3,
    pub cam_up: Vec3,
    pub cam_forward: Vec3,
    pub ortho_half_height: Option<f32>,
    /// How far behind its perspective position the camera sits. An
    /// orthographic view parks it well back from the scene; depth of field
    /// takes that off, so the focus distance means the same in both views.
    pub eye_offset: f32,
    /// World bounds of everything shown, for the shadow map.
    pub scene_min: Vec3,
    pub scene_max: Vec3,
    /// Largest extent of any one object, for finishes that were never measured.
    pub model_size: f32,
    pub time: f32,
    pub gradient_background: bool,
    /// The model's own smoothness (the plain finish's gloss).
    pub smoothness: f32,
    /// Side of the shadow map the renderer draws (render/quality.rs).
    pub shadow_size: u32,
}

/// How far the softboxes stand from the middle of the scene, in its radii:
/// a studio's distance, near enough that a highlight moves across a form.
const SOFTBOX_DISTANCE: f32 = 3.0;

/// The softbox light `i` of a preset is, in degrees across and up as seen
/// from the subject: the key as the preset's light size, the fill a bank
/// twice that, the rim and kicker tall strips, the top a wide overhead.
fn softbox(preset: &LightingPreset, i: usize) -> (f32, f32) {
    let s = preset.light_size;
    match i {
        0 => (s, s),
        1 => (s * 2.0, s * 2.0),
        2 | 3 => (s * 0.5, s * 2.5),
        _ => (s * 1.5, s * 1.5),
    }
}

/// The key light's shadow map, fitted to the scene: the box of everything
/// shown, seen from the light, inside the map's square with a little room
/// at the edges for the soft filter, and in depth inside the map's middle
/// half (casters are pushed away from the light a little, which must not
/// push them out of it).
pub struct ShadowFit {
    /// The light's axes: across, up, and toward the light.
    x: Vec3,
    y: Vec3,
    key: Vec3,
    /// The middle of the map, along `x` and `y`.
    centre: [f32; 2],
    /// Along `key`, the side of the box nearest the light and how deep it is.
    near: f32,
    /// The map's side, in world units.
    pub side: f32,
    /// World units per unit of map depth.
    pub depth: f32,
}

impl ShadowFit {
    /// With `floor` (its height, under the box) the map also takes in where
    /// the box's shadow falls on the floor, so the shadow is not cut off at
    /// the map's edge however low the light.
    pub fn around(key: Vec3, min: Vec3, max: Vec3, floor: Option<f32>) -> ShadowFit {
        let (min, max) =
            if min.is_finite() && max.is_finite() && min.cmple(max).all() { (min, max) } else { (Vec3::splat(-0.5), Vec3::splat(0.5)) };
        let up = if key.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let x = up.cross(key).normalize();
        let y = key.cross(x);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut take = |p: Vec3| {
            let q = Vec3::new(p.dot(x), p.dot(y), p.dot(key));
            lo = lo.min(q);
            hi = hi.max(q);
        };
        // The longest a shadow on the floor is followed: a light near the
        // horizon would otherwise stretch the map over a great deal of floor.
        let longest = (max - min).max_element().max(1e-3) * 3.0;
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            take(p);
            if let Some(height) = floor.filter(|h| h.is_finite() && key.y > 0.05) {
                let along = ((p.y - height) / key.y).min(longest);
                take(p - key * along);
            }
        }
        let extent = (hi - lo).max(Vec3::splat(1e-4));
        // A twentieth more on every side: the widest soft edge stays on the map.
        let side = extent.x.max(extent.y) * 1.1;
        ShadowFit {
            x,
            y,
            key,
            centre: [(lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5],
            near: hi.z,
            side,
            // The box's depth fills half the map's.
            depth: extent.z * 2.0,
        }
    }

    /// World to the map: x and y -1..1 across it, depth 0..1 away from the
    /// light with the box between a quarter and three quarters.
    pub fn view_proj(&self) -> Mat4 {
        let half = self.side * 0.5;
        let rows = [
            (self.x / half).extend(-self.centre[0] / half),
            (self.y / half).extend(-self.centre[1] / half),
            (-self.key / self.depth).extend(0.25 + self.near / self.depth),
            glam::Vec4::W,
        ];
        Mat4::from_cols(rows[0], rows[1], rows[2], rows[3]).transpose()
    }
}

const fn idx(i: usize, len: usize) -> usize {
    if i < len { i } else { len - 1 }
}

fn clamp01(x: f32) -> f32 {
    x.clamp(0.0, 1.0)
}

fn shift_hue(c: Rgb, shift: f32) -> Rgb {
    if shift <= 0.0 || shift >= 1.0 {
        return c;
    }
    let (r, g, b) = (c[0], c[1], c[2]);
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let d = max - min;
    let mut h = if d <= 1e-6 {
        0.0
    } else if max == r {
        ((g - b) / d).rem_euclid(6.0)
    } else if max == g {
        (b - r) / d + 2.0
    } else {
        (r - g) / d + 4.0
    } / 6.0;
    let s = if max <= 1e-6 { 0.0 } else { d / max };
    h = (h + shift).rem_euclid(1.0);
    let f = |n: f32| {
        let k = (n + h * 6.0).rem_euclid(6.0);
        max - max * s * k.min(4.0 - k).clamp(0.0, 1.0)
    };
    [f(5.0), f(3.0), f(1.0)]
}

impl Look {
    pub fn preset(&self) -> &'static LightingPreset {
        &LIGHTING[idx(self.lighting, LIGHTING.len())]
    }

    /// The axes lights are placed in: the camera's when locked, a level
    /// world frame when not, turned by `rotation` either way.
    fn rig_frame(&self, f: &Frame) -> (Vec3, Vec3, Vec3) {
        let (right, up, forward) = if self.follow_camera {
            (f.cam_right, f.cam_up, f.cam_forward)
        } else {
            let forward = self.world_forward;
            (forward.cross(Vec3::Y).normalize_or(Vec3::X), Vec3::Y, forward)
        };
        // A turn about the rig's own vertical: forward swings toward right.
        let (s, c) = self.rotation.to_radians().sin_cos();
        (right * c - forward * s, up, forward * c + right * s)
    }

    /// World direction toward light `i` of the current preset.
    fn light_source(&self, i: usize, f: &Frame) -> Vec3 {
        let spec = &self.preset().lights[i];
        let (right, up, forward) = self.rig_frame(f);
        let (se, ce) = spec.elevation.to_radians().sin_cos();
        let (sa, ca) = spec.azimuth.to_radians().sin_cos();
        (right * (ce * sa) + up * se - forward * (ce * ca)).normalize()
    }

    /// The sun of the default sky the model reflects when no HDRI is on: the
    /// key light as it falls in a level front view.
    pub fn sky_sun(&self) -> (Vec3, Rgb) {
        let spec = &self.preset().lights[0];
        let (se, ce) = spec.elevation.to_radians().sin_cos();
        let (sa, ca) = spec.azimuth.to_radians().sin_cos();
        let c = linear(spec.color);
        let k = spec.intensity * self.brightness.clamp(0.0, MAX_BRIGHTNESS);
        (Vec3::new(ce * sa, se, ce * ca).normalize(), [c[0] * k, c[1] * k, c[2] * k])
    }

    pub fn uniform(&self, f: &Frame) -> LookUniform {
        let mut u = LookUniform::zeroed();
        let preset = self.preset();
        let brightness = self.brightness.clamp(0.0, MAX_BRIGHTNESS);

        // Lights. All directional, so a preset looks the same on any sculpt
        // at any scale. The key is the one that casts shadows.
        let count = if self.five_point { 5 } else { 3 };
        for i in 0..count {
            let spec = &preset.lights[i];
            let d = self.light_source(i, f);
            let c = linear(spec.color);
            let k = spec.intensity * brightness;
            u.light_dir[i] = [d.x, d.y, d.z, 0.0];
            u.light_color[i] = [c[0] * k, c[1] * k, c[2] * k, 0.0];
        }
        // Softboxes: each light a rectangle facing the middle of the scene
        // from its direction, studio-far, as big as the preset says.
        if self.softboxes {
            let (min, max) = (f.scene_min, f.scene_max);
            let (centre, radius) = if min.is_finite() && max.is_finite() {
                ((min + max) * 0.5, ((max - min).length() * 0.5).max(1e-3))
            } else {
                (Vec3::ZERO, 1.0)
            };
            let distance = radius * SOFTBOX_DISTANCE;
            let size = self.shadow_softness.clamp(0.0, 4.0);
            for i in 0..count {
                let (across, high) = softbox(preset, i);
                let (half_w, half_h) =
                    ((across * size).to_radians() * 0.5, (high * size).to_radians() * 0.5);
                if half_w.min(half_h) < 0.002 {
                    // Too small to be anything but a point: a direction.
                    continue;
                }
                let d = Vec3::from_slice(&u.light_dir[i][..3]);
                let right = Vec3::Y.cross(d).normalize_or(Vec3::X);
                let up = d.cross(right);
                let (a, b) = (distance * half_w.tan(), distance * half_h.tan());
                // The solid angle of the rectangle from its middle's axis.
                let solid = 4.0 * (a * b / ((a * a + distance * distance) * (b * b + distance * distance)).sqrt()).asin();
                u.light_pos[i] = (centre + d * distance).extend(1.0).to_array();
                u.light_side[i] = (right * a).extend(std::f32::consts::PI / solid.max(1e-9)).to_array();
                u.light_up[i] = (up * b).extend(distance * distance).to_array();
            }
        }
        u.light_params = [
            count as f32,
            if self.shadows { 1.0 } else { 0.0 },
            preset.shadow_strength,
            f.time,
        ];

        // One shadow map fitted around everything shown.
        let key = self.light_source(0, f);
        let floor = self.floor.on.then_some(f.scene_min.y);
        let fit = ShadowFit::around(key, f.scene_min, f.scene_max, floor);
        u.shadow_view_proj = fit.view_proj().to_cols_array_2d();
        // The Unity pipeline's biases (depth 0.1, normal 0.5 texels), widened
        // for the soft filter's reach as it does.
        let size = f.shadow_size.max(1) as f32;
        let texel = fit.side / size;
        u.shadow_params = [1.0 / size, 0.1 * texel * 2.5, 0.5 * texel * 2.5, 0.0];
        let softness = self.shadow_softness.clamp(0.0, 4.0);
        let light_tan = (preset.light_size.to_radians() * 0.5 * softness).tan();
        u.shadow_soft = [fit.side, fit.depth, light_tan, 1.0];

        // Ambient and reflection: the HDRI's when one is on, otherwise the
        // preset's sky / equator / ground, scaled in linear space so
        // Brightness acts on it as it does on the lights.
        let amb = |c: Rgb| {
            let c = linear(c);
            [c[0] * brightness, c[1] * brightness, c[2] * brightness, 0.0]
        };
        u.ambient_sky = amb(preset.ambient_sky);
        u.ambient_equator = amb(preset.ambient_equator);
        u.ambient_ground = amb(preset.ambient_ground);
        let hdri = self.hdri.active();
        u.env_params = [
            if hdri { 1.0 } else { 0.0 },
            if hdri { clamp01(self.hdri.reflection) } else { preset.reflection },
            0.0,
            0.25,
        ];
        u.cam_params = [f.ortho_half_height.unwrap_or(0.0), f.eye_offset, 0.0, 0.0];
        let backdrop = if hdri && self.hdri_background {
            2.0
        } else if f.gradient_background || self.hdri_background {
            // HDRI asked for with none loaded falls back to the gradient.
            1.0
        } else {
            0.0
        };
        u.bg_params = [
            backdrop,
            self.hdri.rotation.to_radians(),
            self.hdri.exposure.clamp(0.0, 4.0),
            self.gradient_bias.clamp(0.2, 4.0),
        ];

        u.base_params = [0.0, 0.0, self.normal_strength.clamp(0.0, 2.0), self.normal_scale.clamp(1.0, 300.0)];
        let finish = match self.finish {
            Finish::None => 0.0,
            Finish::LurePlastic => 1.0,
            Finish::Metal => 2.0,
            Finish::Clay => 3.0,
            Finish::Wood => 4.0,
            // 5 is the matcap's pipeline.
            Finish::Resin => 6.0,
        };
        u.matcap_params = [self.matcap_intensity.clamp(0.0, 3.0), clamp01(self.matcap_tint), finish, 0.0];
        // Masked areas go toward dark grey rather than a colour, as in
        // ZBrush and Blender, so the mask does not read as paint.
        u.mask_tint = rgba([0.1, 0.1, 0.1], 0.6);

        if self.floor.on && f.scene_min.is_finite() && f.scene_max.is_finite() {
            let (min, max) = (f.scene_min, f.scene_max);
            let reach = (max.x - min.x).max(max.z - min.z).max(max.y - min.y).max(1e-3) * 1.6;
            let floor = &self.floor;
            u.floor = [1.0, min.y, reach, clamp01(floor.shadow)];
            u.floor_more = [(min.x + max.x) * 0.5, (min.z + max.z) * 0.5, clamp01(floor.reflection), clamp01(floor.roughness)];
        }
        // Contact shadows march a sixtieth of the view's height.
        if self.contact_shadows {
            u.contact[0] = 1.0 / 60.0;
        }
        u.finish = self.finish_params(self.finish, f);
        if self.glow.on {
            u.glow = self.pack_glow(self.size_of(self.glow.reference, f));
        }
        u
    }

    /// The size a finish's pattern is drawn at: the one it was frozen
    /// against, or the model's now if it never was.
    fn size_of(&self, reference: f32, f: &Frame) -> f32 {
        if reference > 0.0 { reference } else { f.model_size }
    }

    /// `finish`'s settings as its shader function reads them.
    pub fn finish_params(&self, finish: Finish, f: &Frame) -> FinishParams {
        match finish {
            Finish::None => self.pack_pbr(f.smoothness),
            Finish::LurePlastic => self.pack_plastic(self.size_of(self.lure.reference, f)),
            Finish::Metal => self.pack_metal(self.size_of(self.metal.reference, f)),
            Finish::Clay => self.pack_clay(self.size_of(self.clay.reference, f)),
            Finish::Wood => self.pack_wood(self.size_of(self.wood.reference, f)),
            Finish::Resin => self.pack_resin(self.size_of(self.resin.reference, f)),
        }
    }

    /// The plain finish: v[0] x metallic, y smoothness (see `finish_pbr`).
    pub fn pack_pbr(&self, smoothness: f32) -> FinishParams {
        FinishParams::of::<PBR_SLOTS>([[clamp01(self.metallic), smoothness, 0.0, 0.0]])
    }

    /// The lure plastic (see `finish_plastic`), its pattern at `size`.
    pub fn pack_plastic(&self, size: f32) -> FinishParams {
        let s = &self.lure;
        let p = &LURES[idx(s.preset, LURES.len())];
        let flake = |c: [f32; 4]| rgba([c[0], c[1], c[2]], c[3]);
        FinishParams::of::<PLASTIC_SLOTS>([
            rgba(p.thin, 1.0),
            rgba(p.thick, 1.0),
            // Translucency drives both how deep light gets and how much of it
            // makes it back out: one "how see-through" dial.
            [p.depth * size * s.translucency, p.transmission * s.translucency, clamp01(s.gloss), 0.0],
            flake(p.flakes[0]),
            flake(p.flakes[1]),
            flake(p.flakes[2]),
            rgba(p.micro, 1.0),
            // A cell holds one flake averaging 0.67 of the cell's width.
            [
                p.flake_size * size * s.flake_size / 0.67,
                clamp01(p.flake_density * s.flake_amount),
                p.flake_shape,
                FLAKE_TILT,
            ],
            [s.sparkle, p.micro_size * size * s.flake_size / 0.67, clamp01(p.micro_density * s.flake_amount), 0.0],
        ])
    }

    /// The aged metal (see `finish_metal`), its pattern at `size`.
    pub fn pack_metal(&self, size: f32) -> FinishParams {
        // Coverage shift at each end of the Exposed Metal slider: enough to
        // take any preset from almost fully coated to mostly bare.
        const EXPOSURE_REACH: f32 = 0.6;
        // How far the seed slider walks through the patch noise, in patch
        // widths, along a direction off the noise lattice's rows.
        const PATTERN_PATH_LENGTH: f32 = 40.0;
        let s = &self.metal;
        let p = &METALS[idx(s.preset, METALS.len())];
        let path = Vec3::new(0.53, 0.29, 0.80).normalize() * (clamp01(s.seed) * PATTERN_PATH_LENGTH);
        FinishParams::of::<METAL_SLOTS>([
            rgba(p.metal, 1.0),
            rgba(p.coat_a, 1.0),
            rgba(p.coat_b, 1.0),
            rgba(p.wash_color, 1.0),
            [path.x, path.y, path.z, 0.0],
            [
                clamp01(p.metal_smoothness * s.gloss),
                p.metal_metallic,
                p.coat_metallic,
                clamp01(p.coat_smoothness * s.gloss),
            ],
            [
                clamp01(p.wash * s.wash),
                p.coverage - (s.exposure - 1.0) * EXPOSURE_REACH,
                p.edge_wear * s.edge_wear,
                p.patch_size * size * s.pattern_size,
            ],
            [p.patch_contrast, p.patchiness, p.follows_recesses, p.grain_size * size * s.pattern_size],
            [p.grain, s.detail, p.patch_breakup, p.sparkle],
            [p.undulation, p.undulation_size * size * s.pattern_size, 0.0, 0.0],
        ])
    }

    /// The sculptor's clay (see `finish_clay`), its pattern at `size`.
    pub fn pack_clay(&self, size: f32) -> FinishParams {
        let s = &self.clay;
        let p = &CLAYS[idx(s.preset, CLAYS.len())];
        FinishParams::of::<CLAY_SLOTS>([
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
        ])
    }

    /// The carved wood (see `finish_wood`), its pattern at `size`.
    pub fn pack_wood(&self, size: f32) -> FinishParams {
        // The pith's walk, in model sizes: from a little off centre (dead
        // centre rings the model with bullseyes) out past its edge, where the
        // rings flatten into the stripes of quarter-sawn wood.
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
        FinishParams::of::<WOOD_SLOTS>([
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
            [
                // Sheen takes the wax with it: a dry wood has no wax highlight either.
                p.wax * s.sheen,
                p.wax_sharpness,
                p.subsurface,
                // Additive, so the slider can add cracks to a preset that has none.
                clamp01(p.checks + (s.cracks - 1.0) * 0.5),
            ],
            [s.detail, 0.0, 0.0, 0.0],
        ])
    }

    /// The cast resin (see `finish_resin`), its depths at `size`.
    pub fn pack_resin(&self, size: f32) -> FinishParams {
        let s = &self.resin;
        let p = &RESINS[idx(s.preset, RESINS.len())];
        FinishParams::of::<RESIN_SLOTS>([
            rgba(p.color, p.depth * size * s.clarity.clamp(0.05, 10.0)),
            [clamp01(p.haze * s.haze), clamp01(p.smoothness * s.gloss), p.transmission, 0.0],
        ])
    }

    /// The glow (see `finish_glow`), its pattern at `size`. v[2].x is 1: on.
    pub fn pack_glow(&self, size: f32) -> FinishParams {
        let s = &self.glow;
        let p = &GLOWS[idx(s.preset, GLOWS.len())];
        FinishParams::of::<GLOW_SLOTS>([
            rgba(shift_hue(p.hot, s.hue_shift), 1.0),
            rgba(shift_hue(p.cool, s.hue_shift), 1.0),
            // Additive around the preset (1 = the preset), so a slider at 2
            // gives a rim, a pulse or a dim to presets that have none.
            [1.0, p.pattern as f32, p.intensity * s.intensity, (p.coverage + (s.coverage - 1.0) * 0.5).clamp(0.0, 1.5)],
            [
                (p.rim + (s.rim - 1.0) * 0.5).max(0.0),
                p.pattern_size * size * s.pattern_size,
                p.speed * s.speed,
                clamp01(p.pulse + (s.pulse - 1.0) * 0.5),
            ],
            [p.pulse_rate, clamp01(p.dim + (s.dim - 1.0) * 0.5), 0.0, 0.0],
        ])
    }
}

// ---- HDRI ------------------------------------------------------------------

/// An equirectangular environment picture in linear colour.
pub struct HdriImage {
    pub width: u32,
    pub height: u32,
    /// RGBA, row by row from the top.
    pub pixels: Vec<f32>,
}

/// Loads an environment picture: Radiance .hdr, OpenEXR, or a PNG / JPG
/// (which has no values above white, so it lights flatly).
pub fn load_hdri(path: &std::path::Path) -> Result<HdriImage, String> {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "hdr" | "exr" | "png" | "jpg" | "jpeg") {
        return Err(format!("Unsupported format .{ext}."));
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(|e| format!("Could not read file: {e}"))?
        .with_guessed_format()
        .map_err(|e| format!("Could not read file: {e}"))?;
    // The decoder's own limit (512 MB) refuses the 16K pictures HDRI sites
    // offer, which are shrunk to MAX_WIDTH straight after anyway.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(8 << 30);
    reader.limits(limits);
    let image = reader.decode().map_err(|e| format!("Could not read file: {e}"))?;
    // Bigger than this gains nothing on screen and costs a great deal of memory.
    const MAX_WIDTH: u32 = 8192;
    let image = if image.width() > MAX_WIDTH {
        image.resize(MAX_WIDTH, MAX_WIDTH / 2, image::imageops::FilterType::Triangle)
    } else {
        image
    };
    let hdr = matches!(ext.as_str(), "hdr" | "exr");
    let rgba = image.into_rgba32f();
    let (width, height) = rgba.dimensions();
    let mut pixels = rgba.into_raw();
    for px in pixels.chunks_exact_mut(4) {
        for c in &mut px[..3] {
            // Pictures are sRGB-encoded; .hdr and .exr are linear already.
            let v = if hdr { *c } else { lin1(*c) };
            // Half floats top out near 65504; a stray NaN would poison every blur.
            *c = if v.is_finite() { v.clamp(0.0, 65000.0) } else { 0.0 };
        }
        px[3] = 1.0;
    }
    if width < 2 || height < 2 {
        return Err("The picture is too small.".into());
    }
    Ok(HdriImage { width, height, pixels })
}

// ---- helpers for the app ---------------------------------------------------

/// The size finishes are measured against: the largest extent of any one
/// shown object. An empty scene counts as the startup sphere.
pub fn model_size(scene: &crate::scene::Scene, slot: &crate::mesh::Mesh) -> f32 {
    let size = scene
        .objects
        .iter()
        .filter(|o| o.visible)
        .map(|o| scene.bounds_of(o, slot))
        .filter(|b| !b.is_empty())
        .map(|b| (b.max - b.min).max_element())
        .fold(0.0f32, f32::max);
    if size > 1e-4 { size } else { 1.0 }
}

/// Applies shading settings written as `key=value,key=value` (the `--look`
/// option, for visual checks). Returns an HDRI to load, if one was named.
///
/// Keys: `matcap`, `panels`, `gradient`, `flat`, `five`, `follow`, `shadows`, `ao`, `contact`,
/// `bloom`, `vignette`, `hdribg`, `dof` (on / off); `finish` (none, lure, metal,
/// clay, wood, optionally `:preset`); `glow` (off or a preset); `light` (a
/// preset); `tonemap` (none, neutral, aces, pbr, agx); `brightness`, `rotation`, `exposure`,
/// `metallic`, `smoothness`, `normal`, `focus`, `aperture`, `softness`
/// (numbers); `shadowmap`, `reflections` (the shadow map's and the
/// reflection cube's sides); `refine` (on / off) and `samples` (how many);
/// `floor` (on / off) with `floorshadow`, `floorreflect`, `floorrough` (numbers);
/// `softboxes` (on / off);
/// `backdrop` (a grey, 0 black to 1 white); `grain`, `sharpen` (0 to 1);
/// `hdri` (a file).
pub fn apply_spec(opts: &mut crate::doc::Options, spec: &str, size: f32) -> Option<PathBuf> {
    let mut hdri = None;
    for item in spec.split(',') {
        let (key, value) = item.split_once('=').unwrap_or((item, "on"));
        let (key, value) = (key.trim(), value.trim());
        let on = !matches!(value, "off" | "0" | "false" | "no");
        let number = value.parse::<f32>().ok();
        let look = &mut opts.look;
        match key {
            "matcap" => opts.matcap_on = on,
            "panels" => opts.clean_view = !on,
            "gradient" => opts.gradient_background = on,
            "backdrop" => opts.background = [number.unwrap_or(0.1).clamp(0.0, 1.0); 3],
            "flat" => opts.flat_shading = on,
            "backfaces" => opts.show_backfaces = on,
            "wire" => opts.wireframe = on,
            "five" => look.five_point = on,
            "follow" => look.follow_camera = on,
            "shadows" => look.shadows = on,
            "softness" => look.shadow_softness = number.unwrap_or(1.0),
            "softboxes" => look.softboxes = on,
            "shadowmap" => opts.quality.shadow_map = number.map_or(2048, |n| n as u32),
            "floor" => look.floor.on = on,
            "floorshadow" => look.floor.shadow = number.unwrap_or(0.85),
            "floorreflect" => look.floor.reflection = number.unwrap_or(0.0),
            "floorrough" => look.floor.roughness = number.unwrap_or(0.3),
            "reflections" => opts.quality.reflections = number.map_or(256, |n| n as u32),
            "refine" => opts.quality.refine = on,
            "samples" => opts.quality.samples = number.map_or(64, |n| n as u32),
            "ao" => look.ambient_occlusion = on,
            "contact" => look.contact_shadows = on,
            "bloom" => look.bloom = on,
            "vignette" => look.vignette = on,
            "hdribg" => look.hdri_background = on,
            "dof" => look.depth_of_field = on,
            "focus" => look.focus_distance = number.unwrap_or(10.0),
            "aperture" => look.aperture = number.unwrap_or(5.6),
            "brightness" => look.brightness = number.unwrap_or(1.0),
            "exposure" => look.exposure = number.unwrap_or(0.0),
            "grain" => look.film_grain = number.unwrap_or(0.0),
            "sharpen" => look.sharpen = number.unwrap_or(0.0),
            "rotation" => look.rotation = number.unwrap_or(0.0),
            "metallic" => look.metallic = number.unwrap_or(0.0),
            "smoothness" => opts.smoothness = number.unwrap_or(0.4),
            "normal" => look.normal_strength = number.unwrap_or(0.0),
            "hdri" => hdri = Some(PathBuf::from(value)),
            "tonemap" => {
                look.tonemap = match value {
                    "none" => Tonemap::None,
                    "aces" => Tonemap::Aces,
                    "pbr" => Tonemap::PbrNeutral,
                    "agx" => Tonemap::Agx,
                    _ => Tonemap::Neutral,
                }
            }
            "light" => {
                if let Some(i) = LIGHTING.iter().position(|p| p.id == value) {
                    look.lighting = i;
                }
            }
            "glow" => match GLOWS.iter().position(|p| p.id == value) {
                Some(i) => look.pick_glow(i, size),
                None => look.glow.on = false,
            },
            "finish" => {
                let (kind, preset) = value.split_once(':').unwrap_or((value, ""));
                let (finish, found) = match kind {
                    "lure" => (Finish::LurePlastic, LURES.iter().position(|p| p.id == preset)),
                    "metal" => (Finish::Metal, METALS.iter().position(|p| p.id == preset)),
                    "clay" => (Finish::Clay, CLAYS.iter().position(|p| p.id == preset)),
                    "wood" => (Finish::Wood, WOODS.iter().position(|p| p.id == preset)),
                    "resin" => (Finish::Resin, RESINS.iter().position(|p| p.id == preset)),
                    _ => (Finish::None, None),
                };
                match found {
                    Some(i) => look.pick_preset(finish, i, size),
                    None => look.set_finish(finish, size),
                }
            }
            other => eprintln!("unknown look setting {other}"),
        }
    }
    hdri
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shadow_map_holds_the_whole_scene_and_little_else() {
        let (min, max) = (Vec3::new(-1.0, 0.0, -0.5), Vec3::new(2.0, 0.5, 0.5));
        for key in [Vec3::Y, Vec3::new(-0.49, 0.64, 0.59).normalize(), Vec3::new(0.3, -0.2, -0.93).normalize()] {
            let fit = ShadowFit::around(key, min, max, None);
            let m = fit.view_proj();
            let (mut lo, mut hi) = (Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY));
            for i in 0..8 {
                let p = Vec3::new(
                    if i & 1 == 0 { min.x } else { max.x },
                    if i & 2 == 0 { min.y } else { max.y },
                    if i & 4 == 0 { min.z } else { max.z },
                );
                let q = m.project_point3(p);
                lo = lo.min(q);
                hi = hi.max(q);
            }
            // Every corner on the map, inside the room left for soft edges...
            assert!(lo.x >= -1.0 && hi.x <= 1.0 && lo.y >= -1.0 && hi.y <= 1.0, "{key}: {lo} {hi}");
            // ...the longer side filling it bar that room...
            let filled = (hi.x - lo.x).max(hi.y - lo.y) / 2.0;
            assert!((filled - 1.0 / 1.1).abs() < 1e-4, "{key}: {filled}");
            // ...and in depth the middle half, nearest the light first.
            assert!((lo.z - 0.25).abs() < 1e-4 && (hi.z - 0.75).abs() < 1e-4, "{key}: {lo} {hi}");
            let toward = m.project_point3((min + max) * 0.5 + key);
            assert!(toward.z < 0.5, "nearer the light is smaller depth");
        }
        // Nothing to fit, or bounds that are not numbers: a unit box instead.
        let fit = ShadowFit::around(Vec3::Y, Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY), None);
        assert!(fit.side.is_finite() && fit.side > 0.0 && fit.depth > 0.0);
    }

    #[test]
    fn with_a_floor_the_shadow_map_takes_in_where_the_shadow_lands() {
        let (min, max) = (Vec3::new(-0.5, 0.0, -0.5), Vec3::new(0.5, 1.0, 0.5));
        // A low light from the side: the shadow runs a long way over the floor.
        let key = Vec3::new(1.0, 0.4, 0.0).normalize();
        let bare = ShadowFit::around(key, min, max, None);
        let floored = ShadowFit::around(key, min, max, Some(0.0));
        // Along the light a shadow stays where its caster is across the map;
        // what the floor needs is depth, out to where the shadow lands.
        assert!((floored.side - bare.side).abs() < 1e-5);
        assert!(floored.depth > bare.depth * 1.5, "{} {}", floored.depth, bare.depth);
        // The tip of the shadow of the top far corner is on the map, and
        // within its depth (beyond it would read as lit).
        let top = Vec3::new(0.5, 1.0, 0.5);
        let tip = top - key * (top.y / key.y);
        let q = floored.view_proj().project_point3(tip);
        assert!(q.x.abs() <= 1.0 && q.y.abs() <= 1.0 && q.z > 0.0 && q.z < 1.0, "{q}");
        assert!(bare.view_proj().project_point3(tip).z > 1.0, "without the floor it was beyond the map");
        // A light below the floor casts nothing onto it: no change.
        let under = Vec3::new(0.3, -0.5, 0.2).normalize();
        assert_eq!(ShadowFit::around(under, min, max, Some(0.0)).depth, ShadowFit::around(under, min, max, None).depth);
    }

    #[test]
    fn the_floor_lies_under_the_lowest_point_and_is_off_in_a_file_from_before() {
        let mut look: Look = crate::io::read_over(&Look::default(), &serde_json::json!({ "shadows": true })).unwrap();
        assert!(!look.floor.on);
        let frame = Frame {
            cam_right: Vec3::X,
            cam_up: Vec3::Y,
            cam_forward: Vec3::NEG_Z,
            ortho_half_height: None,
            eye_offset: 0.0,
            scene_min: Vec3::new(-1.0, -0.25, -2.0),
            scene_max: Vec3::new(3.0, 1.0, 2.0),
            model_size: 4.0,
            time: 0.0,
            gradient_background: false,
            smoothness: 0.4,
            shadow_size: 2048,
        };
        assert_eq!(look.uniform(&frame).floor[0], 0.0, "off: the shader draws none");
        look.floor.on = true;
        let u = look.uniform(&frame);
        assert_eq!(u.floor[0], 1.0);
        assert_eq!(u.floor[1], -0.25, "under the lowest point");
        assert_eq!([u.floor_more[0], u.floor_more[1]], [1.0, 0.0], "centred under the scene");
        assert!(u.floor[2] >= 4.0 * 1.5, "reaching well past the scene before it fades out");
    }

    #[test]
    fn a_softbox_lights_the_subject_as_the_light_it_stands_for() {
        let look = Look { five_point: true, ..Look::default() };
        let frame = Frame {
            cam_right: Vec3::X,
            cam_up: Vec3::Y,
            cam_forward: Vec3::NEG_Z,
            ortho_half_height: None,
            eye_offset: 0.0,
            scene_min: Vec3::splat(-1.0),
            scene_max: Vec3::splat(1.0),
            model_size: 2.0,
            time: 0.0,
            gradient_background: false,
            smoothness: 0.4,
            shadow_size: 2048,
        };
        let u = look.uniform(&frame);
        for i in 0..5 {
            assert_eq!(u.light_pos[i][3], 1.0, "light {i} is a softbox");
            let d = Vec3::from_slice(&u.light_dir[i][..3]);
            let c = Vec3::from_slice(&u.light_pos[i][..3]);
            let (side, up) = (Vec3::from_slice(&u.light_side[i][..3]), Vec3::from_slice(&u.light_up[i][..3]));
            let corners = [c - side - up, c + side - up, c + side + up, c - side + up];
            // At the middle of the scene, facing the light: what a direction
            // of the same intensity gives, nl = 1.
            let diffuse = crate::render::ltc_integral(d, d, Vec3::ZERO, glam::Mat3::IDENTITY, corners)
                * u.light_side[i][3]
                / std::f32::consts::TAU;
            assert!((diffuse - 1.0).abs() < 0.02, "light {i}: {diffuse}");
            // Turned 60 degrees from it: half, as the cosine has it.
            let away = (d.any_orthonormal_vector() * 3f32.sqrt() + d).normalize() * 1.0;
            let n = (d * 0.5 + d.cross(away).cross(d).normalize() * (3f32.sqrt() / 2.0)).normalize();
            let tilted = crate::render::ltc_integral(n, n, Vec3::ZERO, glam::Mat3::IDENTITY, corners)
                * u.light_side[i][3]
                / std::f32::consts::TAU;
            assert!((tilted - 0.5).abs() < 0.03, "light {i} at 60 degrees: {tilted}");
        }
    }

    #[test]
    fn softness_widens_the_key_light_and_zero_makes_it_a_point() {
        let mut look = Look::default();
        let frame = Frame {
            cam_right: Vec3::X,
            cam_up: Vec3::Y,
            cam_forward: Vec3::NEG_Z,
            ortho_half_height: None,
            eye_offset: 0.0,
            scene_min: Vec3::splat(-1.0),
            scene_max: Vec3::splat(1.0),
            model_size: 2.0,
            time: 0.0,
            gradient_background: false,
            smoothness: 0.4,
            shadow_size: 2048,
        };
        let usual = look.uniform(&frame).shadow_soft[2];
        let expected = (look.preset().light_size.to_radians() * 0.5).tan();
        assert!((usual - expected).abs() < 1e-6);
        look.shadow_softness = 2.0;
        assert!(look.uniform(&frame).shadow_soft[2] > usual * 1.9);
        look.shadow_softness = 0.0;
        assert_eq!(look.uniform(&frame).shadow_soft[2], 0.0);
        // A bigger map has finer texels.
        let fine = look.uniform(&frame).shadow_params;
        let coarse = look.uniform(&Frame { shadow_size: 512, ..frame }).shadow_params;
        assert_eq!(fine[0] * 4.0, coarse[0]);
        assert!(fine[1] < coarse[1] && fine[2] < coarse[2]);
    }

    #[test]
    fn new_scenes_get_pbr_neutral_and_saved_ones_keep_their_curve() {
        assert_eq!(Look::default().tonemap, Tonemap::PbrNeutral);
        assert_eq!(Look::default().exposure, 0.0);
        // A scene from before: its curve as it was, and no change of exposure.
        let saved = serde_json::json!({ "tonemap": "Neutral", "bloom": false });
        let look: Look = crate::io::read_over(&Look::default(), &saved).unwrap();
        assert_eq!((look.tonemap, look.exposure, look.bloom), (Tonemap::Neutral, 0.0, false));
        // Each curve has its own number in the shader and its own place in the card.
        let mut numbers: Vec<f32> = Tonemap::ALL.iter().map(|t| t.shader_index()).collect();
        numbers.sort_by(f32::total_cmp);
        assert_eq!(numbers, [0.0, 1.0, 2.0, 3.0, 4.0]);
    }

    #[test]
    fn a_scene_saved_before_soft_shadows_reads_as_the_presets_own() {
        let look: Look = crate::io::read_over(&Look::default(), &serde_json::json!({ "shadows": false })).unwrap();
        assert_eq!(look.shadow_softness, 1.0);
        assert!(!look.shadows);
    }
}
