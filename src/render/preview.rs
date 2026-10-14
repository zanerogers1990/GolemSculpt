//! The hosted tools' layer of the viewport: the preview meshes they ask for
//! through `sculpt_toolkit::Draw`. Self-contained (its own shader, uniforms
//! and buffers) and drawn in the overlay pass after the scene, so previews
//! are depth-tested against the model.
//!
//! A preview's buffers are made the first time it is drawn and remade only
//! when its version changes; copies of one mesh are GPU instances.

use std::collections::HashMap;
use std::ops::Range;

use bytemuck::{Pod, Zeroable};
use sculpt_toolkit::{Look, PreviewDraw};

use super::cache::Stamp;
use super::{DEPTH_FORMAT, Gpu, MSAA_SAMPLES};
use crate::camera::View;

/// Mirrors `Uniforms` in preview.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_forward: [f32; 4],
}

/// One copy of a mesh; mirrors the instance attributes in preview.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    model: [[f32; 4]; 4],
    /// Linear colour, straight alpha.
    color: [f32; 4],
    /// x = 1: shade by face.
    params: [f32; 4],
}

struct GpuMesh {
    version: u64,
    positions: wgpu::Buffer,
    normals: wgpu::Buffer,
    indices: wgpu::Buffer,
    index_count: u32,
}

/// One mesh drawn with one pipeline at a run of instances.
struct Batch {
    mesh: u64,
    pipeline: usize,
    instances: Range<u32>,
}

pub struct Layer {
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    /// Indexed by [`pipeline_index`].
    pipelines: Vec<wgpu::RenderPipeline>,
    /// Keyed by `Preview::id`.
    meshes: HashMap<u64, GpuMesh>,
    instances: Option<(wgpu::Buffer, u64)>,
    batches: Vec<Batch>,
    /// Moves when what is drawn changes (see cache.rs).
    pub(super) stamp: Stamp,
}

const LOOKS: [Look; 3] = [Look::Solid, Look::Shell, Look::Unlit];

fn pipeline_index(look: Look, on_top: bool) -> usize {
    let look = LOOKS.iter().position(|&l| l == look).unwrap_or(0);
    look * 2 + usize::from(on_top)
}

fn srgb_to_linear(c: u8) -> f32 {
    let v = c as f32 / 255.0;
    if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
}

