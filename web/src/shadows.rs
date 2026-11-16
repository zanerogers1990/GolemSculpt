//! Shadows and ambient occlusion on the Clay, Metal, Wood and Plain
//! finishes, as the desktop draws them (src/render: the key light's shadow
//! map in mod.rs and sculpt_pbr.wgsl, the occlusion in post.rs and
//! post.wgsl), in the WebGPU builds only. WebGL2 lacks what the soft shadow
//! reads its map with and would be too slow for the extra passes, so there
//! the finishes keep their lighter look (shade_off.wgsl) and the Look tab
//! greys the two switches out.
//!
//! Ahead of the viewport's own pass, in the same encoder:
//! - the key light's shadow map, drawn again only when what it sees changes
//!   (a stroke, an object moved, the light turned), small (512) while it
//!   changes frame after frame and full size (2048) once it settles, as the
//!   desktop's `shadow_key` and `QUICK_SHADOW` do;
//! - a prepass of view normal and eye depth, the desktop's ground-truth
//!   occlusion over it and its two blur passes, drawn again only when the
//!   view or the model changes, with fewer samples while they keep changing.
//!
//! A child of render.rs, so it reaches the renderer's buffers.

use std::collections::HashMap;
use std::ops::Range;
use std::sync::atomic::{AtomicBool, Ordering};

use eframe::wgpu;
use glam::{Mat4, Vec3, Vec4};

use super::finish::Finishes;
use super::{Globals, Gpu, MeshBuffers, OBJECT_STRIDE, Viewport};

/// The shadow map's side once it holds still (the desktop's default
/// Shadow Detail, "Sharp"), and while it changes frame after frame.
const SHADOW_SIZE: u32 = 2048;
const QUICK_SHADOW: u32 = 512;
const SHADOW_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// View normal and eye depth: full floats, as the desktop keeps them.
const PREPASS_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;
const AO_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// The desktop's occlusion reach (world units) and the power its openness
/// is raised to, and what direct light keeps of it (`env_params.w`).
const AO_RADIUS: f32 = 0.15;
const AO_POWER: f32 = 1.6;
const AO_DIRECT: f32 = 0.25;
/// How dark each lighting preset's key shadow gets: the desktop's
/// `LightingPreset::shadow_strength`, in the order of `lighting::LIGHTING`.
const SHADOW_STRENGTH: [f32; 8] = [0.75, 0.35, 0.25, 0.7, 0.9, 0.85, 1.0, 1.0];

/// Set once the renderer has started on WebGPU: the Look tab offers the
/// switches only then.
static AVAILABLE: AtomicBool = AtomicBool::new(false);

/// Whether this browser draws the shadows and the occlusion (it draws with WebGPU).
pub fn available() -> bool {
    AVAILABLE.load(Ordering::Relaxed)
}

/// Whether a renderer on `backend` can draw them.
pub fn supported(backend: wgpu::Backend) -> bool {
    backend == wgpu::Backend::BrowserWebGpu
}

/// The shader source joined onto shader.wgsl and finish.wgsl.
pub fn shader_part(on: bool) -> &'static str {
    if on { include_str!("shade.wgsl") } else { include_str!("shade_off.wgsl") }
}

/// What the app asks of the shading this frame (`Viewport::shade`).
#[derive(Clone, Copy, Default)]
pub struct Ask {
    pub shadows: bool,
    pub occlusion: bool,
    /// How dark the key's shadow gets (0 none, 1 black).
    pub strength: f32,
    /// The tangent of the key light's angular radius: how fast the
    /// shadow's edge softens away from what casts it.
    pub light_tan: f32,
    /// The active object's bounds, in its own space.
    pub bounds: (Vec3, Vec3),
    /// The picture changes this frame (the view moved, or the model).
    pub moving: bool,
}

impl Ask {
    /// The Look tab's settings as the renderer reads them.
    pub fn of(finishes: &Finishes, bounds: (Vec3, Vec3), moving: bool) -> Ask {
        let l = &finishes.lighting;
        let softness = l.light_size.clamp(0.0, 4.0);
        Ask {
            shadows: l.shadows,
            occlusion: finishes.effects.ambient_occlusion,
            strength: SHADOW_STRENGTH[l.preset.min(SHADOW_STRENGTH.len() - 1)],
            light_tan: (l.preset().light_size.to_radians() * 0.5 * softness).tan(),
            bounds,
            moving,
        }
    }
}

