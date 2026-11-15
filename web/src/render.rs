//! The viewport: the mesh drawn under a matcap into a texture of its own
//! (multisampled, reverse depth, as the desktop draws), then copied into the
//! egui page inside a paint callback.
//!
//! The mesh lives on the graphics card in four vertex buffers (position,
//! normal, mask, curvature) and an index buffer. A stroke sends only the
//! vertex ranges it changed (`Mesh::take_dirty`); a new mesh (a shape, a
//! file, Remesh, Subdivide, an undo of those) is sent whole.
//!
//! A big mesh is drawn only where it is in view: the desktop's `chunks.rs`
//! cuts it into pieces with a box round each, and [`Culling`] hands the
//! renderer the runs of triangles whose boxes the view touches. The
//! wireframe is a list of lines made from the triangles as they arrive, laid
//! out triangle by triangle so dynamic topology can send it in parts too.
//! Save Picture draws the same scene again into a picture of its own
//! (`still.rs`).

use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};

use eframe::egui_wgpu::{self, CallbackResources, CallbackTrait, ScreenDescriptor};
use eframe::wgpu;
use eframe::wgpu::util::DeviceExt;
use sculpt_toolkit::draw::PreviewDraw;

use crate::mesh::Mesh;

/// The desktop's own pieces-in-view code, compiled here as it is.
#[allow(dead_code)]
#[path = "../../src/render/chunks.rs"]
pub mod chunks;
#[path = "still.rs"]
pub mod still;
#[path = "finish.rs"]
pub mod finish;
#[path = "lighting.rs"]
pub mod lighting;

const SAMPLES: u32 = 4;
const COLOR: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
const DEPTH: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;
/// Each drawn thing's `Object` sits this far from the last in its buffer:
/// the alignment a dynamic uniform offset needs.
const OBJECT_STRIDE: u64 = 256;
/// The wireframe keeps two lines (four vertex numbers) for every triangle,
/// whether it needs both or not, so triangle `t`'s lines are always at the
/// same place in the line buffer and can be sent alone when it changes.
const LINE_CORNERS_PER_TRIANGLE: u64 = 4;
/// Meshes with fewer triangles than this are drawn whole: cutting them into
/// pieces would save the graphics card next to nothing.
const CULL_FROM_TRIANGLES: usize = 200_000;

/// One drawn thing's place and tint (the shader's `Object`).
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Object {
    model: [[f32; 4]; 4],
    normal: [[f32; 4]; 4],
    tint: [f32; 4],
}

impl Object {
    fn at(model: glam::Mat4, tint: [f32; 4]) -> Object {
        let normal = model.inverse().transpose();
        let normal = if normal.is_finite() { normal } else { model };
        Object { model: model.to_cols_array_2d(), normal: normal.to_cols_array_2d(), tint }
    }
}

/// The renderer's numbers for the other objects' meshes (`Other`) are the
/// object's own (`scene::Drawn::id`) with this bit set, kept apart from a
/// tool's previews in the same store.
const OTHER: u64 = 1 << 63;

/// An object that is not the active one, drawn where it stands.
pub struct Other {
    pub drawn: Arc<crate::scene::Drawn>,
    pub model: glam::Mat4,
}

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Globals {
    pub view_proj: [[f32; 4]; 4],
    pub view: [[f32; 4]; 4],
    pub mask_tint: [f32; 4],
    pub params: [f32; 4],
    pub bg_top: [f32; 4],
    pub bg_bottom: [f32; 4],
    /// The camera's place (w: 1 for an orthographic view), the way it
    /// looks, its right and up (w: how far the view reaches across and up
    /// at one unit in front of it). The wireframe and the gradient read them.
    pub eye: [f32; 4],
    pub forward: [f32; 4],
    pub right: [f32; 4],
    pub up: [f32; 4],
    /// x: the backdrop is a gradient, y: its bias, z: leave the backdrop
    /// out (set for a picture only), w: the wireframe's opacity.
    pub look: [f32; 4],
    /// xyz: 1 where that mirror plane's guide is drawn.
    pub guides: [f32; 4],
    /// The Effects card, applied as the picture goes into the page:
    /// saturation and contrast (1 unchanged), the vignette's strength (0
    /// none) and smoothness.
    pub grade: [f32; 4],
    /// x: film grain (0 none).
    pub extras: [f32; 4],
}

impl Globals {
    /// The camera's part of the globals (`eye`, `forward`, `right`, `up`),
    /// from the view being drawn.
    pub fn camera(view: &crate::camera::View) -> [[f32; 4]; 4] {
        let ortho = view.ortho_half_height.is_some();
        let aspect = view.size.x / view.size.y.max(1.0);
        let reach = if ortho { 0.0 } else { view.tan_half_fov };
        [
            [view.pos.x, view.pos.y, view.pos.z, ortho as u8 as f32],
            [view.forward.x, view.forward.y, view.forward.z, 0.0],
            [view.right.x, view.right.y, view.right.z, reach * aspect],
            [view.up.x, view.up.y, view.up.z, reach],
        ]
    }
}

