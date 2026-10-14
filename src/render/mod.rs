//! GPU side: device/surface setup, the viewport pipelines, and the mesh
//! buffers. After a brush dab only the vertex ranges that changed are
//! re-uploaded; the mesh keeps its vertices in spatial order so those ranges
//! are few and contiguous.

use std::collections::HashMap;
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use winit::window::Window;

use crate::doc::Options;
use crate::look::{LookUniform, Tonemap, SHADOW_SIZE};
use pipelines::{Cull, MeshKey};
use crate::mesh::Mesh;

mod cache;
mod chunks;
mod dof;
pub mod env;
mod mips;
mod pipelines;
pub mod flash;
pub mod groups;
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
                    // The face group colours read a dense mesh's own buffers (render/groups.rs).
                    max_storage_buffer_binding_size: adapter.limits().max_storage_buffer_binding_size,
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

    /// Has the device leave word with `faults` of an error nothing caught
    /// and of the device being lost. Left to itself wgpu answers the first
    /// with a panic wherever it happens to be standing, and says nothing of
    /// the second until something else fails. These callbacks may run on
    /// any thread and cannot touch the scene, so they only note what
    /// happened; the app finds the note between events, when the scene is
    /// whole, keeps a copy of it and closes (see app/recovery.rs).
    pub fn report_faults_to(&self, faults: crate::recovery::Faults) {
        let errors = faults.clone();
        self.device.on_uncaptured_error(Arc::new(move |error: wgpu::Error| {
            errors.report(fault_words(&error), &error.to_string());
        }));
        self.device.set_device_lost_callback(move |reason, message| {
            faults.report("The graphics card stopped answering", &format!("device lost ({reason:?}) {message}"));
        });
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

    /// The size a picture is given as a texture on this device: its own, or
    /// halved until the device takes it (see `mips::fitted`).
    pub fn fitted_size(&self, width: u32, height: u32) -> (u32, u32) {
        mips::fitted_size(width, height, self.device.limits().max_texture_dimension_2d)
    }
}

/// A graphics error in words for the user: which of the three kinds it is.
/// (What wgpu itself said goes to the log.)
fn fault_words(error: &wgpu::Error) -> &'static str {
    match error {
        wgpu::Error::OutOfMemory { .. } => "The graphics card ran out of memory",
        wgpu::Error::Validation { .. } => "The graphics card refused a drawing command",
        wgpu::Error::Internal { .. } => "The graphics driver reported a fault",
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
    /// The radial symmetry guide (see `sculpt::Radial::guide`): xyz the axis
    /// in the space the guides are in, w how many spokes, 0 for no guide.
    pub radial_axis: [f32; 4],
    /// xyz the way the guide's first spoke points, w its colour.
    pub radial_spoke: [f32; 4],
    /// The active object's own space into the space its symmetry guides are
    /// in (`scene::Guides::space`): nothing to do for a symmetry of its own,
    /// its placement for one fixed in the world.
    pub guide_space: [[f32; 4]; 4],
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
    /// The finished picture with its overlays, kept for frames in which
    /// nothing of it changes (see cache.rs), and what it was drawn from.
    finished: wgpu::TextureView,
    blit_finished: wgpu::BindGroup,
    drawn: Option<cache::SceneKey>,
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
    /// `Mesh::topology_version` of the faces in `indices`.
    topology: u64,
    /// How many vertices the vertex buffers have room for, and how many
    /// corners `indices` has. Exactly what the mesh has, until dynamic
    /// topology outgrows them: then they are made half as big again, so a
    /// stroke that adds a few hundred vertices a frame is not a new set of
    /// buffers, and the whole mesh sent again, every frame.
    vertex_room: u32,
    index_room: u32,
    /// The mesh cut into pieces, so only those in view are drawn (chunks.rs).
    chunks: chunks::Chunks,
}

