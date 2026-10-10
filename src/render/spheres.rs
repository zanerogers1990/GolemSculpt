//! The SSphere tool's layer of the viewport: the armature, the placement
//! ghost, the mirror plane and the skin. Self-contained (its own shader,
//! uniforms and buffers) and drawn at the end of the main viewport pass, so
//! it is depth-tested against the scene.

use bytemuck::{Pod, Zeroable};
use glam::Mat4;

use super::{DEPTH_FORMAT, Gpu, MSAA_SAMPLES};
use crate::camera::View;
use crate::ssphere::skin::Skin;
use crate::ssphere::view::{Armature, Vertex};

/// How the skin is shown this frame.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum SkinLook {
    Hidden,
    /// See-through, over the armature.
    Shell,
    /// Opaque, for judging the shape.
    Solid,
}

/// Mirrors `Uniforms` in spheres.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    view_proj: [[f32; 4]; 4],
    model: [[f32; 4]; 4],
    cam_pos: [f32; 4],
    cam_right: [f32; 4],
    cam_up: [f32; 4],
    cam_forward: [f32; 4],
    params: [f32; 4],
}

/// A GPU buffer that is reused from frame to frame and only replaced when
/// the data outgrows it.
struct Growable {
    label: &'static str,
    usage: wgpu::BufferUsages,
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl Growable {
    fn new(label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Self { label, usage, buffer: None, capacity: 0 }
    }

    fn write(&mut self, gpu: &Gpu, bytes: &[u8]) {
        if bytes.is_empty() {
            return;
        }
        if self.buffer.is_none() || self.capacity < bytes.len() as u64 {
            self.capacity = (bytes.len() as u64).next_power_of_two().max(4096);
            self.buffer = Some(gpu.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.capacity,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        gpu.queue.write_buffer(self.buffer.as_ref().unwrap(), 0, bytes);
    }
}

pub struct Layer {
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    pipe_armature: wgpu::RenderPipeline,
    pipe_armature_blend: wgpu::RenderPipeline,
    pipe_skin_solid: wgpu::RenderPipeline,
    pipe_skin_shell: wgpu::RenderPipeline,
    arm_vertices: Growable,
    arm_indices: Growable,
    /// Index counts of the armature: the opaque part, then everything.
    arm_counts: (u32, u32),
    skin_positions: Growable,
    skin_normals: Growable,
    skin_indices: Growable,
    skin_count: u32,
    /// Which skin is in the buffers (see `SSpheres::skin`).
    skin_generation: Option<u64>,
    look: SkinLook,
    visible: bool,
}

impl Layer {
    pub fn new(gpu: &Gpu) -> Layer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("spheres"),
            source: wgpu::ShaderSource::Wgsl(include_str!("spheres.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("spheres uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("spheres"),
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
            label: Some("spheres"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("spheres"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });

        let armature_buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Vertex>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x3, 2 => Float32x4],
        })];
        let skin_buffers = [
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
        ];
        // See-through surfaces test against depth but do not write it:
        // writing would let whichever face drew first hide the others, which
        // reads as random holes as the camera orbits.
        let pipeline = |label, buffers: &[Option<wgpu::VertexBufferLayout>], vertex, fragment, see_through: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers,
                },
                primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
                // Reverse depth, as in the main pass.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(!see_through),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
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