/// What goes to the graphics card this frame.
pub enum Upload {
    /// A new mesh: every buffer made afresh. `generation` numbers it, and is
    /// written back to [`Viewport::has`] once the card holds it.
    /// The buffers are made with room for `vertex_room` vertices and
    /// `index_room` corners, so a mesh that dynamic topology grows can be
    /// sent in parts until it outgrows them.
    Whole {
        generation: u64,
        positions: Vec<u8>,
        normals: Vec<u8>,
        mask: Vec<u8>,
        /// The mesh's curvature for cavity; empty while cavity is off.
        curv: Vec<u8>,
        indices: Vec<u8>,
        index_count: u32,
        vertex_room: u64,
        index_room: u64,
    },
    /// Ranges of an existing mesh's vertex buffers: (byte offset, bytes).
    /// Triangles dynamic topology changed or added come as ranges of the
    /// index buffer, with the new count of corners when it changed.
    Parts {
        generation: u64,
        positions: Vec<(u64, Vec<u8>)>,
        normals: Vec<(u64, Vec<u8>)>,
        mask: Vec<(u64, Vec<u8>)>,
        curv: Vec<(u64, Vec<u8>)>,
        indices: Vec<(u64, Vec<u8>)>,
        index_count: Option<u32>,
    },
}

struct MeshBuffers {
    positions: wgpu::Buffer,
    normals: wgpu::Buffer,
    mask: wgpu::Buffer,
    curv: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// The wireframe's lines on the card: two per triangle of the mesh's index
/// buffer, at the same place (`LINE_CORNERS_PER_TRIANGLE`).
struct Lines {
    buffer: wgpu::Buffer,
}

/// A picture being read back from the card for Save Picture.
struct PendingStill {
    request: Arc<still::Request>,
    buffer: wgpu::Buffer,
    size: [u32; 2],
    /// Bytes per row in `buffer`: a row of pixels padded to 256 bytes.
    row: u32,
    /// 0 while the card works, 1 once the buffer can be read, 2 if it cannot.
    mapped: Arc<AtomicU8>,
}

/// A tool's preview mesh on the card, kept while the tool keeps drawing it.
struct PreviewBuffers {
    version: u64,
    mesh: MeshBuffers,
    used: bool,
}

struct Target {
    size: [u32; 2],
    msaa: wgpu::TextureView,
    resolved: wgpu::TextureView,
    depth: wgpu::TextureView,
    blit_group: wgpu::BindGroup,
}

/// Kept in egui's callback resources for the life of the page.
pub struct Gpu {
    mesh_pipeline: wgpu::RenderPipeline,
    wire_pipeline: wgpu::RenderPipeline,
    blit_pipeline: wgpu::RenderPipeline,
    /// The copy into a picture to save, whose colour format is the
    /// picture's rather than the page's.
    still_blit_pipeline: wgpu::RenderPipeline,
    blit_layout: wgpu::BindGroupLayout,
    globals: wgpu::Buffer,
    /// The material finish's settings and lights (`finish::Uniform`).
    finish: wgpu::Buffer,
    mesh_group: wgpu::BindGroup,
    mesh: Option<MeshBuffers>,
    target: Option<Target>,
    object_layout: wgpu::BindGroupLayout,
    objects: wgpu::Buffer,
    object_group: wgpu::BindGroup,
    previews: std::collections::HashMap<u64, PreviewBuffers>,
    /// The page's own colour format wants linear colour (an sRGB format).
    srgb_target: bool,
    /// Which whole mesh the buffers hold (`Upload::Whole::generation`).
    generation: u64,
    /// The matcap drawn with (the app's number for it), what its bind group
    /// is laid out by, and the sampler it is read through.
    matcap: usize,
    mesh_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// The wireframe is shown, and its lines while it is (made from each
    /// whole mesh that arrives, and kept up with its parts).
    wire: bool,
    lines: Option<Lines>,
    /// A picture on its way back from the card.
    still: Option<PendingStill>,
    /// The active object is shown (it can be hidden like any other).
    show_model: bool,
}

impl Gpu {
    pub fn new(device: &wgpu::Device, queue: &wgpu::Queue, page_format: wgpu::TextureFormat, matcap: &image::RgbaImage) -> Gpu {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("golem shader"),
            // The finishes (finish.wgsl) are part of the same module.
            source: wgpu::ShaderSource::Wgsl(concat!(include_str!("shader.wgsl"), include_str!("finish.wgsl")).into()),
        });
        let globals = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("globals"),
            size: std::mem::size_of::<Globals>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let matcap_tex = matcap_texture(device, queue, matcap);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("matcap sampler"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            ..Default::default()
        });

        let uniform_entry = |binding, visibility| wgpu::BindGroupLayoutEntry {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
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
        let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("mesh layout"),
            entries: &[
                uniform_entry(0, wgpu::ShaderStages::VERTEX_FRAGMENT),
                uniform_entry(4, wgpu::ShaderStages::FRAGMENT),
                texture_entry(1, true),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let blit_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("blit layout"),
            entries: &[uniform_entry(0, wgpu::ShaderStages::FRAGMENT), texture_entry(3, false)],
        });
        let finish = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("finish"),
            size: std::mem::size_of::<finish::Uniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let mesh_group = matcap_group(device, &mesh_layout, &globals, &finish, &matcap_tex, &sampler);

        let vertex_buffer = |location, format: wgpu::VertexFormat| (location, format);
        let layouts = [
            vertex_buffer(0, wgpu::VertexFormat::Float32x3),
            vertex_buffer(1, wgpu::VertexFormat::Float32x3),
            vertex_buffer(2, wgpu::VertexFormat::Float32),
            vertex_buffer(3, wgpu::VertexFormat::Float32),
        ];
        let attributes: Vec<[wgpu::VertexAttribute; 1]> = layouts
            .iter()
            .map(|&(location, format)| [wgpu::VertexAttribute { format, offset: 0, shader_location: location }])
            .collect();
        let buffers: Vec<Option<wgpu::VertexBufferLayout>> = layouts
            .iter()
            .zip(&attributes)
            .map(|(&(_, format), attrs)| {
                Some(wgpu::VertexBufferLayout {
                    array_stride: format.size(),
                    step_mode: wgpu::VertexStepMode::Vertex,
                    attributes: attrs,
                })
            })
            .collect();

        let object_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("object layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: true,
                    min_binding_size: wgpu::BufferSize::new(std::mem::size_of::<Object>() as u64),
                },
                count: None,
            }],
        });
        let (objects, object_group) = object_buffer(device, &object_layout, 16);
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("mesh pipeline layout"),
            bind_group_layouts: &[Some(&mesh_layout), Some(&object_layout)],
            immediate_size: 0,
        });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh pipeline"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_mesh"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR,
                    blend: None,
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            // Reverse depth, as the camera's projections are made: near is 1.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::Greater),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });

        // The wireframe: lines over the model, by its own positions, tested
        // against its depth but not written to it (the desktop's wire.rs).
        let wire_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire pipeline"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_wire"),
                compilation_options: Default::default(),
                buffers: &buffers[..1],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_wire"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: COLOR,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: Default::default(),
                bias: Default::default(),
            }),
            multisample: wgpu::MultisampleState { count: SAMPLES, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });

        let blit_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("blit pipeline layout"),
            bind_group_layouts: &[Some(&blit_layout)],
            immediate_size: 0,
        });
        let blit = |format: wgpu::TextureFormat| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("blit pipeline"),
                layout: Some(&blit_pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_blit"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_blit"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend: None, write_mask: wgpu::ColorWrites::ALL })],
                }),
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let blit_pipeline = blit(page_format);
        let still_blit_pipeline = blit(COLOR);

        Gpu {
            mesh_pipeline,
            wire_pipeline,
            blit_pipeline,
            still_blit_pipeline,
            blit_layout,
            globals,
            finish,
            mesh_group,
            mesh: None,
            target: None,
            srgb_target: page_format.is_srgb(),
            generation: 0,
            matcap: 0,
            mesh_layout,
            sampler,
            object_layout,
            objects,
            object_group,
            previews: Default::default(),
            wire: false,
            lines: None,
            still: None,
            show_model: true,
        }
    }

    fn set_matcap(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, image: &image::RgbaImage) {
        let texture = matcap_texture(device, queue, image);
        self.mesh_group = matcap_group(device, &self.mesh_layout, &self.globals, &self.finish, &texture, &self.sampler);
    }

    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, upload: &Upload) {
        match upload {
            Upload::Whole { generation, positions, normals, mask, curv, indices, index_count, vertex_room, index_room } => {
                let make = |label, contents: &[u8], room: u64, usage| {
                    // A buffer may not be empty, and is a whole number of 4 bytes.
                    let size = (room.max(contents.len() as u64).max(16) + 3) & !3;
                    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                        label: Some(label),
                        size,
                        usage: usage | wgpu::BufferUsages::COPY_DST,
                        mapped_at_creation: false,
                    });
                    if !contents.is_empty() {
                        queue.write_buffer(&buffer, 0, contents);
                    }
                    buffer
                };
                let vertex = wgpu::BufferUsages::VERTEX;
                self.mesh = Some(MeshBuffers {
                    positions: make("positions", positions, vertex_room * 12, vertex),
                    normals: make("normals", normals, vertex_room * 12, vertex),
                    mask: make("mask", mask, vertex_room * 4, vertex),
                    curv: make("curvature", curv, vertex_room * 4, vertex),
                    indices: make("indices", indices, index_room * 4, wgpu::BufferUsages::INDEX),
                    index_count: *index_count,
                });
                self.generation = *generation;
                // The wireframe's lines, with the same room to grow.
                self.lines = None;
                if self.wire {
                    let triangles = (*index_room).max(indices.len() as u64 / 4) / 3;
                    let buffer = make("wire lines", &[], triangles * LINE_CORNERS_PER_TRIANGLE * 4, wgpu::BufferUsages::INDEX);
                    write_lines(queue, &buffer, 0, indices);
                    self.lines = Some(Lines { buffer });
                }
            }
            Upload::Parts { generation, positions, normals, mask, curv, indices, index_count } => {
                // Parts of a mesh the card does not hold would land in the wrong one.
                let Some(mesh) = self.mesh.as_mut().filter(|_| self.generation == *generation) else { return };
                if let Some(count) = index_count {
                    mesh.index_count = (*count).min((mesh.indices.size() / 4) as u32);
                }
                let mesh = &*mesh;
                let lists = [
                    (&mesh.positions, positions),
                    (&mesh.normals, normals),
                    (&mesh.mask, mask),
                    (&mesh.curv, curv),
                    (&mesh.indices, indices),
                ];
                for (buffer, parts) in lists {
                    for (offset, bytes) in parts {
                        if !bytes.is_empty() && offset + bytes.len() as u64 <= buffer.size() {
                            queue.write_buffer(buffer, *offset, bytes);
                        }
                    }
                }
                // Triangles that changed change their lines, at the same place.
                if let Some(lines) = &self.lines {
                    for (offset, bytes) in indices {
                        write_lines(queue, &lines.buffer, offset / 12, bytes);
                    }
                }
            }
        }
    }

    /// Puts the tool's previews on the card (only the ones that changed) and
    /// writes every drawn thing's place: the model first, then each copy of
    /// each preview. Returns (preview id, slot) for every draw.
    fn prepare_objects(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        previews: &[PreviewDraw],
        model: glam::Mat4,
        others: &[Other],
    ) -> Vec<(u64, u32)> {
        for p in self.previews.values_mut() {
            p.used = false;
        }
        // The active object where it stands; a tool works in its space, so
        // the tool's previews stand where it does too.
        let mut slots = vec![Object::at(model, [1.0, 1.0, 1.0, 0.0])];
        let mut draws = Vec::new();
        for other in others {
            let d = &other.drawn;
            let id = d.id | OTHER;
            if !self.previews.contains_key(&id) {
                let zeros = vec![0u8; d.vertices * 4];
                let make = |label, contents: &[u8], usage| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(label),
                        contents: if contents.is_empty() { &[0; 16] } else { contents },
                        usage,
                    })
                };
                let mesh = MeshBuffers {
                    positions: make("object positions", &d.positions, wgpu::BufferUsages::VERTEX),
                    normals: make("object normals", &d.normals, wgpu::BufferUsages::VERTEX),
                    mask: make("object mask", &zeros, wgpu::BufferUsages::VERTEX),
                    curv: make("object curvature", &zeros, wgpu::BufferUsages::VERTEX),
                    indices: make("object indices", &d.indices, wgpu::BufferUsages::INDEX),
                    index_count: d.index_count,
                };
                self.previews.insert(id, PreviewBuffers { version: 0, mesh, used: true });
            }
            if let Some(p) = self.previews.get_mut(&id) {
                p.used = true;
            }
            draws.push((id, slots.len() as u32));
            slots.push(Object::at(other.model, [1.0, 1.0, 1.0, 0.0]));
        }
        for draw in previews {
            let preview = &draw.mesh;
            if preview.is_empty() {
                continue;
            }
            let id = preview.id();
            let stale = self.previews.get(&id).is_none_or(|p| p.version != preview.version());
            if stale {
                let data = preview.data();
                let zeros = vec![0u8; data.positions.len() * 4];
                let make = |label, contents: &[u8], usage| {
                    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                        label: Some(label),
                        contents: if contents.is_empty() { &[0; 16] } else { contents },
                        usage,
                    })
                };
                let mesh = MeshBuffers {
                    positions: make("preview positions", bytemuck::cast_slice(&data.positions), wgpu::BufferUsages::VERTEX),
                    normals: make("preview normals", bytemuck::cast_slice(&data.normals), wgpu::BufferUsages::VERTEX),
                    mask: make("preview mask", &zeros, wgpu::BufferUsages::VERTEX),
                    curv: make("preview curvature", &zeros, wgpu::BufferUsages::VERTEX),
                    indices: make("preview indices", bytemuck::cast_slice(&data.indices), wgpu::BufferUsages::INDEX),
                    index_count: data.indices.len() as u32,
                };
                self.previews.insert(id, PreviewBuffers { version: preview.version(), mesh, used: true });
            }
            if let Some(p) = self.previews.get_mut(&id) {
                p.used = true;
            }
            let [r, g, b, _] = draw.style.color.to_normalized_gamma_f32();
            for t in &draw.transforms {
                draws.push((id, slots.len() as u32));
                slots.push(Object::at(model * *t, [r, g, b, 0.85]));
            }
        }
        self.previews.retain(|_, p| p.used);

        let needed = slots.len() as u64 * OBJECT_STRIDE;
        if needed > self.objects.size() {
            let capacity = (slots.len() * 2).max(16);
            (self.objects, self.object_group) = object_buffer(device, &self.object_layout, capacity);
        }
        let mut bytes = vec![0u8; needed as usize];
        for (i, slot) in slots.iter().enumerate() {
            let at = i * OBJECT_STRIDE as usize;
            bytes[at..at + std::mem::size_of::<Object>()].copy_from_slice(bytemuck::bytes_of(slot));
        }
        queue.write_buffer(&self.objects, 0, &bytes);
        draws
    }

    fn ensure_target(&mut self, device: &wgpu::Device, size: [u32; 2]) {
        // WebGL2 caps textures at 2048 pixels a side: past that the model is
        // drawn smaller and stretched to fit, rather than not at all.
        let max = device.limits().max_texture_dimension_2d.max(1);
        let size = size.map(|v| v.clamp(1, max));
        if self.target.as_ref().is_some_and(|t| t.size == size) {
            return;
        }
        self.target = Some(self.make_target(device, size, &self.globals));
    }

    /// The textures a scene is drawn into at `size`: smoothed (four samples
    /// a pixel), then resolved into one the copy reads through `globals`.
    fn make_target(&self, device: &wgpu::Device, size: [u32; 2], globals: &wgpu::Buffer) -> Target {
        let texture = |label, format, samples, usage| {
            device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: samples,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        let msaa = texture("viewport msaa", COLOR, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let resolved = texture(
            "viewport",
            COLOR,
            1,
            wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        let depth = texture("viewport depth", DEPTH, SAMPLES, wgpu::TextureUsages::RENDER_ATTACHMENT);
        let blit_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("blit group"),
            layout: &self.blit_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(&resolved) },
            ],
        });
        Target { size, msaa, resolved, depth, blit_group }
    }

    /// Draws the model (only `runs` of its corners), the tool's previews and
    /// the wireframe into a pass already begun on a target.
    fn draw_scene(&self, pass: &mut wgpu::RenderPass<'_>, draws: &[(u64, u32)], runs: &[Range<u32>]) {
        pass.set_pipeline(&self.mesh_pipeline);
        pass.set_bind_group(0, &self.mesh_group, &[]);
        let bind = |pass: &mut wgpu::RenderPass<'_>, mesh: &MeshBuffers, slot: u32| {
            pass.set_bind_group(1, &self.object_group, &[slot * OBJECT_STRIDE as u32]);
            pass.set_vertex_buffer(0, mesh.positions.slice(..));
            pass.set_vertex_buffer(1, mesh.normals.slice(..));
            pass.set_vertex_buffer(2, mesh.mask.slice(..));
            pass.set_vertex_buffer(3, mesh.curv.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
        };
        if let Some(mesh) = self.mesh.as_ref().filter(|m| m.index_count > 0 && self.show_model) {
            bind(pass, mesh, 0);
            for run in runs {
                pass.draw_indexed(run.clone(), 0, 0..1);
            }
        }
        for &(id, slot) in draws {
            if let Some(p) = self.previews.get(&id).filter(|p| p.mesh.index_count > 0) {
                bind(pass, &p.mesh, slot);
                pass.draw_indexed(0..p.mesh.index_count, 0, 0..1);
            }
        }
        if let (true, true, Some(mesh), Some(lines)) = (self.wire, self.show_model, &self.mesh, &self.lines) {
            pass.set_pipeline(&self.wire_pipeline);
            pass.set_bind_group(1, &self.object_group, &[0]);
            pass.set_vertex_buffer(0, mesh.positions.slice(..));
            pass.set_index_buffer(lines.buffer.slice(..), wgpu::IndexFormat::Uint32);
            let room = (lines.buffer.size() / 4) as u32;
            let per = LINE_CORNERS_PER_TRIANGLE as u32;
            for run in runs {
                let (start, end) = (run.start / 3 * per, (run.end / 3 * per).min(room));
                if start < end {
                    pass.draw_indexed(start..end, 0, 0..1);
                }
            }
        }
    }

    /// The runs of the model's corners to draw: those `culled` names when
    /// they belong to the mesh the card holds, else all of it.
    fn runs(&self, culled: Option<&Culled>) -> Vec<Range<u32>> {
        let Some(mesh) = &self.mesh else { return Vec::new() };
        match culled {
            Some(c) if c.generation == self.generation && c.total == mesh.index_count => c.runs.clone(),
            _ => vec![0..mesh.index_count],
        }
    }

    /// Takes up a picture to save: draws the scene into a picture of its own
    /// size, copies it into a buffer the processor can read, and starts that
    /// buffer on its way back. `globals` are this frame's.
    fn start_still(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        request: Arc<still::Request>,
        globals: &Globals,
        draws: &[(u64, u32)],
    ) {
        let size = still::fit(request.size, device.limits().max_texture_dimension_2d);
        let mut picture_globals = *globals;
        // The picture keeps the colours as drawn (it is not an sRGB
        // texture), with or without the backdrop.
        picture_globals.params[1] = 0.0;
        picture_globals.look[2] = if request.transparent { 1.0 } else { 0.0 };
        let globals_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("picture globals"),
            contents: bytemuck::bytes_of(&picture_globals),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let target = self.make_target(device, size, &globals_buffer);
        let extent = wgpu::Extent3d { width: size[0], height: size[1], depth_or_array_layers: 1 };
        let picture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("picture"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: COLOR,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let picture_view = picture.create_view(&Default::default());
        let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
        let row = (size[0] * 4).div_ceil(align) * align;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("picture read back"),
            size: row as u64 * size[1] as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor { label: Some("picture") });
        {
            let mut pass = begin_scene_pass(&mut encoder, &target);
            let runs = self.runs(None);
            self.draw_scene(&mut pass, draws, &runs);
        }
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("picture copy"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &picture_view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Store },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.still_blit_pipeline);
            pass.set_bind_group(0, &target.blit_group, &[]);
            pass.draw(0..3, 0..1);
        }
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo { texture: &picture, mip_level: 0, origin: wgpu::Origin3d::ZERO, aspect: wgpu::TextureAspect::All },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout { offset: 0, bytes_per_row: Some(row), rows_per_image: Some(size[1]) },
            },
            extent,
        );
        queue.submit([encoder.finish()]);
        let mapped = Arc::new(AtomicU8::new(0));
        let flag = mapped.clone();
        buffer.map_async(wgpu::MapMode::Read, .., move |result| {
            flag.store(if result.is_ok() { 1 } else { 2 }, Ordering::Release);
        });
        self.still = Some(PendingStill { request, buffer, size, row, mapped });
    }

    /// Looks whether the picture on its way back has arrived, and if so
    /// hands it to the request that asked for it.
    fn poll_still(&mut self, device: &wgpu::Device) {
        let Some(pending) = &self.still else { return };
        // WebGL2 hands a mapped buffer over when asked; WebGPU by itself.
        let _ = device.poll(wgpu::PollType::Poll);
        match pending.mapped.load(Ordering::Acquire) {
            0 => return,
            1 => {
                let [w, h] = pending.size;
                let mut pixels = Vec::with_capacity(w as usize * h as usize * 4);
                if let Ok(view) = pending.buffer.get_mapped_range(..) {
                    for y in 0..h as usize {
                        let at = y * pending.row as usize;
                        pixels.extend_from_slice(&view[at..at + w as usize * 4]);
                    }
                }
                pending.buffer.unmap();
                let picture = image::RgbaImage::from_raw(w, h, pixels).ok_or_else(|| "The picture came back short.".to_owned());
                pending.request.finish(picture);
            }
            _ => pending.request.finish(Err("The graphics card did not hand the picture back.".into())),
        }
        self.still = None;
    }
}

