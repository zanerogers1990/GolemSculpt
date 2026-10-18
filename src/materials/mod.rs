//! Materials: what paint can put on a model besides colour. A material is a
//! finish (plain, lure plastic, aged metal, clay, wood, a glow, or a custom
//! shader of the user's own) with its settings, a swatch colour for rows and
//! plain previews, and how painted colour tints it.
//!
//! **Settings, not packed numbers.** A material keeps the same settings the
//! Look panel edits (`look::MetalSettings` and the rest) and is turned into
//! the shader's sixteen vec4s only when it is drawn ([`Material::pack`]), by
//! the very `Look::pack_*` functions the Look uses. So "Metal - Rust" from
//! the library and Look -> Metal -> Rust are the same numbers on the card,
//! byte for byte (a test holds them to it), and a later change to how a
//! finish is packed reaches saved materials too.
//!
//! **Where materials come from:**
//! - built in ([`builtin`]): Plain and one per preset of every finish (6
//!   lures, 7 metals, 5 clays, 6 woods, 8 glows), made at start-up and never
//!   written anywhere;
//! - custom shaders ([`custom`]): a `.wgsl` file and a `.json` sidecar in the
//!   `materials/shaders` folder, each one material of its own;
//! - the user's own ([`files`]): one JSON file each in the `materials`
//!   folder beside the settings, made by Duplicate, Save as Material or
//!   Import Material.
//!
//! The scene's palette ([`palette`]) holds full copies of the materials the
//! scene's paint uses, so a scene file opens the same on a computer that has
//! none of them.

pub mod builtin;
pub mod custom;
pub mod demo;
pub mod files;
pub mod library;
pub mod palette;
pub mod rows;
pub mod thumbs;

#[cfg(test)]
mod tests;

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::look::{
    ClaySettings, FinishParams, GlowSettings, Look, LureSettings, MetalSettings, ResinSettings, WoodSettings, linear,
};

pub use library::{Ask, Library};

/// The longest a material's name may be: what fits beside its thumbnail.
pub const MAX_NAME: usize = 32;

/// The library and the custom shaders, as the app and the Materials card
/// share them, and what the palette was last packed from.
#[derive(Default)]
pub struct State {
    pub library: Library,
    pub shaders: custom::Shaders,
    pub thumbs: thumbs::Cache,
    /// The palette, model size and shaders the renderer's palette was last
    /// packed from (see app/materials.rs), so it is packed only when one of
    /// them changes.
    pub packed_from: Option<(crate::mesh::paint::palette::ScenePaint, f32, u64)>,
    /// Moves whenever a custom shader is read again.
    pub shaders_seen: u64,
    /// The grooved ball thumbnails are drawn on, made the first time one is.
    pub ball: Option<crate::mesh::Mesh>,
}

/// Which finish function shades a material. The numbers are what the
/// painted shader switches on (`KIND_` in painted.wgsl); 0 to 4 are the
/// Look's own `FINISH_` numbers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Kind {
    Plain = 0,
    Lure = 1,
    Metal = 2,
    Clay = 3,
    Wood = 4,
    Resin = 5,
    Glow = 6,
    Custom = 7,
    /// Nothing but the swatch colour, plainly lit: what a material that
    /// cannot be drawn (a custom shader that does not compile) shows as.
    Swatch = 8,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Plain => "Plain",
            Kind::Lure => "Lure Plastic",
            Kind::Metal => "Metal",
            Kind::Clay => "Clay",
            Kind::Wood => "Wood",
            Kind::Resin => "Resin",
            Kind::Glow => "Glow",
            Kind::Custom => "Custom",
            Kind::Swatch => "Swatch",
        }
    }

    /// Whether the finish reads the mesh's curvature (recess or ridge).
    pub fn reads_curvature(self) -> bool {
        matches!(self, Kind::Metal | Kind::Clay | Kind::Wood | Kind::Glow)
    }

    /// Whether the finish reads the depth of the back faces behind it.
    pub fn reads_back_depth(self) -> bool {
        matches!(self, Kind::Lure | Kind::Resin)
    }
}

/// How painted colour changes a material where both are painted.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default, Serialize, Deserialize)]
pub enum TintMode {
    /// The material keeps its own colours; painted colour does not show on it.
    #[default]
    Off,
    /// The material's colour is multiplied by the paint: a tinted metal, a
    /// coloured clay.
    Multiply,
    /// The paint takes the place of the material's colour, which keeps its
    /// gloss, bumps and the rest.
    Replace,
}

impl TintMode {
    pub const ALL: [TintMode; 3] = [TintMode::Off, TintMode::Multiply, TintMode::Replace];

