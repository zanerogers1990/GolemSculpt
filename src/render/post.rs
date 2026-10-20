//! The passes around the surface shading: ambient occlusion before it, and
//! depth of field (in dof.rs), bloom, vignette, the tone curve and colour
//! grading after it.

use bytemuck::{Pod, Zeroable};

use super::dof::Dof;
use super::{target_texture, Globals, Gpu, MSAA_SAMPLES};
use crate::look::Look;

/// The bent normal in rgb, how open in alpha (post.wgsl, `fs_ssao`).
const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const BLOOM_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The glow is blurred at most this many times, each at half the size.
const MAX_BLOOM_LEVELS: usize = 6;

/// Mirrors `PostU` in post.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct PostParams {
    /// xy viewport px, z tan(half fov) (0 = orthographic), w aspect
    pub viewport: [f32; 4],
    /// x orthographic half height, y occlusion radius, z the power the
    /// openness is raised to (how strong it looks), w which sample of a
    /// refined picture (its noise turns with it)
    pub ao: [f32; 4],
    /// x intensity (0 = off), y threshold, z scatter
    pub bloom: [f32; 4],
    /// x intensity * 3 (0 = off), y smoothness * 5
    pub vignette: [f32; 4],
    /// x tone curve (0 none, 1 neutral, 2 ACES, 3 PBR Neutral, 4 AgX),
    /// y grading on, z saturation, w contrast (both 1 = unchanged)
    pub grade: [f32; 4],
    /// x what the light is multiplied by before the tone curve (2 to the
    /// exposure in stops), y 1 when the backdrop is a picked colour (flat or
    /// gradient), which is shown as picked: no exposure, curve or grading,
    /// z 1 for a picture with no backdrop (alpha out is the model's coverage)
    pub exposure: [f32; 4],
    /// x 1 while the picture changes every frame (the occlusion takes fewer
    /// samples then)
    pub occlusion: [f32; 4],
    /// x film grain, y sharpening, z the colour table's strength (0 none),
    /// w its points a side
    pub extras: [f32; 4],
}

/// The colour table's bind group's shape (render/lut.rs): the table and a
/// sampler that reads between its points. Made the same wherever it is made,
/// so the renderer's table fits every picture size's composite pass.
pub fn lut_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("colour table"),
        entries: &[
            wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Texture {
                    sample_type: wgpu::TextureSampleType::Float { filterable: true },
                    view_dimension: wgpu::TextureViewDimension::D3,
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
    })
}

/// The colour table as the composite pass reads it.
pub fn lut_bind(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, table: &wgpu::TextureView) -> wgpu::BindGroup {
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("colour table"),
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("colour table"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(table) },
            wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&sampler) },
        ],
    })
}

struct Targets {
    size: (u32, u32),
    /// Occlusion scratch, then "how open" for the surface shader.
    ao_scratch: wgpu::TextureView,
    bloom_down: Vec<wgpu::TextureView>,
    bloom_up: Vec<wgpu::TextureView>,
    bind_ssao: wgpu::BindGroup,
    /// The blur's two steps: across (reading `ao`), then down (reading the scratch).
    bind_ssao_across: wgpu::BindGroup,
    bind_ssao_down: wgpu::BindGroup,
    bind_prefilter: wgpu::BindGroup,
    /// Per level from 1: across into `bloom_up`, then down into `bloom_down`.
    bind_blur: Vec<(wgpu::BindGroup, wgpu::BindGroup)>,
    /// Per level from the second smallest up to 0.
    bind_up: Vec<wgpu::BindGroup>,
    bind_composite: wgpu::BindGroup,
    /// The shaded scene and its depth buffer, kept for depth of field to
    /// build on if it is switched on.
    scene: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// `bind_prefilter` and `bind_composite` again, reading the scene as
    /// depth of field left it. Only there while that effect is on.
    bind_focused: Option<(wgpu::BindGroup, wgpu::BindGroup)>,
}

pub struct Post {
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    /// The occlusion passes' layout: the first texture read unfiltered.
    layout_ao: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipe_ssao: wgpu::RenderPipeline,
    pipe_ssao_blur: wgpu::RenderPipeline,
    pipe_prefilter: wgpu::RenderPipeline,
    pipe_blur_h: wgpu::RenderPipeline,
    pipe_blur_v: wgpu::RenderPipeline,
    pipe_up: wgpu::RenderPipeline,
    pipe_composite: wgpu::RenderPipeline,
    /// The same, into a plain target: for frames with no overlays, which
    /// then need no multisampled copy of the picture at all.
    pipe_composite_single: wgpu::RenderPipeline,
    targets: Option<Targets>,
    /// How open each pixel is (1 = fully), for the surface shader.
    pub ao: Option<wgpu::TextureView>,
    /// Depth of field. Not built until the first time it is switched on.
    dof: Option<Dof>,
}

