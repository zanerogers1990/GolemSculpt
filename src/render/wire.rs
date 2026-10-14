//! Wireframe overlay: the edges of every drawn object's faces, as lines over
//! the finished picture. A quad-remeshed mesh shows its quads; any other mesh
//! shows its triangles.
//!
//! The lines index straight into the positions the renderer already keeps for
//! each mesh, so they follow every brush stroke with nothing to re-send; only
//! a change of topology rebuilds them.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use rayon::prelude::*;

use super::cache::Stamp;
use super::{DEPTH_FORMAT, DrawItem, Globals, Gpu, MSAA_SAMPLES, MeshBuffers};
use crate::mesh::Mesh;

/// How far toward the eye the lines sit, as a share of their distance.
const LIFT: f32 = 0.0015;
/// Display colour and opacity of the lines.
const COLOR: [f32; 4] = [0.02, 0.025, 0.035, 0.6];

/// Mirrors `Uniforms` in wire.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    cam_forward: [f32; 4],
    color: [f32; 4],
    params: [f32; 4],
}

struct Lines {
    /// `Mesh::uid` the lines were built from.
    uid: u64,
    indices: wgpu::Buffer,
    count: u32,
}

pub struct Layer {
    /// Whether the overlay is shown. Lines are only built while it is.
    pub enabled: bool,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    pipeline: wgpu::RenderPipeline,
    /// Keyed by the scene's mesh id.
    lines: HashMap<u64, Lines>,
    /// Moves when lines are built or let go (see cache.rs).
    pub(super) stamp: Stamp,
}

impl Layer {
    pub fn new(gpu: &Gpu) -> Layer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("wire"),
            source: wgpu::ShaderSource::Wgsl(include_str!("wire.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("wire"),
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
            label: Some("wire"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("wire"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("wire"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_wire"),
                compilation_options: Default::default(),
                buffers: &[
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 12,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0 => Float32x3],
                    }),
                    // The renderer's per-object matrices.
                    Some(wgpu::VertexBufferLayout {
                        array_stride: 64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4],
                    }),
                ],
            },
            primitive: wgpu::PrimitiveState { topology: wgpu::PrimitiveTopology::LineList, ..Default::default() },
            // Reverse depth, as in the main pass; tested but not written.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_wire"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Layer { enabled: false, uniforms, bind, pipeline, lines: HashMap::new(), stamp: Stamp::default() }
    }

    /// Builds the lines of mesh `id` if the overlay is on and they are
    /// missing or belong to a mesh that has since been replaced.
    pub(super) fn sync(&mut self, gpu: &Gpu, id: u64, mesh: &Mesh) {
        if !self.enabled || self.lines.get(&id).is_some_and(|l| l.uid == mesh.uid) {
            return;
        }
        let lines = lines_of(mesh);
        self.stamp.bump();
        if lines.is_empty() {
            self.lines.remove(&id);
            return;
        }
        let indices = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("wire lines"),
            size: lines.len() as u64 * 4,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&indices, 0, bytemuck::cast_slice(&lines));
        self.lines.insert(id, Lines { uid: mesh.uid, indices, count: lines.len() as u32 });
    }

    /// Frees the lines of meshes the scene no longer has.
    pub(super) fn retain(&mut self, keep: impl Fn(u64) -> bool) {
        let before = self.lines.len();
        self.lines.retain(|&id, _| self.enabled && keep(id));
        if self.lines.len() != before {
            self.stamp.bump();
        }
    }

    /// Whether anything of the wireframe is drawn.
    pub(super) fn active(&self) -> bool {
        self.enabled && !self.lines.is_empty()
    }

    /// Draws into the overlay pass, over the finished picture and behind
    /// whatever of the scene is in front.
    pub(super) fn draw(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass,
        globals: &Globals,
        meshes: &HashMap<u64, MeshBuffers>,
        objects: Option<&wgpu::Buffer>,
        items: &[DrawItem],
    ) {
        let (true, Some(objects)) = (self.enabled, objects) else { return };
        let ortho = if globals.viewport[2] == 0.0 { 1.0 } else { 0.0 };
        let [x, y, z, _] = globals.cam_pos;
        let uniforms = Uniforms {
            view_proj: globals.view_proj,
            cam_pos: [x, y, z, ortho],
            cam_forward: globals.cam_forward,
            color: COLOR,
            params: [LIFT, 0.0, 0.0, 0.0],
        };
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(1, objects.slice(..));
        for (i, item) in items.iter().enumerate() {
            let (Some(mesh), Some(lines)) = (meshes.get(&item.mesh), self.lines.get(&item.mesh)) else { continue };
            if lines.uid != mesh.uid {
                continue;
            }
            pass.set_vertex_buffer(0, mesh.positions.slice(..));
            pass.set_index_buffer(lines.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..lines.count, 0, i as u32..i as u32 + 1);
        }
    }
}

/// Vertex pairs, one per edge: of the quads a quad remesh left, else of the
/// triangles.
fn lines_of(mesh: &Mesh) -> Vec<u32> {
    // Part of the mesh is hidden: only the edges of the faces that show.
    if let Some(lines) = mesh.visible_wire_lines() {
        return lines;
    }
    if let Some(polys) = &mesh.polys {
        let mut edges: Vec<(u32, u32)> = Vec::with_capacity(polys.indices.len());
        for f in polys.start.windows(2) {
            let face = &polys.indices[f[0] as usize..f[1] as usize];
            for (k, &a) in face.iter().enumerate() {
                let b = face[(k + 1) % face.len()];
                edges.push((a.min(b), a.max(b)));
            }
        }
        edges.par_sort_unstable();
        edges.dedup();
        return edges.into_iter().flat_map(|(a, b)| [a, b]).collect();
    }
    // Two triangles run a shared edge in opposite directions, so keeping the
    // sides that run low -> high takes each edge once without sorting
    // millions of them.
    mesh.indices
        .par_chunks_exact(3)
        .flat_map_iter(|t| {
            [(t[0], t[1]), (t[1], t[2]), (t[2], t[0])].into_iter().filter(|(a, b)| a < b).flat_map(|(a, b)| [a, b])
        })
        .collect()
}