/// Tells frames where the picture changes from frames where it holds
/// still, so the app can ask for one more frame after a change and the
/// still picture is drawn at full quality.
#[derive(Default)]
pub struct Motion {
    last: Option<[[f32; 4]; 4]>,
}

impl Motion {
    /// Whether this frame differs from the last: the view moved, or the
    /// model is being sent to the card.
    pub fn update(&mut self, view_proj: [[f32; 4]; 4], uploading: bool) -> bool {
        let moved = self.last != Some(view_proj);
        self.last = Some(view_proj);
        moved || uploading
    }
}

/// The shader's `ShadeU` (shade.wgsl, shade_passes.wgsl).
#[repr(C)]
#[derive(Clone, Copy, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
struct Uniform {
    shadow_view_proj: [[f32; 4]; 4],
    shadow_params: [f32; 4],
    shadow_soft: [f32; 4],
    flags: [f32; 4],
    key: [f32; 4],
    viewport: [f32; 4],
    ao: [f32; 4],
    occlusion: [f32; 4],
    picture: [f32; 4],
}

/// The key light's shadow map fitted to the scene: the desktop's
/// `look::ShadowFit` (it cannot be compiled here).
struct ShadowFit {
    x: Vec3,
    y: Vec3,
    key: Vec3,
    centre: [f32; 2],
    near: f32,
    side: f32,
    depth: f32,
}

impl ShadowFit {
    fn around(key: Vec3, min: Vec3, max: Vec3) -> ShadowFit {
        let (min, max) =
            if min.is_finite() && max.is_finite() && min.cmple(max).all() { (min, max) } else { (Vec3::splat(-0.5), Vec3::splat(0.5)) };
        let up = if key.y.abs() > 0.99 { Vec3::Z } else { Vec3::Y };
        let x = up.cross(key).normalize();
        let y = key.cross(x);
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        for i in 0..8 {
            let p = Vec3::new(
                if i & 1 == 0 { min.x } else { max.x },
                if i & 2 == 0 { min.y } else { max.y },
                if i & 4 == 0 { min.z } else { max.z },
            );
            let q = Vec3::new(p.dot(x), p.dot(y), p.dot(key));
            lo = lo.min(q);
            hi = hi.max(q);
        }
        let extent = (hi - lo).max(Vec3::splat(1e-4));
        ShadowFit {
            x,
            y,
            key,
            centre: [(lo.x + hi.x) * 0.5, (lo.y + hi.y) * 0.5],
            near: hi.z,
            side: extent.x.max(extent.y) * 1.1,
            depth: extent.z * 2.0,
        }
    }

    fn view_proj(&self) -> Mat4 {
        let half = self.side * 0.5;
        let rows = [
            (self.x / half).extend(-self.centre[0] / half),
            (self.y / half).extend(-self.centre[1] / half),
            (-self.key / self.depth).extend(0.25 + self.near / self.depth),
            Vec4::W,
        ];
        Mat4::from_cols(rows[0], rows[1], rows[2], rows[3]).transpose()
    }
}

/// The textures that go with the viewport's size.
struct Targets {
    size: [u32; 2],
    prepass: wgpu::TextureView,
    prepass_depth: wgpu::TextureView,
    /// The occlusion ends up in `a`; `b` is the blur's scratch.
    a: wgpu::TextureView,
    b: wgpu::TextureView,
    /// The post passes' groups: the prepass with `b`, and with `a`.
    reads_b: wgpu::BindGroup,
    reads_a: wgpu::BindGroup,
    /// What the viewport's own pass reads (group 2).
    main: wgpu::BindGroup,
}

