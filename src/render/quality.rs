//! How hard the renderer works. These belong to the computer, not to a
//! scene: they are kept in the settings file (prefs.rs), so a sculpt opened
//! on a slower machine does not bring a heavy shadow map with it.

use serde::{Deserialize, Serialize};

/// The sides the key light's shadow map can have, smallest first.
pub const SHADOW_SIZES: [u32; 3] = [512, 2048, 4096];
/// The sides the reflection cube can have.
pub const REFLECTION_SIZES: [u32; 2] = [256, 512];

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Quality {
    /// Side of the key light's shadow map, in texels: one of
    /// [`SHADOW_SIZES`]. Larger keeps contact shadows crisp on a big scene.
    pub shadow_map: u32,
    /// Side of the cube of surroundings the model reflects: one of
    /// [`REFLECTION_SIZES`].
    pub reflections: u32,
}

impl Default for Quality {
    fn default() -> Self {
        Self { shadow_map: 2048, reflections: 256 }
    }
}

/// The one of `sizes` nearest `wanted`.
fn nearest(sizes: &[u32], wanted: u32) -> u32 {
    sizes.iter().copied().min_by_key(|&s| s.abs_diff(wanted)).unwrap_or(sizes[0])
}

impl Quality {
    /// The shadow map's side: the size asked for, or the nearest one there is.
    pub fn shadow_size(&self) -> u32 {
        nearest(&SHADOW_SIZES, self.shadow_map)
    }

    /// The reflection cube's side, the same way.
    pub fn reflection_size(&self) -> u32 {
        nearest(&REFLECTION_SIZES, self.reflections)
    }

    /// The same settings, each brought to a value the renderer has.
    pub fn sanitized(self) -> Quality {
        Quality { shadow_map: self.shadow_size(), reflections: self.reflection_size() }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_shadow_size_is_always_one_the_renderer_has() {
        assert_eq!(Quality::default().shadow_size(), 2048);
        for (asked, got) in [(0, 512), (512, 512), (1000, 512), (1500, 2048), (3000, 2048), (4096, 4096), (99999, 4096)] {
            assert_eq!(Quality { shadow_map: asked, ..Default::default() }.shadow_size(), got, "{asked}");
        }
        assert_eq!(Quality::default().reflection_size(), 256);
        assert_eq!(Quality { reflections: 1024, ..Default::default() }.sanitized().reflections, 512);
    }

    #[test]
    fn a_settings_file_from_before_reads_as_the_usual_quality() {
        let read: Quality = serde_json::from_str("{}").unwrap();
        assert_eq!(read, Quality::default());
    }
}
