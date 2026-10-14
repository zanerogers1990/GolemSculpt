//! Face group colours in the viewport: every face tinted with the colour of
//! the group it is in (`mesh/groups.rs`), drawn over the finished picture.
//!
//! **Over the picture, not in it.** The colours are an overlay, like the
//! wireframe, drawn after the materials, the lights and the post effects.
//! That is what makes them the same on every look: a matcap, a lit finish
//! and flat shading all come out tinted the same way, with nothing added to
//! the main shader for any of them, and with the overlay off there is not
//! one instruction or byte of it anywhere in a frame. The picture's own
//! shading shows through the tint, so the form still reads.
//!
//! **A colour per face, looked up.** A group is a property of a triangle,
//! and the vertices along a border belong to both groups, so the colour
//! cannot be a fifth vertex buffer beside the mask and the curvature: the
//! border triangles would smear from one colour to the next, or the border
//! vertices would have to be doubled, and every vertex number the engine
//! keeps would change. Instead the triangles are drawn without a vertex
//! buffer: the shader reads the corner's vertex from the mesh's index buffer
//! and its place from the position buffer, both of which the renderer already
//! keeps, and the colour from one small buffer of four bytes a triangle.
//! Nothing of the mesh is copied, a stroke that moves vertices needs nothing
//! re-sent, and the colours are sent again only when the groups, the hidden
//! faces or the triangles themselves change.
//!
//! The other way to a per-face value, the primitive index in the fragment
//! shader, needs a device feature this app does not ask for and not every
//! adapter has.

use std::collections::HashMap;

use rayon::prelude::*;

use super::cache::Stamp;
use super::{DEPTH_FORMAT, DrawItem, Gpu, MSAA_SAMPLES, MeshBuffers};
use crate::mesh::Mesh;
use crate::mesh::groups::colour;

/// What the renderer's position and index buffers are made with besides
/// their own use, so that this layer's shader can read them.
pub(super) const READ: wgpu::BufferUsages = wgpu::BufferUsages::STORAGE;

/// How much of a group's colour is added to the picture before the picture
/// is tinted by it, so that the colour still shows on a black material.
const LIFT: f32 = 0.22;

/// What the colours of a mesh were built from.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Built {
    /// `Mesh::uid`, and the versions of its groups, hidden faces and triangles.
    uid: u64,
    groups: u64,
    visibility: u64,
    topology: u64,
}

impl Built {
    fn of(mesh: &Mesh) -> Built {
        Built {
            uid: mesh.uid,
            groups: mesh.groups_version(),
            visibility: mesh.visibility_version(),
            topology: mesh.topology_version(),
        }
    }
}

struct Tint {
    built: Built,
    /// The renderer's buffers the bind group reads; a new set of either is a
    /// new bind group.
    positions: wgpu::Buffer,
    indices: wgpu::Buffer,
    bind: wgpu::BindGroup,
    /// How many corners to draw: three for each triangle that shows.
    count: u32,
}

pub struct Layer {
    /// Whether the colours are shown. Nothing is built while they are not.
    pub enabled: bool,
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    mesh_layout: wgpu::BindGroupLayout,
    pipeline: wgpu::RenderPipeline,
    /// Keyed by the scene's mesh id. Only meshes that have groups are here.
    tints: HashMap<u64, Tint>,
    /// Moves when colours are built or let go (see cache.rs).
    pub(super) stamp: Stamp,
}

/// The colour of group `id` as the shader unpacks it: red, green, blue and
/// strength, a byte each, lowest byte first. No group is white at no strength.
fn packed(id: u16) -> u32 {
    if id == 0 {
        return 0x00ff_ffff;
    }
    let byte = |x: f32| (x.clamp(0.0, 1.0) * 255.0).round() as u32;
    let [r, g, b] = colour(id);
    byte(r) | byte(g) << 8 | byte(b) << 16 | byte(LIFT) << 24
}

