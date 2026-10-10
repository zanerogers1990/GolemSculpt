//! GPU side: device/surface setup, the viewport pipelines, and the mesh
//! buffers. After a brush dab only the vertex ranges that changed are
//! re-uploaded; the mesh keeps its vertices in spatial order so those ranges
//! are few and contiguous.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::doc::Options;
use crate::look::{Finish, LookUniform, Tonemap, SHADOW_SIZE};
use crate::mesh::Mesh;

mod dof;
pub mod env;
mod mips;
pub mod flash;
pub mod post;
pub mod preview;
pub mod spheres;
pub mod wire;

const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
const GBUF_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// The scene is shaded in linear colour with room above white, so glow can
/// bloom and the tone curve has something to roll off.
const HDR_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Full floats: plastic thickness is a small difference of two eye depths.
const BACK_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::R32Float;
/// Ambient occlusion reach and strength. The Unity app's 0.3 and 0.4, with
/// the reach halved because shapes here are built at half its size.
const AO_RADIUS: f32 = 0.15;
const AO_INTENSITY: f32 = 0.4;
pub const MSAA_SAMPLES: u32 = 4;

pub struct Gpu {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub surface: wgpu::Surface<'static>,
    pub config: wgpu::SurfaceConfiguration,
    /// Format the pipelines render to (never sRGB; shaders encode manually,
    /// which is also what the UI renderer expects).
    pub format: wgpu::TextureFormat,
    pub adapter_name: String,
}

impl Gpu {
    pub async fn new(window: Arc<Window>) -> Result<Gpu, String> {
        let size = window.inner_size();
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let surface = instance.create_surface(window).map_err(|e| format!("create surface: {e}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                ..Default::default()
            })
            .await
            .map_err(|e| format!("No usable graphics adapter: {e}"))?;
        let info = adapter.get_info();
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("sculpt"),
                required_limits: wgpu::Limits {
                    // Dense meshes need index buffers beyond the 256 MB default.
                    max_buffer_size: adapter.limits().max_buffer_size,
                    ..Default::default()
                },
                memory_hints: wgpu::MemoryHints::Performance,
                ..Default::default()
            })
            .await
            .map_err(|e| format!("Could not open the graphics device: {e}"))?;

        let caps = surface.get_capabilities(&adapter);
        let surface_format = caps
            .formats
            .iter()
            .copied()
            .find(|f| {
                matches!(f, wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm)
            })
            .or_else(|| caps.formats.first().copied())
            .ok_or("The window surface reports no pixel formats")?;
        let format = surface_format.remove_srgb_suffix();
        let mut config = surface
            .get_default_config(&adapter, size.width.max(1), size.height.max(1))
            .ok_or("The window surface is not supported by this adapter")?;
        config.format = surface_format;
        config.view_formats = if format != surface_format { vec![format] } else { vec![] };
        // Vsync with a single queued frame: no tearing, minimal pointer lag.
        config.present_mode = wgpu::PresentMode::AutoVsync;
        config.desired_maximum_frame_latency = 1;
        surface.configure(&device, &config);

        Ok(Gpu {
            device,
            queue,
            surface,
            config,
            format,
            adapter_name: format!("{} ({:?})", info.name, info.backend),
        })
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    pub fn max_texture_side(&self) -> usize {
        self.device.limits().max_texture_dimension_2d as usize
    }
}

/// Uniforms shared by every pipeline; mirrors `Globals` in shader.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub cam_pos: [f32; 4],
    pub cam_right: [f32; 4],
    pub cam_up: [f32; 4],
    pub cam_forward: [f32; 4],
    pub base_color: [f32; 4],
    pub bg_bottom: [f32; 4],
    pub bg_top: [f32; 4],
    pub shading: [f32; 4],
    pub symmetry: [f32; 4],
    pub viewport: [f32; 4],
    pub light_dir: [[f32; 4]; 3],
    pub light_color: [[f32; 4]; 3],
    pub ambient_sky: [f32; 4],
    pub ambient_equator: [f32; 4],
    pub ambient_ground: [f32; 4],
}