impl MeshBuffers {
    /// The runs of corners to draw of this mesh placed by `model` (an
    /// object's matrix by columns, as in `DrawItem`) and seen through
    /// `view_proj`.
    fn visible(&self, view_proj: &glam::Mat4, model: &[[f32; 4]; 4], out: &mut Vec<std::ops::Range<u32>>) {
        out.clear();
        if self.chunks.all().end != self.index_count {
            // Pieces of another set of faces than the buffer's: all of it.
            out.push(0..self.index_count);
            return;
        }
        let mut model = glam::Mat4::from_cols_array_2d(model);
        // The last element carries the active flag, not the matrix's own 1.
        model.w_axis.w = 1.0;
        self.chunks.visible(&(*view_proj * model), out);
    }
}

/// A buffer with room for `room` bytes, holding `bytes` at its start.
fn roomy_buffer(gpu: &Gpu, label: &str, bytes: &[u8], room: u64, usage: wgpu::BufferUsages) -> wgpu::Buffer {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: room.max(bytes.len() as u64).max(4),
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    gpu.queue.write_buffer(&buffer, 0, bytes);
    buffer
}

/// An index buffer holding `corners`. It is never made empty: a mesh with
/// every face hidden still has to bind one, and a buffer of no size cannot
/// be bound.
fn index_buffer(gpu: &Gpu, corners: &[u32]) -> wgpu::Buffer {
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("indices"),
        size: corners.len().max(3) as u64 * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST | groups::READ,
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
    /// The main pass's, one per finish and culling (pipelines.rs).
    mesh_pipes: pipelines::MeshPipelines,
    /// The helper passes', one per way of culling (`Cull::index`).
    pipe_prepass: [wgpu::RenderPipeline; 3],
    pipe_shadow: [wgpu::RenderPipeline; 3],
    pipe_back: [wgpu::RenderPipeline; 3],
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
    /// The face group colours.
    pub groups: groups::Layer,
    /// Off, the pulses and the tools' previews are left out: the clean view,
    /// and pictures drawn for a recording.
    pub overlays: bool,
    /// Set by anything that changes the picture in a way the scene key
    /// cannot see (a new matcap or HDRI picture): the next frame is drawn
    /// afresh rather than copied (see cache.rs).
    pub scene_dirty: bool,
    /// How many times `scene_dirty` has been set, for the scene key: a
    /// picture kept on the side (a recording's) sees it too.
    forced: u64,
    /// Moves whenever a mesh's buffers are sent or let go.
    uploads: cache::Stamp,
    /// What the shadow map holds: drawn from the key light over the objects
    /// as this key has them. It is drawn again only when that changes (see
    /// `shadow_key`), not every frame.
    shadow_drawn: Option<cache::SceneKey>,
    blit: cache::Blit,
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
    // No larger than this device takes: a texture over its limit is refused
    // with an error that stops the app, and a picture halved to fit shades
    // the same. The limit is the device's own, as an HDRI is fitted to.
    let image = &*mips::fitted(image, device.limits().max_texture_dimension_2d);
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
        let vertex_buffers = pipelines::mesh_vertex_buffers();
        // The helper passes read only positions, normals and the objects'
        // matrices: the mask and curvature slots are left out of their layout.
        let slim_buffers = [vertex_buffers[0].clone(), vertex_buffers[1].clone(), None, vertex_buffers[3].clone(), None];
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
        let primitive = wgpu::PrimitiveState::default();

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
        let mesh_pipes =
            pipelines::MeshPipelines::new(device, &shader, &layout, HDR_FORMAT, DEPTH_FORMAT, MSAA_SAMPLES);
        let layout = pipeline_layout(&layout_pre);
        let helper = |label, fragment: &str, format: wgpu::TextureFormat, cull: Cull| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_prepass"),
                    compilation_options: Default::default(),
                    buffers: &slim_buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: cull.face(), ..Default::default() },
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
        // Culled as the main pass is, so they see the faces it shows.
        let pipe_prepass = Cull::ALL.map(|cull| helper("prepass", "fs_prepass", GBUF_FORMAT, cull));
        // Back faces only: the far side of the model under each pixel. Indexed
        // by the object's own culling, and culling the other face; with Show
        // Backfaces on both come through and the shader drops the front ones.
        let pipe_back = Cull::ALL.map(|cull| helper("back depth", "fs_back_depth", BACK_FORMAT, cull.flipped()));
        // Depth only, from the key light; ordinary (not reverse) depth. Only
        // faces toward the light: a closed model's others are behind them.
        let pipe_shadow = Cull::ALL.map(|cull| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("shadow"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_shadow"),
                    compilation_options: Default::default(),
                    buffers: &slim_buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: cull.face(), ..Default::default() },
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
            })
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
            mesh_pipes,
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
            groups: groups::Layer::new(gpu),
            overlays: true,
            scene_dirty: false,
            forced: 0,
            uploads: cache::Stamp::default(),
            shadow_drawn: None,
            blit: cache::Blit::new(gpu),
        }
    }

    /// Starts building every way of shading the main pass can need on a
    /// thread of its own, so the first time a finish is picked is not a
    /// pause (see pipelines.rs).
    pub fn prewarm(&mut self) {
        self.mesh_pipes.prewarm();
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
        self.scene_dirty = true;
    }

    /// Takes a new HDRI picture for the environment, or drops the current one.
    pub fn set_hdri(&mut self, gpu: &Gpu, image: Option<crate::look::HdriImage>) {
        self.env.set_hdri(gpu, image);
        self.scene_dirty = true;
    }

    fn ensure_targets(&mut self, gpu: &Gpu, size: (u32, u32)) {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        let device = &gpu.device;
        let finished = target_texture(device, "finished picture", size, self.format, 1, true);
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
            blit_finished: self.blit.bind(gpu, &finished),
            finished,
            drawn: None,
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
        // Faces were hidden or shown since the last frame, or dynamic
        // topology split some: the wireframe's lines are of the old set, so
        // they go, and are built again below.
        let stale = |b: &MeshBuffers| {
            b.visibility != mesh.visibility_version() || b.topology != mesh.topology_version()
        };
        if self.meshes.get(&id).is_some_and(stale) {
            self.wire.retain(|other| other != id);
        }
        if rebuild || self.meshes.get(&id).is_none_or(|b| b.uid != mesh.uid) {
            self.upload_mesh(gpu, id, mesh);
        } else {
            self.update_mesh(gpu, id, mesh);
        }
        self.wire.sync(gpu, id, mesh);
        self.groups.sync(gpu, id, mesh, self.meshes.get(&id));
    }

    /// Moves whenever any mesh's buffers are sent again or let go: the
    /// meshes the picture was drawn from have changed.
    pub fn uploads(&self) -> u64 {
        self.uploads.count()
    }

    /// Frees the buffers of meshes the scene no longer has.
    pub fn retain_meshes(&mut self, keep: impl Fn(u64) -> bool) {
        let before = self.meshes.len();
        self.meshes.retain(|&id, _| keep(id));
        if self.meshes.len() != before {
            self.uploads.bump();
        }
        let meshes = &self.meshes;
        self.wire.retain(|id| meshes.contains_key(&id));
        self.groups.retain(|id| meshes.contains_key(&id));
    }

    /// Creates fresh buffers for a mesh whose topology changed.
    pub fn upload_mesh(&mut self, gpu: &Gpu, id: u64, mesh: &mut Mesh) {
        let _ = mesh.take_dirty();
        self.uploads.bump();
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
            positions: make("positions", bytemuck::cast_slice(&mesh.positions), vertex | groups::READ),
            normals: make("normals", bytemuck::cast_slice(&mesh.normals), vertex),
            mask: make("mask", bytemuck::cast_slice(&mesh.mask), vertex),
            curv: make("curvature", bytemuck::cast_slice(curv), vertex),
            indices: index_buffer(gpu, &shown),
            index_count: shown.len() as u32,
            visibility: mesh.visibility_version(),
            topology: mesh.topology_version(),
            vertex_room: mesh.vertex_count() as u32,
            index_room: shown.len().max(3) as u32,
            chunks: chunks::Chunks::build(mesh),
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
        // Dynamic topology added more vertices than the buffers hold: new
        // ones with room to spare, and every vertex sent again.
        let count = mesh.vertex_count() as u32;
        let outgrown = count > buffers.vertex_room;
        if outgrown {
            let room = count + count / 2;
            let vertex = wgpu::BufferUsages::VERTEX;
            let make = |label, data: &[u8], stride: u64| roomy_buffer(gpu, label, data, room as u64 * stride, vertex);
            let positions: &[u8] = bytemuck::cast_slice(&mesh.positions);
            buffers.positions = roomy_buffer(gpu, "positions", positions, room as u64 * 12, vertex | groups::READ);
            buffers.normals = make("normals", bytemuck::cast_slice(&mesh.normals), 12);
            buffers.mask = make("mask", bytemuck::cast_slice(&mesh.mask), 4);
            // Neutral all the way to the end while no finish reads it, so the
            // vertices still to come are neutral too.
            let neutral;
            let curv: &[f32] = if mesh.curv.len() == mesh.vertex_count() {
                &mesh.curv
            } else {
                neutral = vec![0.5f32; room as usize];
                &neutral
            };
            buffers.curv = make("curvature", bytemuck::cast_slice(curv), 4);
            buffers.vertex_room = room;
            bytes += count as usize * 32;
        }
        let moved = outgrown || !dirty.positions.is_empty();
        let dirty = if outgrown { crate::mesh::DirtyRanges { triangles: dirty.triangles, ..Default::default() } } else { dirty };
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
        // only the list of faces to draw is sent again. The same when
        // triangles changed while some are hidden, since the list holds only
        // the ones that show and so has no fixed place for any of them.
        let split = buffers.topology != mesh.topology_version();
        let shown_again = buffers.visibility != mesh.visibility_version();
        if shown_again || (split && mesh.any_hidden()) {
            let shown = mesh.visible_indices();
            buffers.indices = index_buffer(gpu, &shown);
            buffers.index_count = shown.len() as u32;
            buffers.index_room = shown.len().max(3) as u32;
            buffers.visibility = mesh.visibility_version();
            debug_assert_eq!(buffers.index_count as usize, mesh.visible_triangle_count() * 3);
            bytes += shown.len() * 4;
        } else if split {
            // Dynamic topology rewrote some triangles and added others. With
            // nothing hidden the list is the mesh's own, triangle for
            // triangle, so only those are sent, unless they no longer fit.
            let corners = mesh.indices.len() as u32;
            if corners > buffers.index_room {
                let room = corners + corners / 2;
                let data: &[u8] = bytemuck::cast_slice(&mesh.indices);
                let usage = wgpu::BufferUsages::INDEX | groups::READ;
                buffers.indices = roomy_buffer(gpu, "indices", data, room as u64 * 4, usage);
                buffers.index_room = room;
                bytes += data.len();
            } else {
                for r in &dirty.triangles {
                    let data: &[u8] = bytemuck::cast_slice(&mesh.indices[r.start as usize * 3..r.end as usize * 3]);
                    gpu.queue.write_buffer(&buffers.indices, r.start as u64 * 12, data);
                    bytes += data.len();
                }
            }
            buffers.index_count = corners;
        }
        buffers.topology = mesh.topology_version();
        if split || shown_again {
            buffers.chunks = chunks::Chunks::build(mesh);
        } else if moved {
            buffers.chunks.refresh(mesh);
        }
        if bytes > 0 || split || shown_again {
            self.uploads.bump();
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

        // What this frame needs. A matcap carries its own lighting, so none
        // of the lighting's helper passes run under one.
        let settings = &opts.look;
        let lit = globals.shading[0] < 0.5 && !items.is_empty();
        let cavity = (globals.shading[2] > 0.0 || globals.shading[3] > 0.0) && !items.is_empty();
        let hdri = settings.hdri.active();
        let occlusion = lit && settings.ambient_occlusion;
        let shadows = lit && look.light_params[1] > 0.5;
        // The pipeline: the matcap, or the finish the look names, with the glow or not.
        let surface = if globals.shading[0] > 0.5 {
            pipelines::SURFACE_MATCAP
        } else {
            ((look.matcap_params[2] + 0.5) as u32).clamp(0, 4)
        };
        let glow = look.glow.v[2][0] > 0.5;
        let plastic = lit && surface == 1;
        let source = lit.then(|| env_source(globals, look, opts));
        let post_params = post_params(globals, look, settings);

        // Nothing it is drawn from has changed: the picture is the one kept.
        if std::mem::take(&mut self.scene_dirty) {
            self.forced += 1;
        }
        let key = self.scene_key(size, globals, look, opts, items, source.as_ref(), &post_params);
        let targets = self.targets.as_mut().unwrap();
        if targets.drawn.as_ref() == Some(&key) {
            self.blit.draw(encoder, &targets.blit_finished, target);
            return;
        }
        targets.drawn = None;

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
        if let Some(source) = &source {
            self.env.update(gpu, encoder, source);
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
        self.post.set_params(gpu, &post_params);
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

        // Which parts of each object are in view (chunks.rs).
        let view_proj = glam::Mat4::from_cols_array_2d(&globals.view_proj);
        let in_view = self.visible_runs(&view_proj, items);
        // How each object is culled, and the main pass's pipeline for each.
        let culls: Vec<Cull> =
            items.iter().map(|i| Cull::of(opts.show_backfaces, pipelines::mirrored(&i.model))).collect();
        let mesh_pipes: Vec<(Cull, wgpu::RenderPipeline)> = Cull::ALL
            .into_iter()
            .filter(|c| culls.contains(c))
            .map(|cull| (cull, self.mesh_pipes.get(MeshKey { surface, glow, cull }).clone()))
            .collect();
        // Draws the objects culled as `cull`, the parts of each in `runs`.
        let draw_items = |pass: &mut wgpu::RenderPass, runs: &[Vec<std::ops::Range<u32>>], cull: Cull| {
            let Some((objects, _)) = &self.objects else { return };
            for (i, item) in items.iter().enumerate() {
                let Some(mesh) = self.meshes.get(&item.mesh) else { continue };
                if runs[i].is_empty() || culls[i] != cull {
                    continue;
                }
                pass.set_vertex_buffer(0, mesh.positions.slice(..));
                pass.set_vertex_buffer(1, mesh.normals.slice(..));
                pass.set_vertex_buffer(2, mesh.mask.slice(..));
                pass.set_vertex_buffer(3, objects.slice(..));
                pass.set_vertex_buffer(4, mesh.curv.slice(..));
                pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
                for run in &runs[i] {
                    pass.draw_indexed(run.clone(), 0, i as u32..i as u32 + 1);
                }
            }
        };
        // A colour buffer cleared to "nothing here", with its own depth.
        let helper_pass = |encoder: &mut wgpu::CommandEncoder, label: &str, view: &wgpu::TextureView, pipelines: &[wgpu::RenderPipeline; 3]| {
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
            pass.set_bind_group(0, &self.bind_pre, &[]);
            for cull in Cull::ALL.into_iter().filter(|c| culls.contains(c)) {
                pass.set_pipeline(&pipelines[cull.index()]);
                draw_items(&mut pass, &in_view, cull);
            }
        };

        let shadow_key = shadows.then(|| self.shadow_key(&look, items, opts.show_backfaces));
        if shadows && shadow_key != self.shadow_drawn {
            self.shadow_drawn = shadow_key;
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("shadow"),
                color_attachments: &[],
                depth_stencil_attachment: depth_attachment(&self.shadow, 1.0),
                ..Default::default()
            });
            pass.set_bind_group(0, &self.bind_pre, &[]);
            let light = glam::Mat4::from_cols_array_2d(&look.shadow_view_proj);
            let runs = self.visible_runs(&light, items);
            for cull in Cull::ALL.into_iter().filter(|c| culls.contains(c)) {
                pass.set_pipeline(&self.pipe_shadow[cull.index()]);
                draw_items(&mut pass, &runs, cull);
            }
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
            for (cull, pipeline) in &mesh_pipes {
                pass.set_pipeline(pipeline);
                draw_items(&mut pass, &in_view, *cull);
            }
        }

        // Bloom, vignette and the tone curve, into the finished picture:
        // straight into the one kept when nothing goes over it, otherwise
        // into a multisampled copy the overlays are drawn on.
        let bloom = settings.bloom && settings.bloom_intensity > 0.0;
        let overlaid = (self.overlays && (self.flash.active() || self.previews.active()))
            || self.groups.active()
            || self.wire.active()
            || self.spheres.active();
        if !overlaid {
            self.post.finish(encoder, &targets.finished, bloom, true);
            self.blit.draw(encoder, &targets.blit_finished, target);
            self.targets.as_mut().unwrap().drawn = Some(key);
            return;
        }
        self.post.finish(encoder, &targets.ldr, bloom, false);

        // Overlays go on the finished picture, hidden by whatever of the
        // scene is in front of them.
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("overlays"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.ldr,
                depth_slice: None,
                resolve_target: Some(&targets.finished),
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
            self.flash.draw(&mut pass, &self.meshes, opts.show_backfaces);
        }
        let objects = self.objects.as_ref().map(|o| &o.0);
        self.groups.draw(gpu, &mut pass, &globals.view_proj, &self.meshes, objects, items, &in_view, opts.show_backfaces);
        self.wire.draw(gpu, &mut pass, globals, &self.meshes, objects, items);
        self.spheres.draw(&mut pass);
        if self.overlays {
            self.previews.draw(&mut pass);
        }
        drop(pass);
        self.blit.draw(encoder, &targets.blit_finished, target);
        self.targets.as_mut().unwrap().drawn = Some(key);
    }

    /// For each of `items`, the runs of its mesh's corners in the view
    /// `view_proj` (none for a mesh with no buffers).
    fn visible_runs(&self, view_proj: &glam::Mat4, items: &[DrawItem]) -> Vec<Vec<std::ops::Range<u32>>> {
        items
            .iter()
            .map(|item| {
                let mut runs = Vec::new();
                if let Some(mesh) = self.meshes.get(&item.mesh) {
                    mesh.visible(view_proj, &item.model, &mut runs);
                }
                runs
            })
            .collect()
    }

    /// What the shadow map is drawn from: the key light's direction and
    /// view, the biases, the objects and their meshes. With the lights
    /// following the camera the key turns with every orbit, and the map is
    /// drawn every frame of one; with the lights left in the world it is
    /// drawn once and kept until the model or the scene changes.
    fn shadow_key(&self, look: &LookUniform, items: &[DrawItem], show_backfaces: bool) -> cache::SceneKey {
        let mut key = cache::SceneKey::default();
        key.push(&look.light_dir[0]).push(&look.shadow_view_proj).push(&look.shadow_params);
        key.push(&(show_backfaces as u32));
        key.push(&(items.len() as u64));
        for item in items {
            key.push(&item.mesh).push(&item.model);
        }
        key.push(&self.uploads.count());
        key
    }

    /// The bytes of everything this frame's picture is drawn from (see
    /// cache.rs). `source` and `post` are worked out from the others but
    /// are cheap, and taking them in keeps any setting they read in the key.
    #[allow(clippy::too_many_arguments)]
    fn scene_key(
        &self,
        size: (u32, u32),
        globals: &Globals,
        look: &LookUniform,
        opts: &Options,
        items: &[DrawItem],
        source: Option<&env::EnvParams>,
        post: &post::PostParams,
    ) -> cache::SceneKey {
        let settings = &opts.look;
        let mut key = cache::SceneKey::default();
        key.push(&[size.0, size.1]).push(globals).push(&look_without_clock(look)).push(post);
        key.push(&(items.len() as u64));
        for item in items {
            key.push(&item.mesh).push(&item.model);
        }
        if let Some(source) = source {
            key.push(source);
        }
        let flag = |on: bool| if on { 1.0f32 } else { 0.0 };
        key.push(&[
            flag(settings.hdri.active()),
            settings.hdri.ambient,
            flag(settings.depth_of_field),
            settings.focus_distance,
            settings.aperture,
            flag(settings.bloom),
            flag(self.overlays),
            flag(self.wire.enabled),
            flag(self.groups.enabled),
            flag(opts.show_backfaces),
        ]);
        key.push(&[
            self.forced,
            self.uploads.count(),
            self.env.generation as u64,
            self.flash.stamp.count(),
            self.spheres.stamp.count(),
            self.previews.stamp.count(),
            self.wire.stamp.count(),
            self.groups.stamp.count(),
        ]);
        key
    }
}

