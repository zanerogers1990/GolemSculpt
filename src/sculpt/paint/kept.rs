//! The Paint brush's settings as the settings file keeps them (`prefs.rs`).
//!
//! Every field is optional on reading, and each is brought into the range its
//! control allows, so a file from before there was a Paint brush, or one
//! edited by hand, gives the settings the brush starts with. The material to
//! paint with is the scene's, not the app's, and is not here; nor is Paint
//! Layer Mask, which is how one stroke is being made and not a preference.

use serde::{Deserialize, Serialize};

use super::{PaintMode, PaintSettings, RECENT, ZADD_BRUSHES};
use crate::sculpt::Brush;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct PaintKept {
    /// sRGB.
    pub colour: [u8; 3],
    pub recent: Vec<[u8; 3]>,
    pub opacity: f32,
    /// By name: Paint, Erase, Blur, Smear or Sample.
    pub mode: String,
    pub rgb: bool,
    pub material: bool,
    pub roughness: bool,
    pub metallic: bool,
    pub roughness_value: f32,
    pub metallic_value: f32,
    pub zadd: bool,
    /// The kernel Zadd runs, by name.
    pub zadd_brush: String,
    pub tolerance: f32,
}

impl Default for PaintKept {
    fn default() -> Self {
        Self::of(&PaintSettings::default())
    }
}

/// A number from the file within 0 to 1; one that is not a number gives way to `or`.
fn unit(value: f32, or: f32) -> f32 {
    if value.is_finite() { value.clamp(0.0, 1.0) } else { or }
}

impl PaintKept {
    pub fn of(s: &PaintSettings) -> Self {
        PaintKept {
            colour: s.colour,
            recent: s.recent.clone(),
            opacity: s.opacity,
            mode: s.mode.name().to_owned(),
            rgb: s.rgb,
            material: s.material,
            roughness: s.roughness,
            metallic: s.metallic,
            roughness_value: s.roughness_value,
            metallic_value: s.metallic_value,
            zadd: s.zadd,
            zadd_brush: s.zadd_brush.name().to_owned(),
            tolerance: s.tolerance,
        }
    }

    /// Lays what the file kept into `s`.
    pub fn apply(&self, s: &mut PaintSettings) {
        let d = PaintSettings::default();
        s.colour = self.colour;
        s.recent = self.recent.iter().copied().take(RECENT).collect();
        // A brush that was left on Sample would leave the next session's first press doing nothing to see.
        s.mode = PaintMode::named(&self.mode).filter(|&m| m != PaintMode::Sample).unwrap_or(d.mode);
        s.opacity = unit(self.opacity, d.opacity).max(0.01);
        (s.rgb, s.material, s.roughness, s.metallic) = (self.rgb, self.material, self.roughness, self.metallic);
        s.roughness_value = unit(self.roughness_value, d.roughness_value);
        s.metallic_value = unit(self.metallic_value, d.metallic_value);
        s.zadd = self.zadd;
        s.zadd_brush = ZADD_BRUSHES.into_iter().find(|b| b.name() == self.zadd_brush).unwrap_or(Brush::Standard);
        s.tolerance = unit(self.tolerance, d.tolerance);
    }
}