/// Begins a pass that draws a scene into `target`: cleared to nothing (the
/// copy puts the backdrop behind it), depth cleared to the far side.
fn begin_scene_pass<'e>(encoder: &'e mut wgpu::CommandEncoder, target: &Target) -> wgpu::RenderPass<'e> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("viewport"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &target.msaa,
            depth_slice: None,
            resolve_target: Some(&target.resolved),
            ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
        })],
        depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
            view: &target.depth,
            depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    })
}

/// Writes the wireframe's lines for the triangles in `indices` (corner
/// numbers as little-endian bytes, three a triangle), the first of which is
/// triangle `first`. A triangle keeps the sides that run from a lower vertex
/// number to a higher: the triangle on the other side of a side runs it the
/// other way, so each side is drawn once (as on the desktop). That is one or
/// two sides; a slot left over is a line of no length, which draws nothing.
fn write_lines(queue: &wgpu::Queue, buffer: &wgpu::Buffer, first: u64, indices: &[u8]) {
    /// Triangles turned into lines at a time, so a big mesh's lines never
    /// need one big list of their own in memory.
    const BLOCK: usize = 1 << 16;
    let mut out: Vec<u32> = Vec::with_capacity(BLOCK * LINE_CORNERS_PER_TRIANGLE as usize);
    for (b, block) in indices.chunks(BLOCK * 12).enumerate() {
        out.clear();
        for t in block.chunks_exact(12) {
            let corner = |k: usize| u32::from_le_bytes([t[k * 4], t[k * 4 + 1], t[k * 4 + 2], t[k * 4 + 3]]);
            let (a, b, c) = (corner(0), corner(1), corner(2));
            let mut kept = 0;
            for (p, q) in [(a, b), (b, c), (c, a)] {
                if p < q && kept < 2 {
                    out.extend_from_slice(&[p, q]);
                    kept += 1;
                }
            }
            for _ in kept..2 {
                out.extend_from_slice(&[a, a]);
            }
        }
        let offset = (first + (b * BLOCK) as u64) * LINE_CORNERS_PER_TRIANGLE * 4;
        let bytes: &[u8] = bytemuck::cast_slice(&out);
        if !bytes.is_empty() && offset + bytes.len() as u64 <= buffer.size() {
            queue.write_buffer(buffer, offset, bytes);
        }
    }
}