struct Targets {
    size: (u32, u32),
    /// The shaded scene, multisampled, and what it resolves into.
    msaa: wgpu::TextureView,
    scene: wgpu::TextureView,
    depth: wgpu::TextureView,
    /// The finished picture, multisampled again so overlays drawn on it
    /// (the SSphere armature) keep smooth edges against the scene's depth.
    ldr: wgpu::TextureView,
    gbuf: wgpu::TextureView,
    gbuf_depth: wgpu::TextureView,
    /// Eye depth of the nearest back face, for the lure plastic.
    back: wgpu::TextureView,
}

/// One object to draw: which mesh, and its per-object shader input (the
/// object's matrix by columns, with the unused last element set to 1 for the
/// active object).
#[derive(Clone, Copy)]
pub struct DrawItem {
    pub mesh: u64,
    pub model: [[f32; 4]; 4],
}

struct MeshBuffers {
    /// `Mesh::uid` of what was uploaded.
    uid: u64,
    positions: wgpu::Buffer,
    normals: wgpu::Buffer,
    mask: wgpu::Buffer,
    curv: wgpu::Buffer,
    /// The faces that show: every triangle of the mesh, less the hidden ones.
    indices: wgpu::Buffer,
    index_count: u32,
    /// `Mesh::visibility_version` of the faces in `indices`.
    visibility: u64,
}

/// An index buffer holding `corners`. It is never made empty: a mesh with
/// every face hidden still has to bind one, and a buffer of no size cannot
/// be bound.
fn index_buffer(gpu: &Gpu, corners: &[u32]) -> wgpu::Buffer {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("indices"),
        size: corners.len().max(3) as u64 * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&buffer, 0, bytemuck::cast_slice(corners));
    buffer
}

pub struct Renderer {
    format: wgpu::TextureFormat,
    globals: wgpu::Buffer,
    look: wgpu::Buffer,
    layout_main: wgpu::BindGroupLayout,
    bind_pre: wgpu::BindGroup,
    bind_main: Option<wgpu::BindGroup>,
    pipe_background: wgpu::RenderPipeline,
    pipe_mesh: wgpu::RenderPipeline,
    pipe_prepass: wgpu::RenderPipeline,
    pipe_shadow: wgpu::RenderPipeline,
    pipe_back: wgpu::RenderPipeline,
    matcap: wgpu::TextureView,
    sampler: wgpu::Sampler,
    shadow: wgpu::TextureView,
    shadow_sampler: wgpu::Sampler,
    env: env::Env,
    /// The `env.generation` the main bind group was built against.
    env_seen: u32,
    post: post::Post,
    targets: Option<Targets>,
    /// Keyed by the scene's mesh id.
    meshes: HashMap<u64, MeshBuffers>,
    /// Per-object vertex data for the current frame, and how many it holds.
    objects: Option<(wgpu::Buffer, usize)>,
    /// Bytes sent to the GPU by the last mesh update (for the stats readout).
    pub last_upload_bytes: usize,
    /// Whether the current finish reads per-vertex curvature; meshes only
    /// keep it up to date while this is set.
    pub want_curvature: bool,
    /// The SSphere tool's armature and skin, drawn over the scene.
    pub spheres: spheres::Layer,
    /// The hosted tools' preview meshes, drawn over the scene.
    pub previews: preview::Layer,
    /// The blue and white pulse over a picked or undone object.
    pub flash: flash::Layer,
    /// The wireframe overlay.
    pub wire: wire::Layer,
    /// Off, the pulses and the tools' previews are left out: the clean view,
    /// and pictures drawn for a recording.
    pub overlays: bool,
}

/// The renderer's working textures for a second picture size: what a
/// recording is drawn with (see capture.rs). Kept apart from the window's so
/// that drawing at one size does not rebuild what the other needs.
pub struct Side {
    targets: Option<Targets>,
    bind_main: Option<wgpu::BindGroup>,
    env_seen: u32,
    post: post::Post,
}

