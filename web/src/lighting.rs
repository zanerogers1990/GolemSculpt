//! The Lighting and Effects cards' settings: the desktop's lighting presets
//! (key, fill and rim, plus kicker and top for 5-point) and a reduced set of
//! its post effects (exposure, tone curve, colour, vignette, film grain).
//!
//! What the browser leaves out: softboxes, the HDRI environment, bloom,
//! depth of field and contact shadows. The key light's shadow and ambient
//! occlusion come with WebGPU only (shadows.rs); with WebGL2 Light Size only
//! widens the highlights.
//!
//! The preset table is copied from src/look.rs (`LIGHTING`), which cannot be
//! compiled here, so a preset changed there should be changed here too.

use glam::Vec3;
use serde::{Deserialize, Serialize};

use super::finish::{Rgb, linear};

/// One light of a preset: azimuth from the viewer (0 from the camera, +90
/// from screen right, 180 from behind), elevation above the horizon,
/// colour, intensity.
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

pub struct LightingPreset {
    pub name: &'static str,
    pub description: &'static str,
    pub lights: [PresetLight; 5],
    pub ambient_sky: Rgb,
    pub ambient_equator: Rgb,
    pub ambient_ground: Rgb,
    /// Strength of the reflected surroundings.
    pub reflection: f32,
    /// How wide the key light looks from the subject, in degrees.
    pub light_size: f32,
}

pub const LIGHTING: [LightingPreset; 8] = [
    LightingPreset {
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
        reflection: 0.6,
        light_size: 6.0,
    },
    LightingPreset {
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
        reflection: 0.4,
        light_size: 14.0,
    },
    LightingPreset {
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
        reflection: 0.8,
        light_size: 16.0,
    },
    LightingPreset {
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
        reflection: 0.7,
        light_size: 6.0,
    },
    LightingPreset {
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
        reflection: 0.5,
        light_size: 4.0,
    },
    LightingPreset {
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
        reflection: 0.6,
        light_size: 3.0,
    },
    LightingPreset {
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
        reflection: 0.25,
        light_size: 5.0,
    },
    LightingPreset {
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
        reflection: 0.2,
        light_size: 2.0,
    },
];

/// The Lighting card's settings (brightness is `Finishes::brightness`).
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Lighting {
    pub preset: usize,
    pub five_point: bool,
    /// Degrees the whole rig is turned about the view's up.
    pub rotation: f32,
    /// The key light stays at the upper left of the view; off, the lights
    /// stay where they were in the scene.
    pub follow_camera: bool,
    /// Times the preset's own light size (0 is hard points).
    pub light_size: f32,
    /// The key light casts a soft shadow (WebGPU only: shadows.rs).
    pub shadows: bool,
}

impl Default for Lighting {
    fn default() -> Self {
        Lighting { preset: 0, five_point: false, rotation: 0.0, follow_camera: true, light_size: 1.0, shadows: true }
    }
}

impl Lighting {
    pub fn preset(&self) -> &'static LightingPreset {
        &LIGHTING[self.preset.min(LIGHTING.len() - 1)]
    }
}

/// The tone curves, in the order the card shows them (the desktop's).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum Tonemap {
    #[default]
    PbrNeutral,
    Agx,
    Aces,
    Neutral,
    None,
}

impl Tonemap {
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

    /// The number the shader switches on (the desktop's).
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

/// The Effects card's settings.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Effects {
    /// Camera stops, -4..4.
    pub exposure: f32,
    pub tonemap: Tonemap,
    /// -100..100, 0 unchanged.
    pub saturation: f32,
    pub contrast: f32,
    pub vignette: bool,
    pub vignette_intensity: f32,
    pub vignette_smoothness: f32,
    /// 0..1.
    pub film_grain: f32,
    /// Soft shadows in creases and where forms meet (WebGPU only: shadows.rs).
    pub ambient_occlusion: bool,
}

impl Default for Effects {
    fn default() -> Self {
        Effects {
            exposure: 0.0,
            tonemap: Tonemap::PbrNeutral,
            saturation: 0.0,
            contrast: 0.0,
            vignette: false,
            vignette_intensity: 0.3,
            vignette_smoothness: 0.4,
            film_grain: 0.0,
            ambient_occlusion: true,
        }
    }
}

impl Effects {
    /// Exposure (times), the tone curve, film grain, and 0.
    pub fn tone(&self) -> [f32; 4] {
        [self.exposure.clamp(-4.0, 4.0).exp2(), self.tonemap.shader_index(), self.film_grain.clamp(0.0, 1.0), 0.0]
    }

    /// Saturation and contrast (1 unchanged), the vignette's strength (0
    /// none) and its smoothness, as the desktop's post shader reads them.
    pub fn grade(&self) -> [f32; 4] {
        [
            self.saturation.clamp(-100.0, 100.0) / 100.0 + 1.0,
            self.contrast.clamp(-100.0, 100.0) / 100.0 + 1.0,
            if self.vignette { self.vignette_intensity.clamp(0.0, 1.0) * 3.0 } else { 0.0 },
            self.vignette_smoothness.clamp(0.01, 1.0) * 5.0,
        ]
    }
}

/// The lights of `lighting` in world space for a view whose axes are
/// `right`, `up` and `forward`: each light's direction and its linear colour
/// times intensity and `brightness`, then the sky, equator and ground light.
pub fn rig(lighting: &Lighting, brightness: f32, right: Vec3, up: Vec3, forward: Vec3) -> Rig {
    let p = lighting.preset();
    let count = if lighting.five_point { 5 } else { 3 };
    let mut out = Rig { dir: [[0.0; 4]; 5], color: [[0.0; 4]; 5], count, ambient: [[0.0; 4]; 3], reflection: p.reflection };
    let turn = lighting.rotation.to_radians();
    for (i, l) in p.lights.iter().take(count).enumerate() {
        let (se, ce) = l.elevation.to_radians().sin_cos();
        let (sa, ca) = (l.azimuth.to_radians() + turn).sin_cos();
        let d = (right * (ce * sa) + up * se - forward * (ce * ca)).normalize_or(Vec3::Y);
        let c = linear(l.color);
        let k = l.intensity * brightness;
        out.dir[i] = [d.x, d.y, d.z, 0.0];
        out.color[i] = [c[0] * k, c[1] * k, c[2] * k, 0.0];
    }
    for (slot, c) in out.ambient.iter_mut().zip([p.ambient_sky, p.ambient_equator, p.ambient_ground]) {
        let c = linear(c);
        *slot = [c[0] * brightness, c[1] * brightness, c[2] * brightness, 0.0];
    }
    out
}

/// How wide the lights look, in radians, for the shader's highlights.
pub fn light_angle(lighting: &Lighting) -> f32 {
    (lighting.preset().light_size * lighting.light_size.clamp(0.0, 4.0)).to_radians()
}

pub struct Rig {
    pub dir: [[f32; 4]; 5],
    pub color: [[f32; 4]; 5],
    pub count: usize,
    /// Sky, equator, ground.
    pub ambient: [[f32; 4]; 3],
    pub reflection: f32,
}