/// The runs of a big mesh's corners in view, for the mesh the card holds as
/// generation `generation` with `total` corners in its index buffer.
pub struct Culled {
    generation: u64,
    total: u32,
    runs: Vec<Range<u32>>,
}

/// Drawing only what is in view: the mesh cut into pieces (the desktop's
/// `chunks.rs`), kept up with the mesh as it changes. It lives in the app,
/// which has the mesh; each frame it hands the renderer the runs in view.
#[derive(Default)]
pub struct Culling {
    chunks: chunks::Chunks,
    /// What the pieces were cut from: the mesh's number, its topology's and
    /// its hidden faces' versions, its triangles and its tree's leaves.
    key: Option<(u64, u64, u64, usize, usize)>,
    runs: Vec<Range<u32>>,
}

impl Culling {
    /// The runs of `mesh` that `view_proj` sees, once `upload` (what goes to
    /// the card this frame, as generation `generation`) is there. `None`,
    /// for all of it, for a small mesh.
    pub fn update(&mut self, mesh: &Mesh, upload: Option<&Upload>, view_proj: &glam::Mat4, generation: u64) -> Option<Culled> {
        if mesh.triangle_count() < CULL_FROM_TRIANGLES {
            self.key = None;
            return None;
        }
        let key = (mesh.uid, mesh.topology_version(), mesh.visibility_version(), mesh.triangle_count(), mesh.bvh.leaves.len());
        let rebuild = self.key != Some(key)
            || match upload {
                Some(Upload::Whole { .. }) => true,
                // The count of corners is in every upload now (app.rs `upload`);
                // a change of it is in the key already, as the triangle count.
                Some(Upload::Parts { indices, .. }) => !indices.is_empty(),
                None => false,
            };
        if rebuild {
            self.chunks = chunks::Chunks::build(mesh);
            self.key = Some(key);
        } else if let Some(Upload::Parts { positions, .. }) = upload {
            // Vertices moved: the boxes round the pieces follow the tree's.
            if !positions.is_empty() {
                self.chunks.refresh(mesh);
            }
        }
        self.chunks.visible(view_proj, &mut self.runs);
        Some(Culled { generation, total: self.chunks.all().end, runs: self.runs.clone() })
    }