pub struct Shade {
    uniform: wgpu::Buffer,
    shadow: wgpu::TextureView,
    compare: wgpu::Sampler,
    point: wgpu::Sampler,
    linear: wgpu::Sampler,
    /// Group 2 of the viewport's own pipeline (read by shade.wgsl).
    pub(super) main_layout: wgpu::BindGroupLayout,
    post_layout: wgpu::BindGroupLayout,
    geometry_group: wgpu::BindGroup,
    shadow_pipeline: wgpu::RenderPipeline,
    prepass_pipeline: wgpu::RenderPipeline,
    ssao_pipeline: wgpu::RenderPipeline,
    blur_across: wgpu::RenderPipeline,
    blur_down: wgpu::RenderPipeline,
    targets: Targets,
    /// What the shadow map and the occlusion hold, and whether the map is
    /// the quick small one.
    shadow_drawn: Option<Vec<u8>>,
    shadow_quick: bool,
    ao_drawn: Option<Vec<u8>>,
    /// Bumped each frame the model is sent to the card.
    geometry: u64,
    /// The other objects' bounds, by `Drawn::id`.
    bounds: HashMap<u64, (Vec3, Vec3)>,
    /// This frame's uniform, written again after a picture to save
    /// borrowed it (`still`).
    current: Uniform,
}