    pub fn name(self) -> &'static str {
        match self {
            TintMode::Off => "Off",
            TintMode::Multiply => "Multiply",
            TintMode::Replace => "Replace",
        }
    }

    /// The number the painted shader reads.
    pub fn code(self) -> f32 {
        self as u32 as f32
    }
}

/// Tint by paint colour: the mode and how much of it (0 to 1).
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Tint {
    pub mode: TintMode,
    pub amount: f32,
}

impl Default for Tint {
    fn default() -> Self {
        Tint { mode: TintMode::Off, amount: 1.0 }
    }
}

/// The plain finish's settings: also the surface under a glow.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Plain {
    /// sRGB 0 to 1.
    pub base_color: [f32; 3],
    pub metallic: f32,
    pub smoothness: f32,
}

impl Default for Plain {
    /// The model's own colour and gloss as a new scene has them (a test
    /// holds these to `doc::Options::default`, which cannot be asked here:
    /// it holds the library, whose Plain material is made of this).
    fn default() -> Self {
        Plain { base_color: [0.65, 0.65, 0.68], metallic: 0.0, smoothness: 0.4 }
    }
}

/// A glow material: light given off over a plain surface.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Glow {
    pub glow: GlowSettings,
    #[serde(default)]
    pub base: Plain,
}

/// One slider's value of a custom material.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Value {
    Toggle(bool),
    Number(f32),
    /// sRGB 0 to 1.
    Colour([f32; 3]),
}

/// A custom material: which shader, and its sliders' values by name (a
/// slider with no value here has its sidecar default).
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Custom {
    pub id: String,
    pub values: BTreeMap<String, Value>,
}

/// The finish and its settings.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
#[serde(tag = "finish")]
pub enum Settings {
    Plain(Plain),
    Lure(LureSettings),
    Metal(MetalSettings),
    Clay(ClaySettings),
    Wood(WoodSettings),
    Resin(ResinSettings),
    Glow(Glow),
    Custom(Custom),
}

impl Settings {
    pub fn kind(&self) -> Kind {
        match self {
            Settings::Plain(_) => Kind::Plain,
            Settings::Lure(_) => Kind::Lure,
            Settings::Metal(_) => Kind::Metal,
            Settings::Clay(_) => Kind::Clay,
            Settings::Wood(_) => Kind::Wood,
            Settings::Resin(_) => Kind::Resin,
            Settings::Glow(_) => Kind::Glow,
            Settings::Custom(_) => Kind::Custom,
        }
    }

    /// The settings a material of `kind` starts with: the Look's own
    /// starting settings for that finish. A custom one names no shader.
    pub fn default_of(kind: Kind) -> Settings {
        let look = Look::default();
        match kind {
            Kind::Plain | Kind::Swatch => Settings::Plain(Plain::default()),
            Kind::Lure => Settings::Lure(look.lure),
            Kind::Metal => Settings::Metal(look.metal),
            Kind::Clay => Settings::Clay(look.clay),
            Kind::Wood => Settings::Wood(look.wood),
            Kind::Resin => Settings::Resin(look.resin),
            Kind::Glow => Settings::Glow(Glow { glow: GlowSettings { on: true, ..look.glow }, base: Plain::default() }),
            Kind::Custom => Settings::Custom(Custom::default()),
        }
    }

    /// The finish named `word` in a file, whatever its case.
    pub fn kind_named(word: &str) -> Option<Kind> {
        let word = word.trim();
        [Kind::Plain, Kind::Lure, Kind::Metal, Kind::Clay, Kind::Wood, Kind::Resin, Kind::Glow, Kind::Custom]
            .into_iter()
            .find(|k| format!("{k:?}").eq_ignore_ascii_case(word))
    }
}

/// A custom material's shader as the scene file keeps it: the source and
/// the sidecar as they were when the material was put in the palette, so
/// the scene opens anywhere.
#[derive(Clone, PartialEq, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Source {
    pub wgsl: String,
    pub sidecar: String,
}

/// A material.
#[derive(Clone, PartialEq, Debug, Serialize, Deserialize)]
pub struct Material {
    pub name: String,
    /// sRGB 0 to 1: the row's dot, the matcap's tint, and what the material
    /// shows as if it cannot be drawn.
    #[serde(default = "grey")]
    pub swatch: [f32; 3],
    #[serde(default)]
    pub tint: Tint,
    pub settings: Settings,
    /// A custom material's shader, in a palette's copy only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<Source>,
}

fn grey() -> [f32; 3] {
    [0.6, 0.6, 0.6]
}