    /// The share of the mesh's corners last found in view (1 when all).
    pub fn share_in_view(&self) -> f32 {
        let total = self.chunks.all().end;
        if self.key.is_none() || total == 0 {
            return 1.0;
        }
        self.runs.iter().map(|r| r.len() as f32).sum::<f32>() / total as f32
    }
}

/// One frame of the viewport, handed to egui as a paint callback.
pub struct Viewport {
    pub globals: Globals,
    pub upload: Option<Arc<Upload>>,
    /// The rectangle's size in physical pixels.
    pub size: [u32; 2],
    /// Where the generation of the mesh the card holds is written, so the
    /// app can tell a whole mesh that never arrived (a frame the page did not
    /// draw) and send it again.
    pub has: Arc<AtomicU64>,
    /// The number of this frame's upload, and where the number of the last
    /// one the card took is written: changes the app sent in a frame the
    /// page never drew are sent again until one is (`GolemApp::upload`).
    pub upload_seq: u64,
    pub took: Arc<AtomicU64>,
    /// What a tool asks to have drawn this frame (Tubes' tubes).
    pub previews: Vec<PreviewDraw>,
    /// The matcap to draw with, of these.
    pub matcap: usize,
    pub matcaps: Arc<Vec<image::RgbaImage>>,
    /// Draw the wireframe over the model.
    pub wire: bool,
    /// Set by the renderer when it needs the mesh sent whole (the wireframe
    /// was turned on, and its lines are made from a whole mesh).
    pub wants_whole: Arc<AtomicBool>,
    /// The runs of the model in view, for a big mesh (`Culling`).
    pub culled: Option<Culled>,
    /// A picture to save, drawn the first frame it is here.
    pub still: Option<Arc<still::Request>>,
    /// The material finish to shade the model with, or the matcap.
    pub finish: finish::Uniform,
    /// Where the active object stands, and whether it is shown.
    pub model: glam::Mat4,
    pub show_model: bool,
    /// Every other object that is shown.
    pub others: Vec<Other>,
}

