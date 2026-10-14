//! The materials that come with the app: Plain, and one for every preset of
//! every finish the Look panel has (6 lure plastics, 7 metals, 5 clays, 6
//! woods and the 8 glows). They are made from the preset tables at start-up,
//! so a preset added there is a material here too, and are never written to
//! disk. Each is the Look's own starting settings with that preset picked:
//! what Look -> finish -> preset gives, before any slider is moved.
//!
//! Names are "Finish - Preset" ("Metal - Rust"), and the swatch is the dot
//! the Look panel draws beside the preset's button.

use super::{Glow, Material, Plain, Settings, Tint, TintMode};
use crate::look::{self, Look};

/// How a built-in finish takes painted colour. A plain material and a glow's
/// plain surface show it outright (a painted plain surface is the colour you
/// painted); clay takes it as a tint, the way a coloured clay body looks;
/// plastic, metal and wood have colours of their own that are the point of
/// them, so paint does not change them unless the material is told to.
fn tint_of(kind: super::Kind) -> Tint {
    use super::Kind;
    let mode = match kind {
        Kind::Plain | Kind::Glow => TintMode::Replace,
        Kind::Clay => TintMode::Multiply,
        _ => TintMode::Off,
    };
    Tint { mode, amount: 1.0 }
}

fn named(finish: &str, preset: &str, swatch: look::Rgb, settings: Settings) -> Material {
    let tint = tint_of(settings.kind());
    Material::new(&format!("{finish} - {preset}"), swatch, tint, settings)
}

/// A metal's swatch: its bare metal and its coat, mixed by how much of the
/// surface the coat covers. Not the Look panel's dot (the coat's colour
/// alone): that is near black for Antique Gold and Silver, whose coat is the
/// dark grime in the recesses, and a swatch also tints the matcap.
fn metal_swatch(p: &look::MetalPreset) -> look::Rgb {
    let c = p.coverage.clamp(0.0, 1.0);
    std::array::from_fn(|k| p.metal[k] * (1.0 - c) + p.coat_a[k] * c)
}

/// Every built-in material, in the order the library lists them.
pub fn all() -> Vec<Material> {
    let look = Look::default();
    let plain = Plain::default();
    let mut out = vec![Material::new("Plain", plain.base_color, tint_of(super::Kind::Plain), Settings::Plain(plain))];
    for (i, p) in look::LURES.iter().enumerate() {
        let s = look::LureSettings { preset: i, ..look.lure.clone() };
        out.push(named("Lure", p.name, p.thin, Settings::Lure(s)));
    }
    for (i, p) in look::METALS.iter().enumerate() {
        let s = look::MetalSettings { preset: i, ..look.metal.clone() };
        out.push(named("Metal", p.name, metal_swatch(p), Settings::Metal(s)));
    }
    for (i, p) in look::CLAYS.iter().enumerate() {
        let s = look::ClaySettings { preset: i, ..look.clay.clone() };
        out.push(named("Clay", p.name, p.color, Settings::Clay(s)));
    }
    for (i, p) in look::WOODS.iter().enumerate() {
        let s = look::WoodSettings { preset: i, ..look.wood.clone() };
        out.push(named("Wood", p.name, p.early, Settings::Wood(s)));
    }
    for (i, p) in look::GLOWS.iter().enumerate() {
        let glow = look::GlowSettings { on: true, preset: i, ..look.glow.clone() };
        out.push(named("Glow", p.name, p.cool, Settings::Glow(Glow { glow, base: Plain::default() })));
    }
    out
}

/// The material Save as Material makes of the Look's surface finish as it
/// is now, sliders and all. With no finish (plain colour) it is a plain
/// material of the model's colour and gloss.
pub fn from_look(look: &Look, base_color: [f32; 3], smoothness: f32) -> Material {
    let (finish, preset, swatch, settings) = match look.finish {
        look::Finish::None => {
            let plain = Plain { base_color, metallic: look.metallic, smoothness };
            ("Plain", "", base_color, Settings::Plain(plain))
        }
        look::Finish::LurePlastic => {
            let p = &look::LURES[look.lure.preset.min(look::LURES.len() - 1)];
            ("Lure", p.name, p.thin, Settings::Lure(look.lure.clone()))
        }
        look::Finish::Metal => {
            let p = &look::METALS[look.metal.preset.min(look::METALS.len() - 1)];
            ("Metal", p.name, metal_swatch(p), Settings::Metal(look.metal.clone()))
        }
        look::Finish::Clay => {
            let p = &look::CLAYS[look.clay.preset.min(look::CLAYS.len() - 1)];
            ("Clay", p.name, p.color, Settings::Clay(look.clay.clone()))
        }
        look::Finish::Wood => {
            let p = &look::WOODS[look.wood.preset.min(look::WOODS.len() - 1)];
            ("Wood", p.name, p.early, Settings::Wood(look.wood.clone()))
        }
    };
    let name = if preset.is_empty() { format!("My {finish}") } else { format!("My {finish} - {preset}") };
    let tint = tint_of(settings.kind());
    Material::new(&name, swatch, tint, settings)
}