impl Side {
    pub fn new(gpu: &Gpu) -> Side {
        Side { targets: None, bind_main: None, env_seen: 0, post: post::Post::new(gpu) }
    }
}

pub(crate) fn target_texture(
    device: &wgpu::Device,
    label: &str,
    size: (u32, u32),
    format: wgpu::TextureFormat,
    samples: u32,
    sampled: bool,
) -> wgpu::TextureView {
    let mut usage = wgpu::TextureUsages::RENDER_ATTACHMENT;
    if sampled {
        usage |= wgpu::TextureUsages::TEXTURE_BINDING;
    }
    device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        })
        .create_view(&wgpu::TextureViewDescriptor::default())
}

fn matcap_texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &image::RgbaImage) -> wgpu::TextureView {
    let size = wgpu::Extent3d { width: image.width(), height: image.height(), depth_or_array_layers: 1 };
    // With every half size below it, so fine detail seen from a distance
    // reads an average of the matcap instead of sparkling (see mips.rs).
    let smaller = mips::below(image);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("matcap"),
        size,
        mip_level_count: 1 + smaller.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, image) in std::iter::once(image).chain(&smaller).enumerate() {
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            image.as_raw(),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(4 * image.width()),
                rows_per_image: Some(image.height()),
            },
            wgpu::Extent3d { width: image.width(), height: image.height(), depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl Renderer {
    pub fn new(gpu: &Gpu, matcap: &image::RgbaImage) -> Renderer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("viewport"),
            source: wgpu::ShaderSource::Wgsl(
                concat!(include_str!("shader.wgsl"), include_str!("sculpt_pbr.wgsl")).into(),
            ),
        });
        let uniform_buffer = |label, size| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: size as u64,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let globals = uniform_buffer("globals", std::mem::size_of::<Globals>());
        let look = uniform_buffer("look", std::mem::size_of::<LookUniform>());

        let uniform_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture_entry = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
            count: None,
        };
        let sampler_entry = |binding, ty| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(ty),
            count: None,
        };
        let smooth = wgpu::TextureSampleType::Float { filterable: true };
        let exact = wgpu::TextureSampleType::Float { filterable: false };
        let flat = wgpu::TextureViewDimension::D2;
        // The helper passes render into buffers the main pass reads, so they
        // get their own layout without those textures.
        let layout_pre = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("prepass"),
            entries: &[uniform_entry(0), uniform_entry(4)],
        });
        let layout_main = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("main"),
            entries: &[
                uniform_entry(0),
                texture_entry(1, smooth, flat),
                sampler_entry(2, wgpu::SamplerBindingType::Filtering),
                texture_entry(3, exact, flat),
                uniform_entry(4),
                texture_entry(5, smooth, wgpu::TextureViewDimension::Cube),
                sampler_entry(6, wgpu::SamplerBindingType::Filtering),
                texture_entry(7, wgpu::TextureSampleType::Depth, flat),
                sampler_entry(8, wgpu::SamplerBindingType::Comparison),
                texture_entry(9, exact, flat),
                texture_entry(10, exact, flat),
                texture_entry(11, smooth, flat),
                sampler_entry(12, wgpu::SamplerBindingType::Filtering),
            ],
        });
        let bind_pre = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("prepass"),
            layout: &layout_pre,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 4, resource: look.as_entire_binding() },
            ],
        });

        let pipeline_layout = |layout: &wgpu::BindGroupLayout| {
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: None,
                bind_group_layouts: &[Some(layout)],
                immediate_size: 0,
            })
        };
        let vertex_buffers = [
            Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![1 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 4,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![2 => Float32],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: 4,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![7 => Float32],
            }),
        ];
        // Reverse depth: nearer fragments have larger depth values.
        let depth_state = |write: bool| {
            Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(write),
                depth_compare: Some(if write {
                    wgpu::CompareFunction::GreaterEqual
                } else {
                    wgpu::CompareFunction::Always
                }),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            })
        };
        let multisample = |count| wgpu::MultisampleState { count, mask: !0, alpha_to_coverage_enabled: false };
        // Both faces are drawn; the shader tints the inside so holes are visible.
        let primitive = wgpu::PrimitiveState { cull_mode: None, ..Default::default() };

        let layout = pipeline_layout(&layout_main);
        let pipe_background = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("background"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_background"),
                compilation_options: Default::default(),
                buffers: &[],
            },
            primitive,
            depth_stencil: depth_state(false),
            multisample: multisample(MSAA_SAMPLES),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_background"),
                compilation_options: Default::default(),
                targets: &[Some(HDR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let pipe_mesh = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
            },
            primitive,
            depth_stencil: depth_state(true),
            multisample: multisample(MSAA_SAMPLES),
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(HDR_FORMAT.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        let layout = pipeline_layout(&layout_pre);
        let helper = |label, fragment: &str, format: wgpu::TextureFormat, cull_mode| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_mesh"),
                    compilation_options: Default::default(),
                    buffers: &vertex_buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode, ..Default::default() },
                depth_stencil: depth_state(true),
                multisample: multisample(1),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(format.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipe_prepass = helper("prepass", "fs_prepass", GBUF_FORMAT, None);
        // Back faces only: the far side of the model under each pixel.
        let pipe_back = helper("back depth", "fs_back_depth", BACK_FORMAT, Some(wgpu::Face::Front));
        // Depth only, from the key light; ordinary (not reverse) depth.
        let pipe_shadow = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("shadow"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_shadow"),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
            },
            primitive,
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Less),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: multisample(1),
            fragment: None,
            multiview_mask: None,
            cache: None,
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("matcap"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let shadow = target_texture(device, "shadow", (SHADOW_SIZE, SHADOW_SIZE), DEPTH_FORMAT, 1, true);
        let shadow_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("shadow"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            compare: Some(wgpu::CompareFunction::LessEqual),
            ..Default::default()
        });
        Renderer {
            format: gpu.format,
            globals,
            look,
            layout_main,
            bind_pre,
            bind_main: None,
            pipe_background,
            pipe_mesh,
            pipe_prepass,
            pipe_shadow,
            pipe_back,
            matcap: matcap_texture(device, &gpu.queue, matcap),
            sampler,
            shadow,
            shadow_sampler,
            env: env::Env::new(gpu),
            env_seen: 0,
            post: post::Post::new(gpu),
            targets: None,
            meshes: HashMap::new(),
            objects: None,
            last_upload_bytes: 0,
            want_curvature: false,
            spheres: spheres::Layer::new(gpu),
            previews: preview::Layer::new(gpu),
            flash: flash::Layer::new(gpu),
            wire: wire::Layer::new(gpu),
            overlays: true,
        }
    }

    /// Exchanges the working textures in use with those in `side`. Call it
    /// before and after drawing at `side`'s size. The two sizes share the
    /// uniforms, so their draws must reach the GPU as separate submissions.
    pub fn swap_side(&mut self, side: &mut Side) {
        std::mem::swap(&mut self.targets, &mut side.targets);
        std::mem::swap(&mut self.bind_main, &mut side.bind_main);
        std::mem::swap(&mut self.env_seen, &mut side.env_seen);
        std::mem::swap(&mut self.post, &mut side.post);
    }

    pub fn set_matcap(&mut self, gpu: &Gpu, image: &image::RgbaImage) {
        self.matcap = matcap_texture(&gpu.device, &gpu.queue, image);
        self.bind_main = None;
    }

    /// Takes a new HDRI picture for the environment, or drops the current one.
    pub fn set_hdri(&mut self, gpu: &Gpu, image: Option<crate::look::HdriImage>) {
        self.env.set_hdri(gpu, image);
    }

    fn ensure_targets(&mut self, gpu: &Gpu, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let device = &gpu.device;
        let targets = Targets {
            size,
            msaa: target_texture(device, "scene", size, HDR_FORMAT, MSAA_SAMPLES, false),
            scene: target_texture(device, "scene resolved", size, HDR_FORMAT, 1, true),
            // Readable afterwards: depth of field works from it.
            depth: target_texture(device, "depth", size, DEPTH_FORMAT, MSAA_SAMPLES, true),
            ldr: target_texture(device, "picture", size, self.format, MSAA_SAMPLES, false),
            gbuf: target_texture(device, "normals", size, GBUF_FORMAT, 1, true),
            gbuf_depth: target_texture(device, "normals depth", size, DEPTH_FORMAT, 1, false),
            back: target_texture(device, "back depth", size, BACK_FORMAT, 1, true),
        };
        self.post.resize(gpu, size, &targets.gbuf, &targets.scene, &targets.depth);
        self.targets = Some(targets);
        self.bind_main = None;
    }

    /// Brings the GPU copy of mesh `id` up to date: fresh buffers if it is
    /// new, was swapped for another mesh or `rebuild` is set, otherwise only
    /// what changed.
    pub fn sync_mesh(&mut self, gpu: &Gpu, id: u64, mesh: &mut Mesh, rebuild: bool) {
        mesh.track_curvature(self.want_curvature);
        // Faces were hidden or shown since the last frame: the wireframe's
        // lines are of the old set, so they go, and are built again below.
        if self.meshes.get(&id).is_some_and(|b| b.visibility != mesh.visibility_version()) {
            self.wire.retain(|other| other != id);
        }
        if rebuild || self.meshes.get(&id).is_none_or(|b| b.uid != mesh.uid) {
            self.upload_mesh(gpu, id, mesh);
        } else {
            self.update_mesh(gpu, id, mesh);
        }
        self.wire.sync(gpu, id, mesh);
    }

    /// Frees the buffers of meshes the scene no longer has.
    pub fn retain_meshes(&mut self, keep: impl Fn(u64) -> bool) {
        self.meshes.retain(|&id, _| keep(id));
        let meshes = &self.meshes;
        self.wire.retain(|id| meshes.contains_key(&id));
    }

    /// Creates fresh buffers for a mesh whose topology changed.
    pub fn upload_mesh(&mut self, gpu: &Gpu, id: u64, mesh: &mut Mesh) {
        let _ = mesh.take_dirty();
        if mesh.vertex_count() == 0 {
            self.meshes.remove(&id);
            return;
        }
        let make = |label, bytes: &[u8], usage| {
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len() as u64,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue.write_buffer(&buffer, 0, bytes);
            buffer
        };
        let vertex = wgpu::BufferUsages::VERTEX;
        // Neutral (neither recess nor ridge) until a finish asks for the real thing.
        let neutral;
        let curv: &[f32] = if mesh.curv.len() == mesh.vertex_count() {
            &mesh.curv
        } else {
            neutral = vec![0.5f32; mesh.vertex_count()];
            &neutral
        };
        // Every vertex goes up, hidden or not; only the faces drawn shrink.
        let shown = mesh.visible_indices();
        let buffers = MeshBuffers {
            uid: mesh.uid,
            positions: make("positions", bytemuck::cast_slice(&mesh.positions), vertex),
            normals: make("normals", bytemuck::cast_slice(&mesh.normals), vertex),
            mask: make("mask", bytemuck::cast_slice(&mesh.mask), vertex),
            curv: make("curvature", bytemuck::cast_slice(curv), vertex),
            indices: index_buffer(gpu, &shown),
            index_count: shown.len() as u32,
            visibility: mesh.visibility_version(),
        };
        debug_assert_eq!(buffers.index_count as usize, mesh.visible_triangle_count() * 3);
        self.meshes.insert(id, buffers);
        self.last_upload_bytes = mesh.vertex_count() * 32 + shown.len() * 4;
    }

    /// Sends only what changed since the last call.
    pub fn update_mesh(&mut self, gpu: &Gpu, id: u64, mesh: &mut Mesh) {
        let dirty = mesh.take_dirty();
        let Some(buffers) = self.meshes.get_mut(&id) else { return };
        let mut bytes = 0;
        for r in &dirty.positions {
            let data: &[u8] = bytemuck::cast_slice(&mesh.positions[r.start as usize..r.end as usize]);
            gpu.queue.write_buffer(&buffers.positions, r.start as u64 * 12, data);
            bytes += data.len();
        }
        for r in &dirty.normals {
            let data: &[u8] = bytemuck::cast_slice(&mesh.normals[r.start as usize..r.end as usize]);
            gpu.queue.write_buffer(&buffers.normals, r.start as u64 * 12, data);
            bytes += data.len();
        }
        for r in &dirty.mask {
            let data: &[u8] = bytemuck::cast_slice(&mesh.mask[r.start as usize..r.end as usize]);
            gpu.queue.write_buffer(&buffers.mask, r.start as u64 * 4, data);
            bytes += data.len();
        }
        if mesh.curv.len() == mesh.vertex_count() {
            for r in &dirty.curv {
                let data: &[u8] = bytemuck::cast_slice(&mesh.curv[r.start as usize..r.end as usize]);
                gpu.queue.write_buffer(&buffers.curv, r.start as u64 * 4, data);
                bytes += data.len();
            }
        }
        // Faces were hidden or shown: the vertices stay where they are, and
        // only the list of faces to draw is sent again.
        if buffers.visibility != mesh.visibility_version() {
            let shown = mesh.visible_indices();
            buffers.indices = index_buffer(gpu, &shown);
            buffers.index_count = shown.len() as u32;
            buffers.visibility = mesh.visibility_version();
            debug_assert_eq!(buffers.index_count as usize, mesh.visible_triangle_count() * 3);
            bytes += shown.len() * 4;
        }
        if bytes > 0 {
            self.last_upload_bytes = bytes;
        }
    }

    /// Draws the viewport into `target` (a single-sample view of `size`):
    /// the background, then each of `items`, then the post effects.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        size: (u32, u32),
        globals: &Globals,
        look: &LookUniform,
        opts: &Options,
        items: &[DrawItem],
    ) {
        self.ensure_targets(gpu, size);
        if !items.is_empty() {
            if self.objects.as_ref().is_none_or(|(_, room)| *room < items.len()) {
                let room = items.len().next_power_of_two().max(16);
                let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("objects"),
                    size: room as u64 * 64,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.objects = Some((buffer, room));
            }
            let data: Vec<[[f32; 4]; 4]> = items.iter().map(|i| i.model).collect();
            gpu.queue.write_buffer(&self.objects.as_ref().unwrap().0, 0, bytemuck::cast_slice(&data));
        }

        // What this frame needs. A matcap carries its own lighting, so none
        // of the lighting's helper passes run under one.
        let settings = &opts.look;
        let lit = globals.shading[0] < 0.5 && !items.is_empty();
        let cavity = (globals.shading[2] > 0.0 || globals.shading[3] > 0.0) && !items.is_empty();
        let hdri = settings.hdri.active();
        let occlusion = lit && settings.ambient_occlusion;
        let shadows = lit && look.light_params[1] > 0.5;
        let plastic = lit && settings.finish == Finish::LurePlastic;

        if lit {
            // What the model reflects: the HDRI, else the background
            // gradient, else the default sky with the key light as its sun.
            let mut source = env::EnvParams {
                params: [0.0; 4],
                bottom: [0.0; 4],
                top: [0.0; 4],
                sun_dir: [0.0; 4],
                sun_color: [0.0; 4],
            };
            if hdri {
                source.params = [2.0, look.bg_params[1], look.bg_params[2], 0.0];
            } else if opts.gradient_background {
                let rgb = |c: [f32; 4]| [c[0], c[1], c[2], 0.0];
                source.params = [1.0, 0.0, 0.0, look.bg_params[3]];
                source.bottom = rgb(globals.bg_bottom);
                source.top = rgb(globals.bg_top);
            } else {
                let (dir, color) = settings.sky_sun();
                source.sun_dir = [dir.x, dir.y, dir.z, 0.0];
                source.sun_color = [color[0], color[1], color[2], 0.0];
            }
            self.env.update(gpu, encoder, &source);
        }

        let mut look = *look;
        if hdri {
            let strength = look.bg_params[2] * settings.hdri.ambient.clamp(0.0, 3.0);
            look.sh = self.env.ambient(look.bg_params[1], strength);
        }
        look.light_params[1] = if shadows { 1.0 } else { 0.0 };
        look.env_params[2] = if occlusion { 1.0 } else { 0.0 };
        look.matcap_params[3] = if plastic { 1.0 } else { 0.0 };
        gpu.queue.write_buffer(&self.globals, 0, bytemuck::bytes_of(globals));
        gpu.queue.write_buffer(&self.look, 0, bytemuck::bytes_of(&look));
        let srgb_to_linear = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        self.post.set_params(
            gpu,
            &post::PostParams {
                viewport: globals.viewport,
                ao: [look.cam_params[0], AO_RADIUS, AO_INTENSITY, 0.0],
                bloom: [
                    if settings.bloom { settings.bloom_intensity.max(0.0) } else { 0.0 },
                    srgb_to_linear(settings.bloom_threshold.max(0.0)),
                    // The pipeline's scatter setting of 0.5, on its 0.05-0.95 scale.
                    0.5,
                    0.0,
                ],
                vignette: [
                    if settings.vignette { settings.vignette_intensity.clamp(0.0, 1.0) * 3.0 } else { 0.0 },
                    settings.vignette_smoothness.clamp(0.01, 1.0) * 5.0,
                    0.0,
                    0.0,
                ],
                grade: [
                    match settings.tonemap {
                        Tonemap::None => 0.0,
                        Tonemap::Neutral => 1.0,
                        Tonemap::Aces => 2.0,
                    },
                    if settings.grading { 1.0 } else { 0.0 },
                    settings.saturation.clamp(-100.0, 100.0) / 100.0 + 1.0,
                    settings.contrast.clamp(-100.0, 100.0) / 100.0 + 1.0,
                ],
            },
        );
        // Depth of field: sets its lens, or puts its textures away if off.
        self.post.focus(gpu, settings, globals, look.cam_params[1]);

        let targets = self.targets.as_ref().unwrap();
        if self.bind_main.is_none() || self.env_seen != self.env.generation {
            self.env_seen = self.env.generation;
            let view = wgpu::BindingResource::TextureView;
            self.bind_main = Some(gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("main"),
                layout: &self.layout_main,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.globals.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: view(&self.matcap) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: view(&targets.gbuf) },
                    wgpu::BindGroupEntry { binding: 4, resource: self.look.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 5, resource: view(&self.env.view) },
                    wgpu::BindGroupEntry { binding: 6, resource: wgpu::BindingResource::Sampler(&self.env.sampler) },
                    wgpu::BindGroupEntry { binding: 7, resource: view(&self.shadow) },
                    wgpu::BindGroupEntry { binding: 8, resource: wgpu::BindingResource::Sampler(&self.shadow_sampler) },
                    wgpu::BindGroupEntry { binding: 9, resource: view(self.post.ao.as_ref().unwrap()) },
                    wgpu::BindGroupEntry { binding: 10, resource: view(&targets.back) },
                    wgpu::BindGroupEntry { binding: 11, resource: view(&self.env.hdri_view) },
                    wgpu::BindGroupEntry { binding: 12, resource: wgpu::BindingResource::Sampler(&self.env.hdri_sampler) },
                ],
            }));
        }
        let bind_main = self.bind_main.as_ref().unwrap();

        let draw_items = |pass: &mut wgpu::RenderPass| {
            let Some((objects, _)) = &self.objects else { return };
            for (i, item) in items.iter().enumerate() {
                let Some(mesh) = self.meshes.get(&item.mesh) else { continue };
                pass.set_vertex_buffer(0, mesh.positions.slice(..));
                pass.set_vertex_buffer(1, mesh.normals.slice(..));
                pass.set_vertex_buffer(2, mesh.mask.slice(..));
                pass.set_vertex_buffer(3, objects.slice(..));
                pass.set_vertex_buffer(4, mesh.curv.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..mesh.index_count, 0, i as u32..i as u32 + 1);
            }
        };
        // A colour buffer cleared to "nothing here", with its own depth.
        let helper_pass = |encoder: &mut wgpu::CommandEncoder, label: &str, view: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &targets.gbuf_depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(0.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                ..Default::default()
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &self.bind_pre, &[]);
            draw_items(&mut pass);
        };

        if shadows {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: depth_attachment(&self.shadow, 1.0),
                ..Default::default()
            });
            pass.set_pipeline(&self.pipe_shadow);
            pass.set_bind_group(0, &self.bind_pre, &[]);
            draw_items(&mut pass);
        }
        if cavity || occlusion {
            helper_pass(encoder, "normals prepass", &targets.gbuf, &self.pipe_prepass);
        }
        if plastic {
            helper_pass(encoder, "back depth", &targets.back, &self.pipe_back);
        }
        if occlusion {
            self.post.occlusion(encoder);
        }

        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("viewport"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &targets.msaa,
                    depth_slice: None,
                    resolve_target: Some(&targets.scene),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: depth_attachment(&targets.depth, 0.0),
                ..Default::default()
            });
            pass.set_bind_group(0, bind_main, &[]);
            pass.set_pipeline(&self.pipe_background);
            pass.draw(0..3, 0..1);
            if !items.is_empty() {
                pass.set_pipeline(&self.pipe_mesh);
                draw_items(&mut pass);
            }
        }

        // Bloom, vignette and the tone curve, into the finished picture.
        self.post.finish(encoder, &targets.ldr, settings.bloom && settings.bloom_intensity > 0.0);

        // Overlays go on the finished picture, hidden by whatever of the
        // scene is in front of them.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlays"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.ldr,
                depth_slice: None,
                resolve_target: Some(target),
                ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &targets.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        });
        // The pulse tints the surface, so the wireframe and the SSpheres go over it.
        if self.overlays {
            self.flash.draw(&mut pass, &self.meshes);
        }
        self.wire.draw(gpu, &mut pass, globals, &self.meshes, self.objects.as_ref().map(|o| &o.0), items);
        self.spheres.draw(&mut pass);
        if self.overlays {
            self.previews.draw(&mut pass);
        }
    }
}