impl CallbackTrait for Viewport {
    fn prepare(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        _screen: &ScreenDescriptor,
        encoder: &mut wgpu::CommandEncoder,
        resources: &mut CallbackResources,
    ) -> Vec<wgpu::CommandBuffer> {
        let Some(gpu) = resources.get_mut::<Gpu>() else { return Vec::new() };
        if !self.wire {
            gpu.lines = None;
        }
        gpu.wire = self.wire;
        if let Some(upload) = &self.upload {
            gpu.upload(device, queue, upload);
        }
        self.took.fetch_max(self.upload_seq, Ordering::Relaxed);
        if self.wire && gpu.lines.is_none() && gpu.mesh.is_some() {
            self.wants_whole.store(true, Ordering::Relaxed);
        }
        if gpu.matcap != self.matcap {
            if let Some(image) = self.matcaps.get(self.matcap) {
                gpu.set_matcap(device, queue, image);
            }
            gpu.matcap = self.matcap;
        }
        gpu.show_model = self.show_model;
        let draws = gpu.prepare_objects(device, queue, &self.previews, self.model, &self.others);
        self.has.store(gpu.generation, Ordering::Relaxed);
        let size = [self.size[0].max(1), self.size[1].max(1)];
        gpu.ensure_target(device, size);
        let mut globals = self.globals;
        globals.params[1] = if gpu.srgb_target { 1.0 } else { 0.0 };
        globals.look[2] = 0.0;
        queue.write_buffer(&gpu.globals, 0, bytemuck::bytes_of(&globals));
        queue.write_buffer(&gpu.finish, 0, bytemuck::bytes_of(&self.finish));

        // A picture to save: one on its way back is looked for, a new one drawn.
        gpu.poll_still(device);
        if let Some(request) = self.still.as_ref().filter(|r| gpu.still.is_none() && r.begin()) {
            gpu.start_still(device, queue, request.clone(), &globals, &draws);
        }

        let runs = gpu.runs(self.culled.as_ref());
        let target = gpu.target.as_ref().expect("made above");
        let mut pass = begin_scene_pass(encoder, target);
        gpu.draw_scene(&mut pass, &draws, &runs);
        Vec::new()
    }

