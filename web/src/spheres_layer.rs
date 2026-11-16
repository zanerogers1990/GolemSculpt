//! The SSpheres tool's layer of the viewport: the desktop's own
//! (`src/render/spheres.rs` and its shader, compiled here as they are), with
//! the few things it takes from the desktop's renderer stood in for: the
//! device it draws with, the viewport's depth format and samples, and the
//! change stamp (`cache::Stamp`), which the web has no use for but keeps so
//! the desktop file compiles unchanged.
//!
//! The shader writes sRGB-encoded colour into a plain 8-bit target, which is
//! what the web's viewport texture holds too, so it needs nothing changed.

use eframe::wgpu;

pub use spheres::{Layer, SkinLook};

/// The viewport's depth format and samples (render.rs).
pub const DEPTH_FORMAT: wgpu::TextureFormat = super::DEPTH;
pub const MSAA_SAMPLES: u32 = super::SAMPLES;

/// What the desktop layer draws with: the device and queue (handles, cheap to
/// copy) and the viewport texture's colour format.
pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub format: wgpu::TextureFormat,
}

impl Gpu {
    pub fn of(device: &wgpu::Device, queue: &wgpu::Queue) -> Gpu {
        Gpu { device: device.clone(), queue: queue.clone(), format: super::COLOR }
    }
}

/// What the tool hands the viewport for one frame (the desktop's
/// `Layer::show` arguments). The skin is shared, and only re-sent to the card
/// when its generation number moves.
pub struct Draw {
    pub view: crate::camera::View,
    pub armature: crate::ssphere::view::Armature,
    pub skin: Option<(std::sync::Arc<crate::ssphere::skin::Skin>, u64)>,
    pub look: SkinLook,
    pub lift: f32,
    pub model: glam::Mat4,
}

/// The layer kept with the renderer, made the first frame the tool is up.
#[derive(Default)]
pub struct Kept {
    layer: Option<(Gpu, Layer)>,
}

impl Kept {
    /// Sets what is drawn this frame: `draw`, or nothing.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, draw: Option<&Draw>) {
        let Some(d) = draw else {
            if let Some((_, layer)) = &mut self.layer {
                layer.hide();
            }
            return;
        };
        let (gpu, layer) = self.layer.get_or_insert_with(|| {
            let gpu = Gpu::of(device, queue);
            let layer = Layer::new(&gpu);
            (gpu, layer)
        });
        let skin = d.skin.as_ref().map(|(s, g)| (&**s, *g));
        layer.show(gpu, &d.view, &d.armature, skin, d.look, d.lift, d.model);
    }

    /// Draws into the viewport's pass, after the scene.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        if let Some((_, layer)) = &self.layer {
            layer.draw(pass);
        }
    }
}

pub mod cache {
    use std::hash::{Hash, Hasher};

    /// Notes when what the layer draws changes (the desktop redraws only
    /// then; the web draws every frame it is asked for, so nothing reads it).
    #[derive(Default)]
    pub struct Stamp {
        hash: u64,
    }

    impl Stamp {
        pub fn note(&mut self, parts: &[&[u8]]) {
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            for part in parts {
                part.hash(&mut hasher);
            }
            self.hash = hasher.finish();
        }
    }
}

#[path = "../../src/render/spheres.rs"]
#[allow(dead_code)]
pub mod spheres;