/// `look` with the clock zeroed unless a glow is moving: nothing else reads
/// it, and a clock that changes every frame would keep any picture from
/// being reused.
fn look_without_clock(look: &LookUniform) -> LookUniform {
    let mut out = *look;
    let glow_on = look.glow.v[2][0] > 0.5;
    let moving = look.glow.v[3][2] != 0.0;
    if !(glow_on && moving) {
        out.light_params[3] = 0.0;
    }
    out
}

/// What the model reflects: the HDRI, else the background gradient, else
/// the default sky with the key light as its sun.
fn env_source(globals: &Globals, look: &LookUniform, opts: &Options) -> env::EnvParams {
    let mut source = env::EnvParams {
        params: [0.0; 4],
        bottom: [0.0; 4],
        top: [0.0; 4],
        sun_dir: [0.0; 4],
        sun_color: [0.0; 4],
    };
    if opts.look.hdri.active() {
        source.params = [2.0, look.bg_params[1], look.bg_params[2], 0.0];
    } else if opts.gradient_background {
        let rgb = |c: [f32; 4]| [c[0], c[1], c[2], 0.0];
        source.params = [1.0, 0.0, 0.0, look.bg_params[3]];
        source.bottom = rgb(globals.bg_bottom);
        source.top = rgb(globals.bg_top);
    } else {
        let (dir, color) = opts.look.sky_sun();
        source.sun_dir = [dir.x, dir.y, dir.z, 0.0];
        source.sun_color = [color[0], color[1], color[2], 0.0];
    }
    source
}

/// The post effects' settings for this frame.
fn post_params(globals: &Globals, look: &LookUniform, settings: &crate::look::Look) -> post::PostParams {
    let srgb_to_linear = |c: f32| if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
    post::PostParams {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_clock_is_in_the_scene_key_only_while_a_glow_moves() {
        let mut look = LookUniform::zeroed();
        look.light_params[3] = 12.5;
        assert_eq!(look_without_clock(&look).light_params[3], 0.0, "no glow");
        look.glow.v[2][0] = 1.0;
        assert_eq!(look_without_clock(&look).light_params[3], 0.0, "a glow standing still");
        look.glow.v[3][2] = 0.8;
        assert_eq!(look_without_clock(&look).light_params[3], 12.5, "a moving glow");
        // Nothing else is touched.
        look.light_params[0] = 3.0;
        assert_eq!(look_without_clock(&look).light_params[0], 3.0);
    }
}