impl Shade {
    /// Made with the renderer on WebGPU; `globals` is the viewport's globals
    /// buffer and `object_layout` its per-object group's layout.
    pub fn new(device: &wgpu::Device, globals: &wgpu::Buffer, object_layout: &wgpu::BindGroupLayout) -> Shade {
        AVAILABLE.store(true, Ordering::Relaxed);
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("shade passes"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shade_passes.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shade"),
            size: std::mem::size_of::<Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let shadow = device
            .create_texture(&wgpu::TextureDescriptor {
                label: Some("shadow map"),
                size: wgpu::Extent3d { width: SHADOW_SIZE, height: SHADOW_SIZE, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: SHADOW_FORMAT,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            })
            .create_view(&Default::default());
        let clamp = wgpu::AddressMode::ClampToEdge;
        let compare = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow compare"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        let point = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow gather"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            ..Default::default()
        });
        let linear = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("occlusion"),
            address_mode_u: clamp,
            address_mode_v: clamp,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let frag = wgpu::ShaderStages::FRAGMENT;
        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer { ty: wgpu::BufferBindingType::Uniform, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let texture_entry = |binding, sample_type| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: frag,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled: false },
            count: None,
        };
        let sampler_entry = |binding, kind| wgpu::BindGroupLayoutEntry { binding, visibility: frag, ty: wgpu::BindingType::Sampler(kind), count: None };
        let main_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shade main layout"),
            entries: &[
                uniform_entry(0, frag),
                texture_entry(1, wgpu::TextureSampleType::Depth),
                sampler_entry(2, wgpu::SamplerBindingType::Comparison),
                sampler_entry(3, wgpu::SamplerBindingType::NonFiltering),
                texture_entry(4, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(5, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let geometry_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shade geometry layout"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT), uniform_entry(1, wgpu::ShaderStages::VERTEX_FRAGMENT)],
        });
        let post_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("shade post layout"),
            entries: &[
                uniform_entry(1, frag),
                texture_entry(2, wgpu::TextureSampleType::Float { filterable: false }),
                texture_entry(3, wgpu::TextureSampleType::Float { filterable: true }),
                sampler_entry(4, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let geometry_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shade geometry"),
            layout: &geometry_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: uniform.as_entire_binding() },
            ],
        });

        let geometry_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shade geometry pipeline layout"),
            bind_group_layouts: &[Some(&geometry_layout), Some(object_layout)],
            immediate_size: 0,
        });
        let attributes = [
            [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 0 }],
            [wgpu::VertexAttribute { format: wgpu::VertexFormat::Float32x3, offset: 0, shader_location: 1 }],
        ];
        let buffers: Vec<Option<wgpu::VertexBufferLayout>> = attributes
            .iter()
            .map(|a| Some(wgpu::VertexBufferLayout { array_stride: 12, step_mode: wgpu::VertexStepMode::Vertex, attributes: a }))
            .collect();
        let shadow_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow map"),
            layout: Some(&geometry_pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_shadow"), compilation_options: Default::default(), buffers: &buffers },
            fragment: None,
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let prepass_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("normals prepass"),
            layout: Some(&geometry_pipeline_layout),
            vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_prepass"), compilation_options: Default::default(), buffers: &buffers },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_prepass"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState { format: PREPASS_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            // Reverse depth, as the viewport's own pass.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: SHADOW_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let post_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("shade post pipeline layout"),
            bind_group_layouts: &[Some(&post_layout)],
            immediate_size: 0,
        });
        let post = |label, entry| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&post_pipeline_layout),
                vertex: wgpu::VertexState { module: &shader, entry_point: Some("vs_post"), compilation_options: Default::default(), buffers: &[] },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format: AO_FORMAT, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let ssao_pipeline = post("occlusion", "fs_ssao");
        let blur_across = post("occlusion blur across", "fs_blur_across");
        let blur_down = post("occlusion blur down", "fs_blur_down");

        let targets = Targets::new(device, [1, 1], &post_layout, &main_layout, &uniform, &shadow, [&compare, &point, &linear]);
        Shade {
            targets,
            uniform,
            shadow,
            compare,
            point,
            linear,
            main_layout,
            post_layout,
            geometry_group,
            shadow_pipeline,
            prepass_pipeline,
            ssao_pipeline,
            blur_across,
            blur_down,
            shadow_drawn: None,
            shadow_quick: false,
            ao_drawn: None,
            geometry: 0,
            bounds: HashMap::new(),
            current: bytemuck::Zeroable::zeroed(),
        }
    }

    fn make_targets(&self, device: &wgpu::Device, size: [u32; 2]) -> Targets {
        let samplers = [&self.compare, &self.point, &self.linear];
        Targets::new(device, size, &self.post_layout, &self.main_layout, &self.uniform, &self.shadow, samplers)
    }

    /// What the viewport's pass binds as group 2.
    pub(super) fn main_group(&self) -> &wgpu::BindGroup {
        &self.targets.main
    }

    /// Works out this frame's shading and draws whatever of the shadow map
    /// and the occlusion has changed into `encoder`, ahead of the viewport's
    /// pass. `draws` and `runs` are what that pass draws.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn prepare(
        &mut self,
        gpu: &Gpu,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        vp: &Viewport,
        globals: &Globals,
        draws: &[(u64, u32)],
        runs: &[Range<u32>],
        size: [u32; 2],
    ) {
        if self.targets.size != size {
            self.targets = self.make_targets(device, size);
            self.ao_drawn = None;
        }
        if vp.upload.is_some() {
            self.geometry += 1;
        }
        let ask = &vp.shade;
        let lit = vp.finish.kind[0] > 0.5;
        let shadows = lit && ask.shadows;
        let occlusion = lit && ask.occlusion;

        // What every drawn thing is and where it stands: the shadow map and
        // the occlusion are drawn again only when this (or the light, or the
        // view) changes.
        let mut scene: Vec<u8> = Vec::new();
        let mut push = |bytes: &[u8]| scene.extend_from_slice(bytes);
        push(bytemuck::bytes_of(&self.geometry));
        push(&[vp.show_model as u8, globals.params[2].to_bits() as u8]);
        push(bytemuck::bytes_of(&vp.model.to_cols_array()));
        for c in &vp.copies {
            push(bytemuck::bytes_of(&c.to_cols_array()));
        }
        for o in &vp.others {
            push(bytemuck::bytes_of(&o.drawn.id));
            push(bytemuck::bytes_of(&o.model.to_cols_array()));
        }
        for p in &vp.previews {
            push(bytemuck::bytes_of(&p.mesh.id()));
            push(bytemuck::bytes_of(&p.mesh.version()));
            for t in &p.transforms {
                push(bytemuck::bytes_of(&t.to_cols_array()));
            }
        }

        // Everything shown, in the world: what the shadow map is fitted round.
        let mut lo = Vec3::splat(f32::INFINITY);
        let mut hi = Vec3::splat(f32::NEG_INFINITY);
        let mut take = |model: &Mat4, (min, max): (Vec3, Vec3)| {
            if !(min.is_finite() && max.is_finite()) {
                return;
            }
            for i in 0..8 {
                let p = Vec3::new(
                    if i & 1 == 0 { min.x } else { max.x },
                    if i & 2 == 0 { min.y } else { max.y },
                    if i & 4 == 0 { min.z } else { max.z },
                );
                let w = model.transform_point3(p);
                lo = lo.min(w);
                hi = hi.max(w);
            }
        };
        if vp.show_model {
            take(&vp.model, ask.bounds);
            for c in &vp.copies {
                take(c, ask.bounds);
            }
        }
        if shadows {
            self.bounds.retain(|id, _| vp.others.iter().any(|o| o.drawn.id == *id));
            for o in &vp.others {
                let b = *self.bounds.entry(o.drawn.id).or_insert_with(|| {
                    let points: &[[f32; 3]] = bytemuck::cast_slice(&o.drawn.positions);
                    points.iter().fold((Vec3::splat(f32::INFINITY), Vec3::splat(f32::NEG_INFINITY)), |(lo, hi), &p| {
                        (lo.min(Vec3::from(p)), hi.max(Vec3::from(p)))
                    })
                });
                take(&o.model, b);
            }
        }

        let key = Vec3::from_slice(&vp.finish.light_dir[0][..3]).normalize_or(Vec3::Y);
        let fit = ShadowFit::around(key, lo, hi);
        let mut u: Uniform = bytemuck::Zeroable::zeroed();
        u.shadow_view_proj = fit.view_proj().to_cols_array_2d();
        let texel = fit.side / SHADOW_SIZE as f32;
        u.shadow_params = [1.0 / SHADOW_SIZE as f32, 0.1 * texel * 2.5, 0.5 * texel * 2.5, ask.moving as u8 as f32];
        u.shadow_soft = [fit.side, fit.depth, ask.light_tan, 1.0];
        u.flags = [shadows as u8 as f32, ask.strength, occlusion as u8 as f32, AO_DIRECT];
        u.key = key.extend(0.0).to_array();
        let ortho = globals.eye[3] > 0.5;
        let aspect = size[0] as f32 / size[1].max(1) as f32;
        let tan_half_fov = globals.up[3];
        // An orthographic view's half height, from its projection (the view
        // has no reach then: `Globals::camera`).
        // view_proj = proj * view, and the view part only turns: the length
        // of the second row's first three numbers is 1 / the half height.
        let half_height = if ortho {
            let m = Mat4::from_cols_array_2d(&globals.view_proj);
            1.0 / m.row(1).truncate().length().max(1e-9)
        } else {
            0.0
        };
        u.viewport = [size[0] as f32, size[1] as f32, if ortho { 0.0 } else { tan_half_fov }, aspect];
        u.ao = [half_height, AO_RADIUS, AO_POWER, 0.0];
        u.occlusion = [ask.moving as u8 as f32, 0.0, 0.0, 0.0];
        u.picture = [size[0] as f32, size[1] as f32, 0.0, 0.0];

        // The shadow map: drawn small while it changes frame after frame,
        // full size once it settles (the desktop's quick shadow).
        let mut shadow_key = Vec::new();
        if shadows {
            shadow_key.extend_from_slice(bytemuck::bytes_of(&u.shadow_view_proj));
            shadow_key.extend_from_slice(&scene);
        }
        let shadow_key = shadows.then_some(shadow_key);
        let changes = shadows && shadow_key != self.shadow_drawn;
        let quick = changes && ask.moving;
        let draw_shadow = changes || (shadows && self.shadow_quick && !quick);
        let current_quick = if draw_shadow { quick } else { self.shadow_quick };
        if shadows && current_quick {
            let grow = SHADOW_SIZE as f32 / QUICK_SHADOW as f32;
            u.shadow_params[0] = 1.0 / QUICK_SHADOW as f32;
            u.shadow_params[1] *= grow;
            u.shadow_params[2] *= grow;
            u.shadow_soft[3] = 1.0 / grow;
        }
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
        self.current = u;

        if draw_shadow {
            self.shadow_drawn = shadow_key;
            self.shadow_quick = quick;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow map"),
                color_attachments: &[],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.shadow,
                    depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(1.0), store: wgpu::StoreOp::Store }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            if quick {
                let side = QUICK_SHADOW as f32;
                pass.set_viewport(0.0, 0.0, side, side, 0.0, 1.0);
            }
            pass.set_pipeline(&self.shadow_pipeline);
            pass.set_bind_group(0, &self.geometry_group, &[]);
            draw_geometry(gpu, &mut pass, draws, None);
        }

        // The occlusion: again only when the view or the scene changed.
        let ao_key = occlusion.then(|| {
            let mut k = scene.clone();
            k.extend_from_slice(bytemuck::bytes_of(&globals.view_proj));
            k.push(ask.moving as u8);
            k
        });
        if occlusion && ao_key != self.ao_drawn {
            self.ao_drawn = ao_key;
            let t = &self.targets;
            {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("normals prepass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &t.prepass,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &t.prepass_depth,
                        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(&self.prepass_pipeline);
                pass.set_bind_group(0, &self.geometry_group, &[]);
                draw_geometry(gpu, &mut pass, draws, Some(runs));
            }
            let post = |encoder: &mut wgpu::CommandEncoder, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup, out: &wgpu::TextureView| {
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("occlusion"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: out,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::WHITE), store: wgpu::StoreOp::Store },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes: None,
                    occlusion_query_set: None,
                    multiview_mask: None,
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                pass.draw(0..3, 0..1);
            };
            post(encoder, &self.ssao_pipeline, &t.reads_b, &t.a);
            post(encoder, &self.blur_across, &t.reads_a, &t.b);
            post(encoder, &self.blur_down, &t.reads_b, &t.a);
        }
        if !occlusion {
            self.ao_drawn = None;
        }
        if !shadows {
            self.shadow_drawn = None;
        }
    }

    /// A picture to save is drawn at its own size: the occlusion is looked
    /// up by its share of the picture rather than by pixel. Call
    /// [`Self::restore`] once that picture's commands are sent.
    pub(super) fn for_picture(&self, queue: &wgpu::Queue, size: [u32; 2]) {
        let mut u = self.current;
        u.picture = [size[0] as f32, size[1] as f32, 0.0, 0.0];
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
    }

    pub(super) fn restore(&self, queue: &wgpu::Queue) {
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&self.current));
    }
}

