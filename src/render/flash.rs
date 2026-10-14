//! The selection flash, ported from the Unity app's `SelectionFlashEffect`: an
//! object pulses blue for 0.35 s when it is picked, framed or selected, and
//! white for 0.1 s when an undo or redo changes it, so the user can see what
//! just happened. The colour starts at its full alpha and fades linearly to
//! nothing.
//!
//! Two halves. [`State`] is the clock: the app starts a pulse on an object,
//! and each frame asks which are still running and how strong they are. It
//! needs no GPU. [`Layer`] is the drawing: it redraws the object's own
//! triangles, from the buffers the scene pass already uses, in one flat
//! translucent colour at the end of the overlay pass. Nothing is uploaded,
//! drawn or allocated per frame while no pulse is running.
//!
//! Unity adds the flash as an extra material in the scene's linear light; here
//! it goes on the finished, encoded picture, so the blend is done on display
//! values. The colours are the same ones, and the difference is a few percent
//! of brightness.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::cache::Stamp;
use super::pipelines::Cull;
use super::{DEPTH_FORMAT, Gpu, MSAA_SAMPLES, MeshBuffers};

/// What the pulse announces; sets its colour and length.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// An object was picked, framed or selected: blue, 0.35 s.
    Select,
    /// An undo or redo changed it: white, 0.1 s. Shorter and neutral because
    /// it only confirms "the surface changed", where the blue one says "you
    /// are now working on something else".
    Undo,
}

impl Kind {
    fn seconds(self) -> f32 {
        match self {
            Kind::Select => 0.35,
            Kind::Undo => 0.1,
        }
    }

    /// Colour as shown on screen (not linear), with the alpha it starts at.
    fn color(self) -> [f32; 4] {
        match self {
            Kind::Select => [0.3, 0.7, 1.0, 0.6],
            Kind::Undo => [1.0, 1.0, 1.0, 0.5],
        }
    }
}

struct Pulse {
    /// A scene object id (not a mesh id: instances share meshes and only the
    /// one that was clicked pulses).
    object: u64,
    kind: Kind,
    /// Seconds since it was first drawn; `None` until then. The clock does
    /// not start before the first frame that shows the pulse, so a slow frame
    /// (a big mesh being re-sent after an undo) cannot use the pulse up
    /// before it has been seen once.
    age: Option<f32>,
    /// Stays at this age for good (for the `--demo-flash` screenshots).
    held: bool,
}

/// The pulses running now. A new one on an object replaces its old one.
#[derive(Default)]
pub struct State {
    pulses: Vec<Pulse>,
}

impl State {
    pub fn start(&mut self, object: u64, kind: Kind) {
        self.pulses.retain(|p| p.object != object);
        self.pulses.push(Pulse { object, kind, age: None, held: false });
    }

    /// Drops everything started while the app was setting itself up, then, if
    /// `demo` names an object, holds a blue pulse on it at half strength: a
    /// still picture of the effect for visual checks.
    pub fn startup(&mut self, demo: Option<u64>) {
        self.pulses.clear();
        if let Some(object) = demo {
            let kind = Kind::Select;
            self.pulses.push(Pulse { object, kind, age: Some(kind.seconds() * 0.5), held: true });
        }
    }

    /// Moves the clocks on by `dt` seconds, forgets the pulses that have run
    /// out, and returns the rest as (object, colour with its alpha for this
    /// instant). With `show` false nothing is returned but time still passes.
    pub fn frame(&mut self, dt: f32, show: bool) -> Vec<(u64, [f32; 4])> {
        for p in &mut self.pulses {
            if !p.held {
                p.age = Some(p.age.map_or(0.0, |a| a + dt));
            }
        }
        self.pulses.retain(|p| p.age.is_some_and(|a| a < p.kind.seconds()));
        if !show {
            return Vec::new();
        }
        self.pulses
            .iter()
            .map(|p| {
                let t = p.age.unwrap_or(0.0) / p.kind.seconds();
                let mut color = p.kind.color();
                color[3] *= 1.0 - t;
                (p.object, color)
            })
            .collect()
    }