/// What a material is packed into for the shader (see [`Material::pack`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Packed {
    pub kind: Kind,
    pub params: FinishParams,
    /// Linear colour of the plain surface (plain, and under a glow).
    pub base: [f32; 3],
    /// The plain surface's metallic and smoothness, under a glow.
    pub base_metal_smooth: [f32; 2],
}

impl Material {
    pub fn new(name: &str, swatch: [f32; 3], tint: Tint, settings: Settings) -> Material {
        Material { name: name.to_owned(), swatch, tint, settings, source: None }
    }

    pub fn kind(&self) -> Kind {
        self.settings.kind()
    }

    /// The material as the shader reads it, with its patterns at `size` (the
    /// model size frozen in its palette entry). Each built-in finish is
    /// packed by the Look's own packer with these settings in the Look's
    /// place. A custom material is packed by its sidecar (`custom::pack`);
    /// one whose shader is not known packs as its swatch.
    pub fn pack(&self, size: f32, shaders: &custom::Shaders) -> Packed {
        let mut look = Look::default();
        let plain = |p: &Plain| (linear(p.base_color), [p.metallic.clamp(0.0, 1.0), p.smoothness.clamp(0.0, 1.0)]);
        let swatch = Packed {
            kind: Kind::Swatch,
            params: bytemuck::Zeroable::zeroed(),
            base: linear(self.swatch),
            base_metal_smooth: [0.0, 0.4],
        };
        let finish = |kind: Kind, params: FinishParams| Packed { kind, params, ..swatch };
        match &self.settings {
            Settings::Plain(p) => {
                look.metallic = p.metallic;
                let (base, base_metal_smooth) = plain(p);
                Packed { kind: Kind::Plain, params: look.pack_pbr(p.smoothness.clamp(0.0, 1.0)), base, base_metal_smooth }
            }
            Settings::Lure(s) => {
                look.lure = s.clone();
                finish(Kind::Lure, look.pack_plastic(size))
            }
            Settings::Metal(s) => {
                look.metal = s.clone();
                finish(Kind::Metal, look.pack_metal(size))
            }
            Settings::Clay(s) => {
                look.clay = s.clone();
                finish(Kind::Clay, look.pack_clay(size))
            }
            Settings::Wood(s) => {
                look.wood = s.clone();
                finish(Kind::Wood, look.pack_wood(size))
            }
            Settings::Resin(s) => {
                look.resin = s.clone();
                finish(Kind::Resin, look.pack_resin(size))
            }
            Settings::Glow(g) => {
                look.glow = GlowSettings { on: true, ..g.glow.clone() };
                let (base, base_metal_smooth) = plain(&g.base);
                Packed { kind: Kind::Glow, params: look.pack_glow(size), base, base_metal_smooth }
            }
            Settings::Custom(c) => match shaders.usable(&c.id) {
                Some(shader) => finish(Kind::Custom, custom::pack(&shader.sidecar, &c.values, size)),
                None => swatch,
            },
        }
    }