impl Post {
    pub fn new(gpu: &Gpu) -> Post {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("post"),
            source: wgpu::ShaderSource::Wgsl(include_str!("post.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("post"),
            size: std::mem::size_of::<PostParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry = |binding, filterable| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        // `first_filterable` false for the occlusion passes, whose first
        // texture is the normals prepass: full-size floats, which are read
        // texel by texel and cannot be filtered.
        let make_layout = |label, first_filterable| {
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    texture_entry(1, first_filterable),
                    texture_entry(2, true),
                    wgpu::BindGroupLayoutEntry {
                        binding: 3,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            })
        };
        let layout = make_layout("post", true);
        let layout_ao = make_layout("post occlusion", false);
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let ao_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post occlusion"),
            bind_group_layouts: &[Some(&layout_ao)],
            immediate_size: 0,
        });
        // The composite pass reads the colour table too, as a second group.
        let tables = lut_layout(device);
        let composite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("post composite"),
            bind_group_layouts: &[Some(&layout), Some(&tables)],
            immediate_size: 0,
        });
        let pipeline = |entry: &'static str, format: wgpu::TextureFormat, samples: u32| {
            let pipeline_layout = match entry {
                "fs_composite" => &composite_layout,
                "fs_ssao" | "fs_ssao_blur" => &ao_pipeline_layout,
                _ => &pipeline_layout,
            };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_post"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("post"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Post {
            pipe_ssao: pipeline("fs_ssao", AO_FORMAT, 1),
            pipe_ssao_blur: pipeline("fs_ssao_blur", AO_FORMAT, 1),
            pipe_prefilter: pipeline("fs_bloom_prefilter", BLOOM_FORMAT, 1),
            pipe_blur_h: pipeline("fs_bloom_blur_h", BLOOM_FORMAT, 1),
            pipe_blur_v: pipeline("fs_bloom_blur_v", BLOOM_FORMAT, 1),
            pipe_up: pipeline("fs_bloom_up", BLOOM_FORMAT, 1),
            // The finished picture goes into the multisampled target the
            // overlays are then drawn on.
            pipe_composite: pipeline("fs_composite", gpu.format, MSAA_SAMPLES),
            pipe_composite_single: pipeline("fs_composite", gpu.format, 1),
            uniform,
            layout,
            layout_ao,
            sampler,
            targets: None,
            ao: None,
            dof: None,
        }
    }

    fn bind(&self, device: &wgpu::Device, a: &wgpu::TextureView, b: &wgpu::TextureView) -> wgpu::BindGroup {
        self.bind_with(device, &self.layout, a, b)
    }

    fn bind_with(
        &self,
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        a: &wgpu::TextureView,
        b: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("post"),
            layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(a) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(b) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Makes the working textures for a viewport of `size`. `normals` is the
    /// prepass (view normal and eye depth), `scene` the shaded picture and
    /// `depth` the depth buffer it was drawn with.
    pub fn resize(
        &mut self,
        gpu: &Gpu,
        size: (u32, u32),
        normals: &wgpu::TextureView,
        scene: &wgpu::TextureView,
        depth: &wgpu::TextureView,
    ) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        // Depth of field's textures were made for the old viewport; `focus`
        // makes them afresh if the effect is still on.
        if let Some(dof) = &mut self.dof {
            dof.release();
        }
        let device = &gpu.device;
        let ao = target_texture(device, "occlusion", size, AO_FORMAT, 1, true);
        let ao_scratch = target_texture(device, "occlusion scratch", size, AO_FORMAT, 1, true);

        // Half size, then half again while there is anything left to halve.
        let (mut w, mut h) = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let levels = (((w.max(h) as f32).log2() - 1.0).floor() as usize).clamp(1, MAX_BLOOM_LEVELS);
        let mut bloom_down = Vec::new();
        let mut bloom_up = Vec::new();
        for _ in 0..levels {
            bloom_down.push(target_texture(device, "bloom down", (w, h), BLOOM_FORMAT, 1, true));
            bloom_up.push(target_texture(device, "bloom up", (w, h), BLOOM_FORMAT, 1, true));
            (w, h) = ((w / 2).max(1), (h / 2).max(1));
        }

        let bind = |a: &wgpu::TextureView, b: &wgpu::TextureView| self.bind(device, a, b);
        let bind_ao = |a: &wgpu::TextureView, b: &wgpu::TextureView| self.bind_with(device, &self.layout_ao, a, b);
        let bind_blur = (1..levels)
            .map(|i| (bind(&bloom_down[i - 1], &bloom_down[i - 1]), bind(&bloom_up[i], &bloom_up[i])))
            .collect();
        // On the way up each level mixes its own blur with the result one
        // level below (the smallest level has only its blur).
        let bind_up = (0..levels.saturating_sub(1))
            .rev()
            .map(|i| {
                let low = if i == levels - 2 { &bloom_down[i + 1] } else { &bloom_up[i + 1] };
                bind(&bloom_down[i], low)
            })
            .collect();
        let result = if levels > 1 { &bloom_up[0] } else { &bloom_down[0] };
        self.targets = Some(Targets {
            size,
            // The raw pass reads only the normals; the scratch fills the
            // second slot, which must be a texture that can be filtered.
            bind_ssao: bind_ao(normals, &ao_scratch),
            bind_ssao_across: bind_ao(normals, &ao),
            bind_ssao_down: bind_ao(normals, &ao_scratch),
            bind_prefilter: bind(scene, scene),
            bind_blur,
            bind_up,
            bind_composite: bind(scene, result),
            scene: scene.clone(),
            depth: depth.clone(),
            bind_focused: None,
            ao_scratch,
            bloom_down,
            bloom_up,
        });
        self.ao = Some(ao);
    }

    /// Sets up depth of field for this frame from the Look settings, or puts
    /// its working textures away if it is off. `eye_offset` is how far behind
    /// its perspective position the camera is parked (orthographic views).
    /// Call after `resize`.
    /// `turn` turns the blur's disc of taps, for a refined picture's samples.
    pub fn focus(&mut self, gpu: &Gpu, look: &Look, globals: &Globals, eye_offset: f32, turn: f32) {
        if !look.depth_of_field {
            if let Some(dof) = &mut self.dof {
                dof.release();
            }
            if let Some(t) = &mut self.targets {
                t.bind_focused = None;
            }
            return;
        }
        let Some(t) = &self.targets else { return };
        let mut dof = self.dof.take().unwrap_or_else(|| Dof::new(gpu));
        let fresh = dof.resize(gpu, t.size, &t.scene, &t.depth);
        dof.set_params(gpu, look, globals, eye_offset, turn);
        let bind_focused = match dof.focused() {
            // Bloom and the finished picture read the blurred scene in place
            // of the sharp one.
            Some(focused) if fresh || t.bind_focused.is_none() => {
                let bloom = if t.bloom_down.len() > 1 { &t.bloom_up[0] } else { &t.bloom_down[0] };
                Some((self.bind(&gpu.device, focused, focused), self.bind(&gpu.device, focused, bloom)))
            }
            _ => None,
        };
        self.dof = Some(dof);
        if let (Some(groups), Some(t)) = (bind_focused, &mut self.targets) {
            t.bind_focused = Some(groups);
        }
    }

    pub fn set_params(&self, gpu: &Gpu, params: &PostParams) {
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(params));
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        to: &wgpu::TextureView,
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
        variant: u32,
    ) {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: to,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, variant..variant + 1);
    }

    /// Works out the ambient occlusion from the normals prepass.
    pub fn occlusion(&self, encoder: &mut wgpu::CommandEncoder) {
        let (Some(t), Some(ao)) = (&self.targets, &self.ao) else { return };
        // Raw into `ao`, blurred across into the scratch, then down back
        // into `ao`.
        Self::pass(encoder, ao, &self.pipe_ssao, &t.bind_ssao, 0);
        Self::pass(encoder, &t.ao_scratch, &self.pipe_ssao_blur, &t.bind_ssao_across, 0);
        Self::pass(encoder, ao, &self.pipe_ssao_blur, &t.bind_ssao_down, 1);
    }

    /// Turns the shaded scene into the finished picture in `to`, which is
    /// multisampled (for overlays to be drawn on) or, with `single`, not,
    /// graded by the colour table `lut` (see `lut_bind`).
    pub fn finish(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        to: &wgpu::TextureView,
        bloom: bool,
        single: bool,
        lut: &wgpu::BindGroup,
    ) {
        let Some(t) = &self.targets else { return };
        // Depth of field comes first, as it does in the Unity pipeline: what
        // glows is the scene as the lens sees it.
        let (bind_prefilter, bind_composite) = match (&self.dof, &t.bind_focused) {
            (Some(dof), Some((prefilter, composite))) => {
                dof.draw(encoder);
                (prefilter, composite)
            }
            _ => (&t.bind_prefilter, &t.bind_composite),
        };
        if bloom {
            Self::pass(encoder, &t.bloom_down[0], &self.pipe_prefilter, bind_prefilter, 0);
            for (i, (across, down)) in t.bind_blur.iter().enumerate() {
                Self::pass(encoder, &t.bloom_up[i + 1], &self.pipe_blur_h, across, 0);
                Self::pass(encoder, &t.bloom_down[i + 1], &self.pipe_blur_v, down, 0);
            }
            let levels = t.bloom_down.len();
            for (k, group) in t.bind_up.iter().enumerate() {
                Self::pass(encoder, &t.bloom_up[levels - 2 - k], &self.pipe_up, group, 0);
            }
        }
        let composite = if single { &self.pipe_composite_single } else { &self.pipe_composite };
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("post composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: to,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
            })],
            ..Default::default()
        });
        pass.set_pipeline(composite);
        pass.set_bind_group(0, bind_composite, &[]);
        pass.set_bind_group(1, lut, &[]);
        pass.draw(0..3, 0..1);
    }
}

#[cfg(test)]
mod shader_tests {
    #[test]
    fn the_post_shader_compiles() {
        crate::render::paint::validate(include_str!("post.wgsl")).unwrap();
    }
}