    /// Whether another frame is needed to carry a pulse on. Checked after
    /// [`Self::frame`], so the frame that sees the last pulse run out (and is
    /// drawn without it) is the last one.
    pub fn animating(&self) -> bool {
        self.pulses.iter().any(|p| !p.held)
    }
}

/// One object to pulse this frame.
pub struct Item {
    /// Key of the object's GPU buffers (`Object::mesh`).
    pub mesh: u64,
    /// The object's matrix by columns, as in `DrawItem`.
    pub model: [[f32; 4]; 4],
    /// Display colour; alpha is the strength right now.
    pub color: [f32; 4],
}

/// Most pulses drawn at once: an undo a moment after a pick makes two, so
/// this is far more than ever happens.
const MAX_PULSES: usize = 16;

/// Per-instance vertex data; mirrors `In` in flash.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Instance {
    model: [[f32; 4]; 4],
    color: [f32; 4],
}

pub struct Layer {
    /// One per way of culling (`Cull::index`), as the main pass culls.
    pipelines: [wgpu::RenderPipeline; 3],
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instances: wgpu::Buffer,
    /// The mesh of each instance written this frame; empty means no flash.
    meshes: Vec<u64>,
    /// Whether each instance's placement mirrors, which turns its culling round.
    mirrored: Vec<bool>,
    /// Moves when what is drawn changes (see cache.rs).
    pub(super) stamp: Stamp,
}

impl Layer {
    pub fn new(gpu: &Gpu) -> Layer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("flash"),
            source: wgpu::ShaderSource::Wgsl(include_str!("flash.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flash uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let instances = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("flash instances"),
            size: (MAX_PULSES * std::mem::size_of::<Instance>()) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("flash"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("flash"),
            layout: &bind_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("flash"),
            bind_group_layouts: &[Some(&bind_layout)],
            immediate_size: 0,
        });
        let vertex_buffers = [
            // The mesh's own position buffer, as the main pass reads it.
            Some(wgpu::VertexBufferLayout {
                array_stride: 12,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &wgpu::vertex_attr_array![0 => Float32x3],
            }),
            Some(wgpu::VertexBufferLayout {
                array_stride: std::mem::size_of::<Instance>() as u64,
                step_mode: wgpu::VertexStepMode::Instance,
                attributes: &wgpu::vertex_attr_array![
                    1 => Float32x4, 2 => Float32x4, 3 => Float32x4, 4 => Float32x4, 5 => Float32x4
                ],
            }),
        ];
        let pipelines = Cull::ALL.map(|cull| device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("flash"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs_flash"),
                compilation_options: Default::default(),
                buffers: &vertex_buffers,
            },
            // The faces the main pass draws: the object's own surface is the
            // only thing at the scene's depth, so with both faces drawn its
            // back faces lose the test unless the mesh is open there, and
            // then the flash covers what is visible through the hole too.
            primitive: wgpu::PrimitiveState { cull_mode: cull.face(), ..Default::default() },
            // Reverse depth, as everywhere. Tested against the scene's depth
            // but never written, with a nudge toward the eye (the shader
            // shares the scene's arithmetic, so this only has to cover the
            // last-bit differences a driver may leave between two shaders;
            // Unity's `Offset -1, -1`).
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
                entry_point: Some("fs_flash"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: gpu.format,
                    blend: Some(wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            multiview_mask: None,
            cache: None,
        }));
        Layer { pipelines, uniforms, bind, instances, meshes: Vec::new(), mirrored: Vec::new(), stamp: Stamp::default() }
    }

