//! How one paint layer's colour lands on what is under it: the blend modes of
//! the W3C "Compositing and Blending Level 1" note, which are the ones every
//! painting program uses under the same names, so a sculptor who knows
//! Multiply or Overlay from elsewhere gets what they expect.
//!
//! Everything works on sRGB-encoded values from 0 to 1, as the note does and
//! as painting programs do: the colours are blended as they are stored, not
//! after converting to linear light. (Blending in linear light gives other
//! answers for every mode but Normal, and a Multiply that does not match
//! what the sculptor has seen before is a surprise, not an improvement.)
//!
//! The formulas are pure functions of numbers, so each is tested against the
//! note's own formula worked out in double precision (`tests.rs`).

/// How a layer's colour is mixed with the colour under it.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum ColourBlend {
    /// The layer's colour over what is under it.
    #[default]
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
    HardLight,
    SoftLight,
    /// Linear dodge: the two added, held at white.
    Add,
    /// The layer's colour taken away from what is under it, held at black.
    Subtract,
    Hue,
    Saturation,
    Colour,
}

impl ColourBlend {
    /// Every mode, in the order a menu lists them.
    pub const ALL: [ColourBlend; 13] = [
        ColourBlend::Normal,
        ColourBlend::Multiply,
        ColourBlend::Screen,
        ColourBlend::Overlay,
        ColourBlend::Darken,
        ColourBlend::Lighten,
        ColourBlend::HardLight,
        ColourBlend::SoftLight,
        ColourBlend::Add,
        ColourBlend::Subtract,
        ColourBlend::Hue,
        ColourBlend::Saturation,
        ColourBlend::Colour,
    ];

    /// The name a menu shows.
    pub fn name(self) -> &'static str {
        match self {
            ColourBlend::Normal => "Normal",
            ColourBlend::Multiply => "Multiply",
            ColourBlend::Screen => "Screen",
            ColourBlend::Overlay => "Overlay",
            ColourBlend::Darken => "Darken",
            ColourBlend::Lighten => "Lighten",
            ColourBlend::HardLight => "Hard Light",
            ColourBlend::SoftLight => "Soft Light",
            ColourBlend::Add => "Add",
            ColourBlend::Subtract => "Subtract",
            ColourBlend::Hue => "Hue",
            ColourBlend::Saturation => "Saturation",
            ColourBlend::Colour => "Colour",
        }
    }

    /// A byte for a file. The numbers never change meaning.
    pub fn to_byte(self) -> u8 {
        Self::ALL.iter().position(|&mode| mode == self).unwrap_or(0) as u8
    }

    /// The mode a file's byte names; a number from a later version reads as Normal.
    pub fn from_byte(byte: u8) -> ColourBlend {
        Self::ALL.get(byte as usize).copied().unwrap_or_default()
    }

    /// `B(Cb, Cs)` of the note: the mixed colour where both are fully there.
    pub fn mix(self, cb: [f32; 3], cs: [f32; 3]) -> [f32; 3] {
        let each = |f: fn(f32, f32) -> f32| [f(cb[0], cs[0]), f(cb[1], cs[1]), f(cb[2], cs[2])];
        match self {
            ColourBlend::Normal => cs,
            ColourBlend::Multiply => each(|b, s| b * s),
            ColourBlend::Screen => each(screen),
            ColourBlend::Overlay => each(|b, s| hard_light(s, b)),
            ColourBlend::Darken => each(f32::min),
            ColourBlend::Lighten => each(f32::max),
            ColourBlend::HardLight => each(hard_light),
            ColourBlend::SoftLight => each(soft_light),
            ColourBlend::Add => each(|b, s| (b + s).min(1.0)),
            ColourBlend::Subtract => each(|b, s| (b - s).max(0.0)),
            ColourBlend::Hue => set_lum(set_sat(cs, sat(cb)), lum(cb)),
            ColourBlend::Saturation => set_lum(set_sat(cb, sat(cs)), lum(cb)),
            ColourBlend::Colour => set_lum(cs, lum(cb)),
        }
    }
}