/// A depth buffer cleared to `clear` and kept after the pass.
fn depth_attachment(view: &wgpu::TextureView, clear: f32) -> Option<wgpu::RenderPassDepthStencilAttachment<'_>> {
    Some(wgpu::RenderPassDepthStencilAttachment {
        view,
        depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(clear), store: wgpu::StoreOp::Store }),
        stencil_ops: None,
    })
}

/// Reads a rendered texture back as an image (used for screenshots).
pub fn read_texture(gpu: &Gpu, texture: &wgpu::Texture) -> image::RgbaImage {
    let (width, height) = (texture.width(), texture.height());
    let row = (width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("readback"),
        size: (row * height) as u64,
        usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(row),
                rows_per_image: Some(height),
            },
        },
        wgpu::Extent3d { width, height, depth_or_array_layers: 1 },
    );
    gpu.queue.submit([encoder.finish()]);
    buffer.map_async(wgpu::MapMode::Read, .., |_| {});
    let _ = gpu.device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None });
    let data = buffer.get_mapped_range(..).expect("readback buffer is mapped");
    let bgra = matches!(texture.format(), wgpu::TextureFormat::Bgra8Unorm);
    let mut out = image::RgbaImage::new(width, height);
    for y in 0..height {
        let src = &data[(y * row) as usize..(y * row + width * 4) as usize];
        for (x, px) in src.chunks_exact(4).enumerate() {
            let rgba = if bgra { [px[2], px[1], px[0], 255] } else { [px[0], px[1], px[2], 255] };
            out.put_pixel(x as u32, y, image::Rgba(rgba));
        }
    }
    out
}
