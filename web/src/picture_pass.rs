//! Reference pictures on the graphics card: each a flat picture standing in
//! the scene, two triangles with its image on them, drawn into the
//! viewport's scene pass after the model (the desktop's
//! `render/references.rs` and its shader, `render/references.wgsl`, which is
//! used as it is). A picture behind is hidden wherever the model is in front
//! of it; one In Front is drawn over the model, see-through by its opacity.
//! Only the viewport draws them: Save Picture and the shadows leave them out,
//! as the desktop's renders do.
//!
//! Each image is sent once, with its smaller copies for when it is far away
//! (mipmaps, made here on the processor), and kept by the picture's number
//! until the picture goes.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use glam::Vec3;
use wgpu::util::DeviceExt;

use super::{COLOR, DEPTH, SAMPLES};

/// One picture as this frame draws it.
#[derive(Clone)]
pub struct Draw {
    /// The picture's number (`references::Picture::id`), which its image is kept by.
    pub id: u64,
    pub image: Arc<image::RgbaImage>,
    pub corners: [Vec3; 4],
    pub uvs: [[f32; 2]; 4],
    pub opacity: f32,
    pub in_front: bool,
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    uv: [f32; 2],
    opacity: f32,
}

struct Kept {
    group: wgpu::BindGroup,
    used: bool,
}

pub struct Pass {
    behind: wgpu::RenderPipeline,
    in_front: wgpu::RenderPipeline,
    uniforms: wgpu::Buffer,
    uniform_group: wgpu::BindGroup,
    image_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    images: HashMap<u64, Kept>,
    vertices: Option<wgpu::Buffer>,
    /// What to draw this frame: (picture id, first vertex, in front).
    draws: Vec<(u64, u32, bool)>,
}

impl Pass {
    pub fn new(device: &wgpu::Device) -> Pass {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reference pictures"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../../src/render/references.wgsl").into()),
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reference uniforms"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
                count: None,
            }],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reference image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reference uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reference uniforms"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reference pictures"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&image_layout)],
            immediate_size: 0,
        });
        let attributes = wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32];
        let pipeline = |compare: wgpu::CompareFunction| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("reference pictures"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_reference"),
                    compilation_options: Default::default(),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<Vertex>() as u64,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attributes,
                    })],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_reference"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: COLOR,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                // Seen only from the side it faces: it never stands between
                // the eye and the model as the view goes round.
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                // Reverse depth: nearer is greater. Tested, never written.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(compare),
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reference sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Pass {
            behind: pipeline(wgpu::CompareFunction::GreaterEqual),
            in_front: pipeline(wgpu::CompareFunction::Always),
            uniforms,
            uniform_group,
            image_layout,
            sampler,
            images: HashMap::new(),
            vertices: None,
            draws: Vec::new(),
        }
    }

    /// Sends what changed: images new this frame, and every picture's corners.
    pub fn prepare(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, view_proj: [[f32; 4]; 4], pictures: &[Draw]) {
        for kept in self.images.values_mut() {
            kept.used = false;
        }
        self.draws.clear();
        let mut vertices: Vec<Vertex> = Vec::new();
        for p in pictures.iter().filter(|p| p.opacity > 0.0) {
            if !self.images.contains_key(&p.id) {
                let group = self.send(device, queue, &p.image);
                self.images.insert(p.id, Kept { group, used: true });
            }
            if let Some(kept) = self.images.get_mut(&p.id) {
                kept.used = true;
            }
            self.draws.push((p.id, vertices.len() as u32, p.in_front));
            // Bottom left, bottom right, top right, top left: two triangles facing the view.
            for i in [0usize, 1, 2, 0, 2, 3] {
                vertices.push(Vertex { position: p.corners[i].to_array(), uv: p.uvs[i], opacity: p.opacity.clamp(0.0, 1.0) });
            }
        }
        self.images.retain(|_, kept| kept.used);
        if vertices.is_empty() {
            return;
        }
        queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&view_proj));
        self.vertices = Some(device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("reference corners"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        }));
    }

    /// Draws them into the scene pass, after the model: those behind first,
    /// then those in front.
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>) {
        let Some(vertices) = &self.vertices else { return };
        if self.draws.is_empty() {
            return;
        }
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.set_bind_group(0, &self.uniform_group, &[]);
        for front in [false, true] {
            pass.set_pipeline(if front { &self.in_front } else { &self.behind });
            for &(id, first, in_front) in self.draws.iter().filter(|d| d.2 == front) {
                let _ = in_front;
                let Some(kept) = self.images.get(&id) else { continue };
                pass.set_bind_group(1, &kept.group, &[]);
                pass.draw(first..first + 6, 0..1);
            }
        }
    }

    /// The image on the card, premultiplied by its alpha, with its mipmaps,
    /// no larger than the card takes.
    fn send(&self, device: &wgpu::Device, queue: &wgpu::Queue, image: &image::RgbaImage) -> wgpu::BindGroup {
        let largest = device.limits().max_texture_dimension_2d.max(1);
        let mut level = image.clone();
        let (w, h) = level.dimensions();
        if w.max(h) > largest {
            let scale = largest as f32 / w.max(h) as f32;
            let (nw, nh) = (((w as f32 * scale) as u32).max(1), ((h as f32 * scale) as u32).max(1));
            level = image::imageops::resize(&level, nw, nh, image::imageops::FilterType::Triangle);
        }
        for p in level.pixels_mut() {
            let a = p[3] as u32;
            for c in 0..3 {
                p[c] = ((p[c] as u32 * a + 127) / 255) as u8;
            }
        }
        let (w, h) = level.dimensions();
        let levels = 32 - w.max(h).leading_zeros();
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reference picture"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for mip in 0..levels {
            if mip > 0 {
                let (pw, ph) = level.dimensions();
                level = image::imageops::resize(&level, (pw / 2).max(1), (ph / 2).max(1), image::imageops::FilterType::Triangle);
            }
            let (lw, lh) = level.dimensions();
            queue.write_texture(
                wgpu::TexelCopyTextureInfo { texture: &texture, mip_level: mip, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
                level.as_raw(),
                wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(4 * lw), rows_per_image: Some(lh) },
                wgpu::Extent3d { width: lw, height: lh, depth_or_array_layers: 1 },
            );
        }
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reference picture"),
            layout: &self.image_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }
}