        let vertex = wgpu::BufferUsages::VERTEX;
        let index = wgpu::BufferUsages::INDEX;
        Layer {
            pipe_armature: pipeline("armature", &armature_buffers, "vs_armature", "fs_armature", false),
            pipe_armature_blend: pipeline("armature see-through", &armature_buffers, "vs_armature", "fs_armature", true),
            pipe_skin_solid: pipeline("skin solid", &skin_buffers, "vs_skin", "fs_skin_solid", false),
            pipe_skin_shell: pipeline("skin shell", &skin_buffers, "vs_skin_shell", "fs_skin_shell", true),
            uniforms,
            bind,
            arm_vertices: Growable::new("armature vertices", vertex),
            arm_indices: Growable::new("armature indices", index),
            arm_counts: (0, 0),
            skin_positions: Growable::new("skin positions", vertex),
            skin_normals: Growable::new("skin normals", vertex),
            skin_indices: Growable::new("skin indices", index),
            skin_count: 0,
            skin_generation: None,
            look: SkinLook::Hidden,
            visible: false,
        }
    }

    /// Nothing of the tool is drawn until the next [`Self::show`].
    pub fn hide(&mut self) {
        self.visible = false;
    }

    /// Sets what the layer draws this frame. The skin is only re-sent when
    /// its generation number changes. `shell_lift` is how far (world units)
    /// the see-through skin is drawn toward the eye. The armature arrives in
    /// world space; the skin is kept in the rig's own space, and `model`
    /// places it (so a rig that follows an object costs no re-upload).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        gpu: &Gpu,
        view: &View,
        armature: &Armature,
        skin: Option<(&Skin, u64)>,
        look: SkinLook,
        shell_lift: f32,
        model: Mat4,
    ) {
        let ortho = if view.ortho_half_height.is_some() { 1.0 } else { 0.0 };
        let uniforms = Uniforms {
            view_proj: view.view_proj.to_cols_array_2d(),
            model: model.to_cols_array_2d(),
            cam_pos: view.pos.extend(ortho).to_array(),
            cam_right: view.right.extend(0.0).to_array(),
            cam_up: view.up.extend(0.0).to_array(),
            cam_forward: view.forward.extend(0.0).to_array(),
            params: [shell_lift, 0.0, 0.0, 0.0],
        };
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(&uniforms));

        self.arm_vertices.write(gpu, bytemuck::cast_slice(&armature.vertices));
        self.arm_indices.write(gpu, bytemuck::cast_slice(&armature.indices));
        self.arm_counts = (armature.opaque, armature.indices.len() as u32);

        match skin {
            Some((skin, generation)) => {
                if self.skin_generation != Some(generation) {
                    self.skin_generation = Some(generation);
                    self.skin_positions.write(gpu, bytemuck::cast_slice(&skin.positions));
                    self.skin_normals.write(gpu, bytemuck::cast_slice(&skin.normals));
                    self.skin_indices.write(gpu, bytemuck::cast_slice(&skin.indices));
                    self.skin_count = skin.indices.len() as u32;
                }
                self.look = look;
            }
            None => self.look = SkinLook::Hidden,
        }
        self.visible = true;
    }

    /// Draws into the main viewport pass, after the scene.
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        if !self.visible {
            return;
        }
        pass.set_bind_group(0, &self.bind, &[]);
        let skin = |pass: &mut wgpu::RenderPass, pipeline: &wgpu::RenderPipeline| {
            let (Some(positions), Some(normals), Some(indices)) =
                (&self.skin_positions.buffer, &self.skin_normals.buffer, &self.skin_indices.buffer)
            else {
                return;
            };
            if self.skin_count == 0 {
                return;
            }
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, positions.slice(..));
            pass.set_vertex_buffer(1, normals.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.skin_count, 0, 0..1);
        };
        let armature = |pass: &mut wgpu::RenderPass, pipeline: &wgpu::RenderPipeline, range: std::ops::Range<u32>| {
            let (Some(vertices), Some(indices)) = (&self.arm_vertices.buffer, &self.arm_indices.buffer) else {
                return;
            };
            if range.is_empty() {
                return;
            }
            pass.set_pipeline(pipeline);
            pass.set_vertex_buffer(0, vertices.slice(..));
            pass.set_index_buffer(indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(range, 0, 0..1);
        };

        // Everything solid first, then the see-through surfaces over it.
        if self.look == SkinLook::Solid {
            skin(pass, &self.pipe_skin_solid);
        }
        armature(pass, &self.pipe_armature, 0..self.arm_counts.0);
        if self.look == SkinLook::Shell {
            skin(pass, &self.pipe_skin_shell);
        }
        armature(pass, &self.pipe_armature_blend, self.arm_counts.0..self.arm_counts.1);
    }
}