impl Targets {
    /// The textures and groups for a viewport `size` pixels big.
    #[allow(clippy::too_many_arguments)]
    fn new(
        device: &wgpu::Device,
        size: [u32; 2],
        post_layout: &wgpu::BindGroupLayout,
        main_layout: &wgpu::BindGroupLayout,
        uniform: &wgpu::Buffer,
        shadow: &wgpu::TextureView,
        [compare, point, linear]: [&wgpu::Sampler; 3],
    ) -> Targets {
        let texture = |label, format| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let prepass = texture("normals prepass", PREPASS_FORMAT);
        let prepass_depth = texture("normals prepass depth", SHADOW_FORMAT);
        let a = texture("occlusion", AO_FORMAT);
        let b = texture("occlusion scratch", AO_FORMAT);
        let post_group = |reads: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("shade post"),
                layout: post_layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 1, resource: uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(&prepass) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(reads) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(linear) },
                ],
            })
        };
        let reads_b = post_group(&b);
        let reads_a = post_group(&a);
        let main = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("shade main"),
            layout: main_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: uniform.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(shadow) },
                wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(compare) },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(point) },
                wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(&a) },
                wgpu::BindGroupEntry { binding: 5, resource: wgpu::BindingResource::Sampler(linear) },
            ],
        });
        Targets { size, prepass, prepass_depth, a, b, reads_b, reads_a, main }
    }
}