impl Layer {
    pub fn new(gpu: &Gpu) -> Layer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("tool previews"),
            source: wgpu::ShaderSource::Wgsl(include_str!("preview.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("tool preview uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("tool previews"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("tool previews"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("tool previews"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let buffers = [
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
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![
                    2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4, 7 => Float32x4
                ],
            }),
        ];
        // See-through surfaces test against depth but do not write it:
        // writing would let whichever face drew first hide the others, which
        // reads as random holes as the camera orbits.
        let pipeline = |look: Look, on_top: bool| {
            let (fragment, see_through) = match look {
                Look::Solid => ("fs_solid", false),
                Look::Shell => ("fs_shell", true),
                Look::Unlit => ("fs_unlit", true),
            };
            // Reverse depth, as in the main pass.
            let compare = if on_top { wgpu::CompareFunction::Always } else { wgpu::CompareFunction::GreaterEqual };
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("tool preview"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_preview"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!see_through && !on_top),
                    depth_compare: Some(compare),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.format,
                        blend: see_through.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let pipelines = LOOKS.iter().flat_map(|&look| [pipeline(look, false), pipeline(look, true)]).collect();
        Layer {
            uniforms,
            bind,
            pipelines,
            meshes: HashMap::new(),
            instances: None,
            batches: Vec::new(),
            stamp: Stamp::default(),
        }
    }

    /// Sets what the layer draws this frame. A mesh is only sent again when
    /// its version has changed; meshes no longer drawn are let go.
    pub fn show(&mut self, gpu: &Gpu, view: &View, draws: &[PreviewDraw]) {
        self.batches.clear();
        if draws.is_empty() {
            self.meshes.clear();
            self.stamp.note(&[]);
            return;
        }
        let ortho = if view.ortho_half_height.is_some() { 1.0 } else { 0.0 };
        let uniforms = Uniforms {
            view_proj: view.view_proj.to_cols_array_2d(),
            cam_pos: view.pos.extend(ortho).to_array(),
            cam_right: view.right.extend(0.0).to_array(),
            cam_up: view.up.extend(0.0).to_array(),
            cam_forward: view.forward.extend(0.0).to_array(),
        };
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        // Everything solid first, then the see-through surfaces over it, and
        // what asked to be on top last of all; within each, the order given.
        let mut order: Vec<usize> = (0..draws.len()).collect();
        order.sort_by_key(|&i| (draws[i].style.on_top, draws[i].style.look != Look::Solid));

        let mut instances: Vec<Instance> = Vec::new();
        for i in order {
            let draw = &draws[i];
            let id = draw.mesh.id();
            self.sync_mesh(gpu, id, &draw.mesh);
            let [r, g, b, a] = draw.style.color.to_srgba_unmultiplied();
            let color = [srgb_to_linear(r), srgb_to_linear(g), srgb_to_linear(b), a as f32 / 255.0];
            let params = [if draw.style.flat { 1.0 } else { 0.0 }, 0.0, 0.0, 0.0];
            let start = instances.len() as u32;
            instances.extend(draw.transforms.iter().map(|m| Instance { model: m.to_cols_array_2d(), color, params }));
            self.batches.push(Batch {
                mesh: id,
                pipeline: pipeline_index(draw.style.look, draw.style.on_top),
                instances: start..instances.len() as u32,
            });
        }
        let batches = &self.batches;
        self.meshes.retain(|id, _| batches.iter().any(|b| b.mesh == *id));

        let bytes: &[u8] = bytemuck::cast_slice(&instances);
        if self.instances.as_ref().is_none_or(|(_, room)| *room < bytes.len() as u64) {
            let room = (bytes.len() as u64).next_power_of_two().max(4096);
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("tool preview instances"),
                size: room,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            self.instances = Some((buffer, room));
        }
        if let Some((buffer, _)) = &self.instances {
            gpu.queue.write_buffer(buffer, 0, bytes);
        }
        let batches: Vec<[u64; 5]> = self
            .batches
            .iter()
            .map(|b| {
                let version = self.meshes.get(&b.mesh).map_or(u64::MAX, |m| m.version);
                [b.mesh, version, b.pipeline as u64, b.instances.start as u64, b.instances.end as u64]
            })
            .collect();
        self.stamp.note(&[bytemuck::bytes_of(&uniforms), bytes, bytemuck::cast_slice(&batches)]);
    }

    /// Whether any preview is drawn this frame.
    pub(super) fn active(&self) -> bool {
        !self.batches.is_empty()
    }

    fn sync_mesh(&mut self, gpu: &Gpu, id: u64, mesh: &sculpt_toolkit::Preview) {
        if self.meshes.get(&id).is_some_and(|m| m.version == mesh.version()) {
            return;
        }
        let data = mesh.data();
        let make = |label, bytes: &[u8], usage| {
            let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: bytes.len().max(4) as u64,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            gpu.queue.write_buffer(&buffer, 0, bytes);
            buffer
        };
        let vertex = wgpu::BufferUsages::VERTEX;
        self.meshes.insert(
            id,
            GpuMesh {
                version: mesh.version(),
                positions: make("tool preview positions", bytemuck::cast_slice(&data.positions), vertex),
                normals: make("tool preview normals", bytemuck::cast_slice(&data.normals), vertex),
                indices: make("tool preview indices", bytemuck::cast_slice(&data.indices), wgpu::BufferUsages::INDEX),
                index_count: data.indices.len() as u32,
            },
        );
    }

    /// Draws into the overlay pass, after the scene.
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        let Some((instances, _)) = &self.instances else { return };
        if self.batches.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(2, instances.slice(..));
        for batch in &self.batches {
            let Some(mesh) = self.meshes.get(&batch.mesh) else { continue };
            if mesh.index_count == 0 {
                continue;
            }
            pass.set_pipeline(&self.pipelines[batch.pipeline]);
            pass.set_vertex_buffer(0, mesh.positions.slice(..));
            pass.set_vertex_buffer(1, mesh.normals.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, batch.instances.clone());
        }
    }
}