    /// The material with every number inside what its sliders allow and its
    /// name one line of plain text: what a material read from a file is put
    /// through, so that nothing a file says reaches the shader out of range.
    pub fn in_range(mut self) -> Material {
        let unit = |x: f32, or: f32| if x.is_finite() { x.clamp(0.0, 1.0) } else { or };
        let fit = |x: f32, lo: f32, hi: f32, or: f32| if x.is_finite() { x.clamp(lo, hi) } else { or };
        let name: String = self.name.replace(char::is_control, " ").trim().chars().take(MAX_NAME).collect();
        self.name = if name.is_empty() { "Material".to_owned() } else { name };
        self.swatch = self.swatch.map(|c| unit(c, 0.6));
        self.tint.amount = unit(self.tint.amount, 1.0);
        let plain = |p: &mut Plain| {
            p.base_color = p.base_color.map(|c| unit(c, 0.65));
            p.metallic = unit(p.metallic, 0.0);
            p.smoothness = unit(p.smoothness, 0.4);
        };
        match &mut self.settings {
            Settings::Plain(p) => plain(p),
            Settings::Lure(s) => {
                s.flake_size = fit(s.flake_size, 0.25, 3.0, 1.0);
                s.flake_amount = fit(s.flake_amount, 0.0, 2.0, 1.0);
                s.sparkle = fit(s.sparkle, 0.0, 3.0, 1.0);
                s.translucency = fit(s.translucency, 0.25, 3.0, 1.0);
                s.gloss = unit(s.gloss, 0.8);
            }
            Settings::Metal(s) => {
                s.exposure = fit(s.exposure, 0.0, 2.0, 1.0);
                s.edge_wear = fit(s.edge_wear, 0.0, 2.0, 1.0);
                s.wash = fit(s.wash, 0.0, 1.5, 1.0);
                s.detail = fit(s.detail, 0.25, 3.0, 1.0);
                s.pattern_size = fit(s.pattern_size, 0.25, 3.0, 1.0);
                s.gloss = fit(s.gloss, 0.0, 2.0, 1.0);
                s.seed = unit(s.seed, 0.0);
            }
            Settings::Resin(s) => {
                s.clarity = fit(s.clarity, 0.1, 4.0, 1.0);
                s.haze = fit(s.haze, 0.0, 3.0, 1.0);
                s.gloss = unit(s.gloss, 1.0);
            }
            Settings::Clay(s) => {
                s.gloss = fit(s.gloss, 0.0, 2.0, 1.0);
                s.wetness = fit(s.wetness, 0.0, 2.0, 1.0);
                s.subsurface = fit(s.subsurface, 0.0, 2.0, 1.0);
                s.recess = fit(s.recess, 0.0, 2.0, 1.0);
                s.detail = fit(s.detail, 0.25, 3.0, 1.0);
                s.grain = fit(s.grain, 0.0, 3.0, 1.0);
            }
            Settings::Wood(s) => {
                s.axis = s.axis.clamp(0, 2);
                s.grain = fit(s.grain, 0.0, 2.0, 1.0);
                s.grain_scale = fit(s.grain_scale, 0.25, 3.0, 1.0);
                s.edge_wear = fit(s.edge_wear, 0.0, 2.0, 1.0);
                s.patina = fit(s.patina, 0.0, 2.0, 1.0);
                s.detail = fit(s.detail, 0.25, 3.0, 1.0);
                s.sheen = fit(s.sheen, 0.0, 2.0, 1.0);
                s.cracks = fit(s.cracks, 0.0, 2.0, 1.0);
                s.seed = unit(s.seed, 0.0);
            }
            Settings::Glow(g) => {
                let s = &mut g.glow;
                s.on = true;
                s.intensity = fit(s.intensity, 0.0, 3.0, 1.0);
                s.coverage = fit(s.coverage, 0.0, 2.0, 1.0);
                s.rim = fit(s.rim, 0.0, 2.0, 1.0);
                s.pattern_size = fit(s.pattern_size, 0.25, 3.0, 1.0);
                s.speed = fit(s.speed, 0.0, 3.0, 1.0);
                s.pulse = fit(s.pulse, 0.0, 2.0, 1.0);
                s.dim = fit(s.dim, 0.0, 2.0, 1.0);
                s.hue_shift = unit(s.hue_shift, 0.0);
                plain(&mut g.base);
            }
            Settings::Custom(c) => {
                c.id = custom::clean_id(&c.id).unwrap_or_default();
                for value in c.values.values_mut() {
                    match value {
                        Value::Number(x) if !x.is_finite() => *x = 0.0,
                        Value::Colour(rgb) => *rgb = rgb.map(|c| unit(c, 0.5)),
                        _ => {}
                    }
                }
            }
        }
        self
    }

    /// The material as a file keeps it.
    pub fn to_json(&self) -> serde_json::Value {
        serde_json::to_value(self).unwrap_or(serde_json::Value::Null)
    }

    /// Reads a material from JSON as forgivingly as the settings file is
    /// read (`io::read_over`): a field the JSON lacks keeps the starting
    /// value of the material's finish, one this version does not know is
    /// ignored, one of the wrong kind is passed over on its own, and numbers
    /// are brought into their sliders' range. Only JSON that names no finish
    /// this version has is refused. `fallback_name` is the material's name if
    /// the JSON gives none.
    pub fn from_json(value: &serde_json::Value, fallback_name: &str) -> Result<Material, String> {
        let settings = value.get("settings").ok_or("it names no finish")?;
        let word = settings.get("finish").and_then(serde_json::Value::as_str).ok_or("it names no finish")?;
        let kind = Settings::kind_named(word).ok_or_else(|| format!("its finish, {word}, is not one this version has"))?;
        let start = Material::new(fallback_name, grey(), Tint::default(), Settings::default_of(kind));
        let mut read = crate::io::read_over(&start, value).unwrap_or(start);
        if read.name.trim().is_empty() {
            read.name = fallback_name.to_owned();
        }
        Ok(read.in_range())
    }

    /// A short line about the material for its row's tip.
    pub fn about(&self) -> String {
        let tint = match self.tint.mode {
            TintMode::Off => String::new(),
            mode => format!(" Painted colour tints it ({}, {:.0}%).", mode.name(), self.tint.amount * 100.0),
        };
        format!("{} material.{tint}", self.kind().name())
    }
}