    fn paint(&self, _info: egui::PaintCallbackInfo, pass: &mut wgpu::RenderPass<'static>, resources: &CallbackResources) {
        let Some(gpu) = resources.get::<Gpu>() else { return };
        let Some(target) = &gpu.target else { return };
        pass.set_pipeline(&gpu.blit_pipeline);
        pass.set_bind_group(0, &target.blit_group, &[]);
        pass.draw(0..3, 0..1);
    }
}

pub fn callback(rect: egui::Rect, viewport: Viewport) -> egui::PaintCallback {
    egui_wgpu::Callback::new_paint_callback(rect, viewport)
}

fn object_buffer(device: &wgpu::Device, layout: &wgpu::BindGroupLayout, capacity: usize) -> (wgpu::Buffer, wgpu::BindGroup) {
    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("objects"),
        size: capacity as u64 * OBJECT_STRIDE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let group = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("objects"),
        layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                buffer: &buffer,
                offset: 0,
                size: wgpu::BufferSize::new(std::mem::size_of::<Object>() as u64),
            }),
        }],
    });
    (buffer, group)
}

fn matcap_texture(device: &wgpu::Device, queue: &wgpu::Queue, image: &image::RgbaImage) -> wgpu::Texture {
    let (w, h) = image.dimensions();
    device.create_texture_with_data(
        queue,
        &wgpu::TextureDescriptor {
            label: Some("matcap"),
            size: wgpu::Extent3d { width: w, height: h, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
        wgpu::util::TextureDataOrder::LayerMajor,
        image.as_raw(),
    )
}

fn matcap_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    globals: &wgpu::Buffer,
    finish: &wgpu::Buffer,
    texture: &wgpu::Texture,
    sampler: &wgpu::Sampler,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("mesh group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry { binding: 0, resource: globals.as_entire_binding() },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::TextureView(&texture.create_view(&Default::default())),
            },
            wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(sampler) },
            wgpu::BindGroupEntry { binding: 4, resource: finish.as_entire_binding() },
        ],
    })
}
