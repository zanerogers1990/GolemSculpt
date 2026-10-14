//! The last finished picture of the 3D view, kept so that a frame in which
//! nothing of the scene changed costs one copy instead of every pass over
//! every mesh.
//!
//! Most frames are like that: the pointer moved and the brush ring with it,
//! or the interface redrew a slider, and the ring and the interface are
//! drawn by egui over the picture, not into it. With ten million triangles
//! and the lit look that was four full passes over the model (shadow,
//! normals, back faces for the plastic, the multisampled colour pass) for a
//! picture that came out the same.
//!
//! **Whether anything changed is worked out from what the picture is drawn
//! from**, not from flags each caller has to remember to set. A frame's
//! [`SceneKey`] is the bytes of everything the passes read: the shared
//! uniforms (camera, shading switches, lights, every finish setting), the
//! list of objects with their matrices, the post effects' settings, the size,
//! and a [`Stamp`] from each overlay layer and from the mesh buffers that
//! moves whenever one of them sends the graphics card something new. When
//! the key is byte for byte the one the kept picture was drawn with, the
//! picture is the same, and it is copied out again. A feature added later
//! that moves vertices (a deformer, a paint stroke) goes through the mesh
//! upload, and one that changes a setting goes through the uniforms, so it
//! invalidates the picture without knowing this file exists. What cannot be
//! seen that way (a new matcap or HDRI picture) sets
//! [`super::Renderer::scene_dirty`].
//!
//! The clock is part of the uniforms but only matters while a glow is
//! moving, so it is left out of the key otherwise; a moving glow, a running
//! pulse, a camera that is still easing and a tool's live preview all change
//! the key every frame and are drawn every frame, as before.

use std::hash::{Hash, Hasher};

use bytemuck::Pod;

use super::Gpu;

/// The bytes of everything a frame's 3D picture is drawn from.
#[derive(Default, Clone, PartialEq, Eq, Debug)]
pub struct SceneKey(Vec<u8>);

impl SceneKey {
    pub fn push<T: Pod>(&mut self, value: &T) -> &mut Self {
        self.0.extend_from_slice(bytemuck::bytes_of(value));
        self
    }
}

/// A count that moves whenever a layer sends something new to the graphics
/// card, for the scene key. A layer that writes the same thing every frame
/// (the pulse, the SSphere armature, the tools' previews) notes what it
/// wrote with [`Stamp::note`], which only moves the count when that differs
/// from last time.
#[derive(Default, Clone, Copy, PartialEq, Eq, Debug)]
pub struct Stamp {
    hash: u64,
    count: u64,
}

impl Stamp {
    /// Something new went to the card.
    pub fn bump(&mut self) {
        self.count = self.count.wrapping_add(1);
    }

    /// `parts` went to the card: moves the count if they differ from what
    /// was noted last.
    pub fn note(&mut self, parts: &[&[u8]]) {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for part in parts {
            part.hash(&mut hasher);
        }
        let hash = hasher.finish();
        if hash != self.hash {
            self.hash = hash;
            self.bump();
        }
    }

    pub fn count(&self) -> u64 {
        self.count
    }
}

const BLIT_WGSL: &str = r"
@group(0) @binding(0) var picture: texture_2d<f32>;

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(p, 0.0, 1.0);
}

@fragment
fn fs_blit(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(picture, vec2<i32>(pos.xy), 0);
}
";

/// Copies the kept picture into the frame's target, pixel for pixel. A draw
/// rather than a texture copy: the window's surface is not made to be copied
/// into on every system, and a draw works for every target.
pub struct Blit {
    layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
}

impl Blit {
    pub fn new(gpu: &Gpu) -> Blit {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("blit"),
            source: wgpu::ShaderSource::Wgsl(BLIT_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: false },
                    view_dimension: wgpu::TextureViewDimension::D2,
                    multisampled: false,
                },
                count: None,
            }],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("blit"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_blit"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_blit"),
                compilation_options: Default::default(),
                targets: &[Some(gpu.format.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        Blit { layout, pipeline }
    }

    /// The bind group that reads `picture`.
    pub fn bind(&self, gpu: &Gpu, picture: &wgpu::TextureView) -> wgpu::BindGroup {
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit"),
            layout: &self.layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(picture) }],
        })
    }

    /// Draws the picture `group` reads over all of `target`.
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder, group: &wgpu::BindGroup, target: &wgpu::TextureView) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamp_moves_only_when_what_was_noted_changes() {
        let mut stamp = Stamp::default();
        stamp.note(&[b"abc", b"de"]);
        let first = stamp.count();
        stamp.note(&[b"abc", b"de"]);
        assert_eq!(stamp.count(), first, "the same bytes again leave it");
        // The same bytes split differently are something else.
        stamp.note(&[b"ab", b"cde"]);
        assert_ne!(stamp.count(), first);
        let second = stamp.count();
        stamp.bump();
        assert_ne!(stamp.count(), second);
    }

    #[test]
    fn keys_compare_by_every_byte() {
        let mut a = SceneKey::default();
        a.push(&1.0f32).push(&[2u32, 3]);
        let mut b = SceneKey::default();
        b.push(&1.0f32).push(&[2u32, 3]);
        assert_eq!(a, b);
        let mut d = SceneKey::default();
        d.push(&1.000001f32).push(&[2u32, 3]);
        assert_ne!(a, d);
        // Negative zero is a different picture input as far as the key goes:
        // the comparison is of bytes, never of values.
        let mut z = SceneKey::default();
        z.push(&-0.0f32).push(&[2u32, 3]);
        let mut p = SceneKey::default();
        p.push(&0.0f32).push(&[2u32, 3]);
        assert_ne!(z, p);
    }
}