/// One colour for each triangle of `mesh` that shows, in the order the
/// renderer's index buffer has them.
fn colours_of(mesh: &Mesh) -> Vec<u32> {
    let ids = mesh.group_ids();
    match mesh.hidden_triangles() {
        None => ids.par_iter().with_min_len(1 << 14).map(|&id| packed(id)).collect(),
        Some(hidden) => ids.iter().zip(hidden).filter(|(_, hidden)| !**hidden).map(|(&id, _)| packed(id)).collect(),
    }
}

impl Layer {
    pub fn new(gpu: &Gpu) -> Layer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("groups"),
            source: wgpu::ShaderSource::Wgsl(include_str!("groups.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("groups uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let buffer = |binding: u32, ty: wgpu::BufferBindingType| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::VERTEX,
            ty: wgpu::BindingType::Buffer { ty, has_dynamic_offset: false, min_binding_size: None },
            count: None,
        };
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("groups"),
            entries: &[buffer(0, wgpu::BufferBindingType::Uniform)],
        });
        let read = wgpu::BufferBindingType::Storage { read_only: true };
        let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("groups mesh"),
            entries: &[buffer(0, read), buffer(1, read), buffer(2, read)],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("groups"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("groups"),
            bind_group_layouts: &[Some(&bind_layout), Some(&mesh_layout)],
            immediate_size: 0,
        });
        // `colour * (strength + picture)`: see the shader.
        let tint = wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::SrcAlpha,
                dst_factor: wgpu::BlendFactor::Src,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::Zero,
                dst_factor: wgpu::BlendFactor::One,
                operation: wgpu::BlendOperation::Add,
            },
        };
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("groups"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_groups"),
                compilation_options: Default::default(),
                // Only the renderer's per-object matrices: the corners are looked up.
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: 64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &wgpu::vertex_attr_array![1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4],
                })],
            },
            // Both faces, like the main pass and the selection flash: the back
            // of the surface loses the depth test unless the mesh is open there.
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            // Tested against the scene's depth and never written, with the
            // flash's nudge toward the eye for the last-bit differences a
            // driver may leave between two shaders (see flash.rs).
            depth_stencil: Some(wgpu::DepthStencilState {
                format: DEPTH_FORMAT,
                depth_write_enabled: Some(false),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState { constant: 2, slope_scale: 1.0, clamp: 0.0 },
            }),
            multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs_groups"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.format,
                    blend: Some(tint),
                    write_mask: wgpu::ColorWrites::COLOR,
                })],
            }),
            multiview_mask: None,
            cache: None,
        });
        Layer { enabled: false, uniforms, bind, mesh_layout, pipeline, tints: HashMap::new(), stamp: Stamp::default() }
    }

    /// Builds the colours of mesh `id` if they are shown and are missing or
    /// out of date. `buffers` is the renderer's copy of the mesh, just
    /// brought up to date.
    pub(super) fn sync(&mut self, gpu: &Gpu, id: u64, mesh: &Mesh, buffers: Option<&MeshBuffers>) {
        let (true, true, Some(buffers)) = (self.enabled, mesh.has_groups(), buffers) else {
            self.forget(id);
            return;
        };
        // A mesh too large for the device to hand a shader its buffers goes uncoloured.
        let most = gpu.device.limits().max_storage_buffer_binding_size as u64;
        if buffers.positions.size() > most || buffers.indices.size() > most {
            self.forget(id);
            return;
        }
        let built = Built::of(mesh);
        let current =
            |tint: &Tint| tint.built == built && tint.positions == buffers.positions && tint.indices == buffers.indices;
        if self.tints.get(&id).is_some_and(current) {
            return;
        }
        let colours = colours_of(mesh);
        let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("group colours"),
            // Never of no size: a buffer that is has no binding.
            size: colours.len().max(1) as u64 * 4,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        gpu.queue.write_buffer(&buffer, 0, bytemuck::cast_slice(&colours));
        fn entry(binding: u32, buffer: &wgpu::Buffer) -> wgpu::BindGroupEntry<'_> {
            wgpu::BindGroupEntry { binding, resource: buffer.as_entire_binding() }
        }
        let bind = gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("groups mesh"),
            layout: &self.mesh_layout,
            entries: &[entry(0, &buffers.positions), entry(1, &buffers.indices), entry(2, &buffer)],
        });
        let (positions, indices) = (buffers.positions.clone(), buffers.indices.clone());
        // Never more corners than there are colours for, whatever the two were built from.
        let count = buffers.index_count.min(colours.len() as u32 * 3);
        self.tints.insert(id, Tint { built, positions, indices, bind, count });
        self.stamp.bump();
    }

    fn forget(&mut self, id: u64) {
        if self.tints.remove(&id).is_some() {
            self.stamp.bump();
        }
    }

    /// Whether anything of the colours is drawn.
    pub(super) fn active(&self) -> bool {
        self.enabled && !self.tints.is_empty()
    }

    /// Frees the colours of meshes the scene no longer has, and all of them
    /// once the colours are switched off.
    pub(super) fn retain(&mut self, keep: impl Fn(u64) -> bool) {
        let before = self.tints.len();
        self.tints.retain(|&id, _| self.enabled && keep(id));
        if self.tints.len() != before {
            self.stamp.bump();
        }
    }

    /// Draws into the overlay pass, over the finished picture and under the
    /// wireframe.
    pub(super) fn draw(
        &self,
        gpu: &Gpu,
        pass: &mut wgpu::RenderPass,
        view_proj: &[[f32; 4]; 4],
        meshes: &HashMap<u64, MeshBuffers>,
        objects: Option<&wgpu::Buffer>,
        items: &[DrawItem],
        runs: &[Vec<std::ops::Range<u32>>],
    ) {
        let (true, false, Some(objects)) = (self.enabled, self.tints.is_empty(), objects) else { return };
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(view_proj));
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, objects.slice(..));
        for (i, item) in items.iter().enumerate() {
            let (Some(mesh), Some(tint)) = (meshes.get(&item.mesh), self.tints.get(&item.mesh)) else { continue };
            // Colours of a mesh that has since been swapped are not its colours.
            if tint.built.uid != mesh.uid || tint.positions != mesh.positions || tint.indices != mesh.indices {
                continue;
            }
            pass.set_bind_group(1, &tint.bind, &[]);
            // Only the parts in view, as the scene drew them (render/chunks.rs).
            let end = tint.count.min(mesh.index_count);
            for run in &runs[i] {
                if run.start < end {
                    pass.draw(run.start..run.end.min(end), i as u32..i as u32 + 1);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    #[test]
    fn there_is_one_colour_for_each_triangle_that_shows_in_the_order_they_are_drawn() {
        let mut mesh = primitives::build(Primitive::Sphere, 2_000, 0.5);
        let ids: Vec<u16> = (0..mesh.triangle_count()).map(|t| (t % 5) as u16).collect();
        mesh.set_group_ids(ids.clone());
        let all = colours_of(&mesh);
        assert_eq!(all.len(), mesh.triangle_count());
        assert!(all.iter().zip(&ids).all(|(&c, &id)| c == packed(id)));
        // No group: white, at no strength, which the blend leaves as the picture.
        assert_eq!(packed(0), 0x00ff_ffff);
        assert!(packed(3) >> 24 == (LIFT * 255.0).round() as u32 && packed(3) != packed(4));

        // With faces hidden the list is as long as what is drawn, and still lines up with it.
        let hidden: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|t| t % 3 == 0).collect();
        mesh.hide(&hidden);
        let shown = colours_of(&mesh);
        assert_eq!(shown.len() * 3, mesh.visible_indices().len());
        let want: Vec<u32> = (0..mesh.triangle_count()).filter(|t| t % 3 != 0).map(|t| packed(ids[t])).collect();
        assert_eq!(shown, want);
    }
}