/// Draws every drawn thing's triangles (positions and normals only) into a
/// pass whose pipeline is set: the model (only `runs` of it when given),
/// its live mirror copies, the other objects and the tools' previews.
fn draw_geometry(gpu: &Gpu, pass: &mut wgpu::RenderPass<'_>, draws: &[(u64, u32)], runs: Option<&[Range<u32>]>) {
    let bind = |pass: &mut wgpu::RenderPass<'_>, mesh: &MeshBuffers, slot: u32| {
        pass.set_bind_group(1, &gpu.object_group, &[slot * OBJECT_STRIDE as u32]);
        pass.set_vertex_buffer(0, mesh.positions.slice(..));
        pass.set_vertex_buffer(1, mesh.normals.slice(..));
        pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
    };
    if let Some(mesh) = gpu.mesh.as_ref().filter(|m| m.index_count > 0 && gpu.show_model) {
        bind(pass, mesh, 0);
        match runs {
            Some(runs) => {
                for run in runs {
                    pass.draw_indexed(run.clone(), 0, 0..1);
                }
            }
            None => pass.draw_indexed(0..mesh.index_count, 0, 0..1),
        }
        for &slot in &gpu.copy_slots {
            bind(pass, mesh, slot);
            pass.draw_indexed(0..mesh.index_count, 0, 0..1);
        }
    }
    for &(id, slot) in draws {
        if let Some(p) = gpu.previews.get(&id).filter(|p| p.mesh.index_count > 0) {
            bind(pass, &p.mesh, slot);
            pass.draw_indexed(0..p.mesh.index_count, 0, 0..1);
        }
    }
}