    /// Sets what is drawn this frame: `items` is empty while nothing pulses,
    /// and then nothing is sent to the GPU. `view_proj` is the scene's.
    pub fn show(&mut self, gpu: &Gpu, view_proj: &[[f32; 4]; 4], items: &[Item]) {
        self.meshes.clear();
        if items.is_empty() {
            self.stamp.note(&[]);
            return;
        }
        let items = &items[..items.len().clamp(0, MAX_PULSES)];
        let data: Vec<Instance> = items.iter().map(|i| Instance { model: i.model, color: i.color }).collect();
        gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::bytes_of(view_proj));
        gpu.queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&data));
        self.meshes.extend(items.iter().map(|i| i.mesh));
        self.mirrored = items.iter().map(|i| super::pipelines::mirrored(&i.model)).collect();
        self.stamp.note(&[bytemuck::bytes_of(view_proj), bytemuck::cast_slice(&data), bytemuck::cast_slice(&self.meshes)]);
    }

    /// Whether a pulse is drawn this frame.
    pub(super) fn active(&self) -> bool {
        !self.meshes.is_empty()
    }

    /// Draws into the overlay pass, after the scene and its depth.
    /// `show_backfaces` as the main pass had it.
    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass, buffers: &HashMap<u64, MeshBuffers>, show_backfaces: bool) {
        if self.meshes.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(1, self.instances.slice(..));
        for (i, id) in self.meshes.iter().enumerate() {
            let Some(mesh) = buffers.get(id) else { continue };
            let cull = Cull::of(show_backfaces, self.mirrored.get(i).copied().unwrap_or(false));
            pass.set_pipeline(&self.pipelines[cull.index()]);
            pass.set_vertex_buffer(0, mesh.positions.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.index_count, 0, i as u32..i as u32 + 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BLUE: [f32; 4] = [0.3, 0.7, 1.0, 0.6];

    #[test]
    fn blue_pulse_starts_at_full_alpha_and_fades_out_in_0_35_s() {
        let mut state = State::default();
        state.start(7, Kind::Select);
        // The first frame shows it at full strength, however long that frame took.
        assert_eq!(state.frame(0.1, true), vec![(7, BLUE)]);
        let half = state.frame(0.175, true);
        assert_eq!(half.len(), 1);
        assert_eq!(half[0].1[..3], BLUE[..3]);
        assert!((half[0].1[3] - 0.3).abs() < 1e-5, "alpha {}", half[0].1[3]);
        assert!(state.animating());
        // The frame that sees it run out is drawn without it and ends the animation.
        assert!(state.frame(0.2, true).is_empty());
        assert!(!state.animating());
    }

    #[test]
    fn undo_pulse_is_white_and_lasts_0_1_s() {
        let mut state = State::default();
        state.start(3, Kind::Undo);
        assert_eq!(state.frame(0.0, true), vec![(3, [1.0, 1.0, 1.0, 0.5])]);
        assert!((state.frame(0.05, true)[0].1[3] - 0.25).abs() < 1e-5);
        assert!(state.frame(0.06, true).is_empty());
    }

    #[test]
    fn a_new_pulse_replaces_the_old_one_on_the_same_object_only() {
        let mut state = State::default();
        state.start(1, Kind::Select);
        state.start(2, Kind::Select);
        state.frame(0.0, true);
        state.frame(0.2, true);
        state.start(1, Kind::Undo);
        let now = state.frame(0.01, true);
        assert_eq!(now.len(), 2);
        let one = now.iter().find(|(id, _)| *id == 1).unwrap().1;
        let two = now.iter().find(|(id, _)| *id == 2).unwrap().1;
        assert_eq!(one[..3], [1.0, 1.0, 1.0]);
        assert_eq!(two[..3], BLUE[..3]);
    }

    #[test]
    fn hidden_frames_return_nothing_but_time_still_passes() {
        let mut state = State::default();
        state.start(1, Kind::Select);
        assert!(state.frame(0.0, false).is_empty());
        assert!(state.animating());
        assert!(state.frame(0.5, false).is_empty());
        assert!(!state.animating());
    }

    #[test]
    fn startup_clears_what_setup_started_and_can_hold_a_demo_pulse() {
        let mut state = State::default();
        state.start(1, Kind::Select);
        state.startup(None);
        assert!(state.frame(0.0, true).is_empty());
        state.startup(Some(5));
        // Half strength, for as many frames as it is asked for, and no more frames wanted.
        for _ in 0..3 {
            let held = state.frame(1.0, true);
            assert_eq!(held.len(), 1);
            assert!((held[0].1[3] - 0.3).abs() < 1e-5);
        }
        assert!(!state.animating());
    }
}