/// A colour that may be partly there: red, green and blue from 0 to 1 (not
/// multiplied by the alpha) and how much of it there is.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rgba {
    pub rgb: [f32; 3],
    pub a: f32,
}

impl Rgba {
    /// Nothing at all.
    pub const CLEAR: Rgba = Rgba { rgb: [0.0; 3], a: 0.0 };
}

/// The note's compositing of a source `src` (the layer, its alpha already
/// scaled by the layer's opacity and mask) over a backdrop `dst` with blend
/// mode `mode`:
///
/// ```text
/// Cs' = (1 - ab) Cs + ab B(Cb, Cs)
/// co  = as Cs' + (1 - as) ab Cb
/// ao  = as + ab (1 - as)
/// Co  = co / ao
/// ```
///
/// Where nothing is under the layer (`ab` = 0) the layer shows as it is,
/// whatever its mode: a Multiply layer painted on bare clay is its own colour,
/// not black. That is the note's rule, and what painting programs do.
pub fn over(mode: ColourBlend, src: Rgba, dst: Rgba) -> Rgba {
    let (sa, ba) = (src.a, dst.a);
    if sa <= 0.0 {
        return dst;
    }
    let mixed = if ba > 0.0 && mode != ColourBlend::Normal { mode.mix(dst.rgb, src.rgb) } else { src.rgb };
    let ao = sa + ba * (1.0 - sa);
    let mut rgb = [0.0; 3];
    for k in 0..3 {
        let cs = (1.0 - ba) * src.rgb[k] + ba * mixed[k];
        let co = sa * cs + (1.0 - sa) * ba * dst.rgb[k];
        rgb[k] = if ao > 0.0 { (co / ao).clamp(0.0, 1.0) } else { 0.0 };
    }
    Rgba { rgb, a: ao.clamp(0.0, 1.0) }
}

pub fn screen(b: f32, s: f32) -> f32 {
    b + s - b * s
}

pub fn hard_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 { b * 2.0 * s } else { screen(b, 2.0 * s - 1.0) }
}

pub fn soft_light(b: f32, s: f32) -> f32 {
    if s <= 0.5 {
        b - (1.0 - 2.0 * s) * b * (1.0 - b)
    } else {
        let d = if b <= 0.25 { ((16.0 * b - 12.0) * b + 4.0) * b } else { b.sqrt() };
        b + (2.0 * s - 1.0) * (d - b)
    }
}

pub fn lum(c: [f32; 3]) -> f32 {
    0.3 * c[0] + 0.59 * c[1] + 0.11 * c[2]
}

pub fn clip_colour(c: [f32; 3]) -> [f32; 3] {
    let l = lum(c);
    let n = c[0].min(c[1]).min(c[2]);
    let x = c[0].max(c[1]).max(c[2]);
    let mut c = c;
    if n < 0.0 && l - n > 0.0 {
        c = c.map(|v| l + (v - l) * l / (l - n));
    }
    if x > 1.0 && x - l > 0.0 {
        c = c.map(|v| l + (v - l) * (1.0 - l) / (x - l));
    }
    c
}

pub fn set_lum(c: [f32; 3], l: f32) -> [f32; 3] {
    let d = l - lum(c);
    clip_colour(c.map(|v| v + d))
}

pub fn sat(c: [f32; 3]) -> f32 {
    c[0].max(c[1]).max(c[2]) - c[0].min(c[1]).min(c[2])
}

pub fn set_sat(c: [f32; 3], s: f32) -> [f32; 3] {
    // The places of the largest, middle and smallest component.
    let mut order = [0usize, 1, 2];
    order.sort_by(|&i, &j| c[i].total_cmp(&c[j]));
    let [min, mid, max] = order;
    let mut out = [0.0; 3];
    if c[max] > c[min] {
        out[mid] = (c[mid] - c[min]) * s / (c[max] - c[min]);
        out[max] = s;
    }
    out
}
