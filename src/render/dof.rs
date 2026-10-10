//! Depth of field: blurs what is nearer or farther than the focus distance,
//! as a camera lens does. The steps are in dof.wgsl.
//!
//! Nothing here is built until the effect is first switched on, and its
//! working textures are dropped again when it is switched off, so leaving it
//! off costs nothing. Switched on it is four small passes, the wide one at
//! half size.

use glam::{Mat4, Vec3};

use super::{target_texture, Globals, Gpu};
use crate::look::Look;

/// Linear colour with room above white, like the scene it blurs; the half
/// size steps also keep a signed blur radius in alpha.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Taps in the disc each pixel gathers: rings of 7, 14 and 21. Must match
/// `TAPS` in dof.wgsl.
const TAPS: usize = 42;

// The Unity app only ever set the focus distance and the aperture, so the
// rest of the lens is its render pipeline's defaults.
/// The lens: 50 mm.
const FOCAL_LENGTH_MM: f32 = 50.0;
/// The widest blur, as a radius in pixels of the full-size picture (and
/// never more than a twentieth of its height).
const MAX_RADIUS_PX: f32 = 14.0;
const MAX_RADIUS: f32 = 0.05;
/// Distances there for the same picture here: shapes here are built at half
/// the Unity app's size (the ambient occlusion reach is halved for the same
/// reason), so its focus distance of 4 is 2 of our units.
const UNITY_SCALE: f32 = 2.0;

struct Targets {
    size: (u32, u32),
    /// Half size: the scene and its blur radius, then the smoothed gather.
    half_a: wgpu::TextureView,
    /// Half size: the gather.
    half_b: wgpu::TextureView,
    /// Full size: the scene with the blur in place.
    focused: wgpu::TextureView,
    bind_prefilter: wgpu::BindGroup,
    bind_blur: wgpu::BindGroup,
    bind_smooth: wgpu::BindGroup,
    bind_composite: wgpu::BindGroup,
}

pub struct Dof {
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipe_prefilter: wgpu::RenderPipeline,
    pipe_blur: wgpu::RenderPipeline,
    pipe_smooth: wgpu::RenderPipeline,
    pipe_composite: wgpu::RenderPipeline,
    targets: Option<Targets>,
}

impl Dof {
    pub fn new(gpu: &Gpu) -> Dof {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("depth of field"),
            source: wgpu::ShaderSource::Wgsl(include_str!("dof.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth of field"),
            size: ((3 + TAPS) * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry = |binding, sample_type, multisampled| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension: wgpu::TextureViewDimension::D2, multisampled },
            count: None,
        };
        let smooth = wgpu::TextureSampleType::Float { filterable: true };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("depth of field"),
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
                texture_entry(1, smooth, false),
                texture_entry(2, smooth, false),
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                // The scene's own multisampled depth buffer, read as it is:
                // no extra pass over the model to get depth.
                texture_entry(4, wgpu::TextureSampleType::Depth, true),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("depth of field"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_dof"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: Default::default(),
                depth_stencil: None,
                multisample: Default::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(FORMAT.into())],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        // Clamped at the edges (the default), so the blur never wraps round
        // from the other side of the picture.
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("depth of field"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });
        Dof {
            pipe_prefilter: pipeline("fs_dof_prefilter"),
            pipe_blur: pipeline("fs_dof_blur"),
            pipe_smooth: pipeline("fs_dof_smooth"),
            pipe_composite: pipeline("fs_dof_composite"),
            uniform,
            layout,
            sampler,
            targets: None,
        }
    }

    /// Makes the working textures for a viewport of `size`, if they are not
    /// there already. `scene` is the shaded picture and `depth` the depth
    /// buffer it was drawn with. Returns true if they were made just now.
    pub fn resize(&mut self, gpu: &Gpu, size: (u32, u32), scene: &wgpu::TextureView, depth: &wgpu::TextureView) -> bool {
        if self.targets.as_ref().is_some_and(|t| t.size == size) {
            return false;
        }
        let device = &gpu.device;
        let half = ((size.0 / 2).max(1), (size.1 / 2).max(1));
        let half_a = target_texture(device, "focus half", half, FORMAT, 1, true);
        let half_b = target_texture(device, "focus gather", half, FORMAT, 1, true);
        let focused = target_texture(device, "scene focused", size, FORMAT, 1, true);
        let bind = |a: &wgpu::TextureView, b: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("depth of field"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(a) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(b) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::TextureView(depth) },
                ],
            })
        };
        self.targets = Some(Targets {
            size,
            bind_prefilter: bind(scene, scene),
            bind_blur: bind(&half_a, &half_a),
            bind_smooth: bind(&half_b, &half_b),
            bind_composite: bind(scene, &half_a),
            half_a,
            half_b,
            focused,
        });
        true
    }

    /// Drops the working textures (the effect was switched off, or the
    /// viewport they were made for is gone).
    pub fn release(&mut self) {
        self.targets = None;
    }

    /// The scene with the blur in place, once `draw` has run.
    pub fn focused(&self) -> Option<&wgpu::TextureView> {
        self.targets.as_ref().map(|t| &t.focused)
    }

    /// Sets the lens for this frame. `eye_offset` is how far behind its
    /// perspective position the camera is parked (orthographic views).
    pub fn set_params(&self, gpu: &Gpu, look: &Look, globals: &Globals, eye_offset: f32) {
        let Some(t) = &self.targets else { return };
        let (width, height) = (t.size.0 as f32, t.size.1 as f32);
        let focus = look.focus_distance.clamp(0.1, 10.0);
        let aperture = look.aperture.clamp(1.0, 32.0);
        // The thin-lens blur circle, with the pipeline's own mix of units
        // kept (focal length in metres, lens opening in millimetres): that
        // mix is what sets how much a given aperture blurs there.
        let focal_length = FOCAL_LENGTH_MM / 1000.0;
        let opening = FOCAL_LENGTH_MM / aperture;
        let blur = opening * focal_length / (focus - focal_length);
        let max_radius = (MAX_RADIUS_PX / height).min(MAX_RADIUS);
        // The gather runs on the half-size picture.
        let half_height = (t.size.1 / 2).max(1) as f32;

        let mut data = vec![
            [width, height, 1.0 / width, 1.0 / height],
            depth_terms(globals),
            [focus / UNITY_SCALE, blur, max_radius, eye_offset],
        ];
        data.extend(disc(max_radius * half_height, half_height));
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&data));
    }

    /// Blurs the shaded scene into `focused`.
    pub fn draw(&self, encoder: &mut wgpu::CommandEncoder) {
        let Some(t) = &self.targets else { return };
        let mut pass = |to: &wgpu::TextureView, pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup| {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("depth of field"),
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
            pass.draw(0..3, 0..1);
        };
        pass(&t.half_a, &self.pipe_prefilter, &t.bind_prefilter);
        pass(&t.half_b, &self.pipe_blur, &t.bind_blur);
        // Back into the first half-size texture, whose own job is done.
        pass(&t.half_a, &self.pipe_smooth, &t.bind_smooth);
        pass(&t.focused, &self.pipe_composite, &t.bind_composite);
    }
}

/// What turns a depth-buffer value `d` back into distance from the eye, read
/// off the projection itself so it is right for both the perspective and the
/// orthographic view: at distance `t` along the view the depth buffer holds
/// (a t + b) / (c t + e), so the distance is (b - d e) / (d c - a).
fn depth_terms(globals: &Globals) -> [f32; 4] {
    let view_proj = Mat4::from_cols_array_2d(&globals.view_proj);
    let eye = Vec3::from_slice(&globals.cam_pos);
    let forward = Vec3::from_slice(&globals.cam_forward);
    let at = |t: f32| view_proj * (eye + forward * t).extend(1.0);
    let (p, q) = (at(1.0), at(2.0));
    let (a, c) = (q.z - p.z, q.w - p.w);
    [a, p.z - a, c, p.w - c]
}

/// The disc each pixel gathers, as the Unity pipeline lays it out: the
/// centre (which the shader adds itself) and three rings around it. Its lens
/// has five blades, but fully rounded by default, which makes the disc a
/// plain circle. `max_radius` is in half-size pixels, of which `height` make
/// the picture's height. Each tap is its offset in whole half-size pixels
/// (the gather reads pixels, not blends of them) and how far out that is as
/// a fraction of the height.
fn disc(max_radius: f32, height: f32) -> Vec<[f32; 4]> {
    const RINGS: usize = 3;
    const FIRST_RING: usize = 7;
    let mut taps = Vec::with_capacity(TAPS);
    for ring in 1..=RINGS {
        // Not quite evenly spaced: the inner ring sits a little further out.
        let bias = 1.0 / FIRST_RING as f32;
        let radius = (ring as f32 + bias) / (RINGS as f32 + bias) * max_radius;
        let points = ring * FIRST_RING;
        for point in 0..points {
            let (sin, cos) = (std::f32::consts::TAU * point as f32 / points as f32).sin_cos();
            let (x, y) = ((radius * cos).round(), (radius * sin).round());
            taps.push([x, y, x.hypot(y) / height, 0.0]);
        }
    }
    debug_assert_eq!(taps.len(), TAPS);
    taps
}

#[cfg(test)]
mod tests {
    use bytemuck::Zeroable;
    use glam::{Vec2, Vec4};

    use super::*;
    use crate::camera::Camera;

    /// The distance `depth_terms` reads back from what the projection puts
    /// in the depth buffer for a point `away` straight ahead of the camera.
    fn read_back(camera: &Camera, away: f32) -> f32 {
        let view = camera.view(Vec2::ZERO, Vec2::new(1600.0, 960.0), 2.0, 1.0);
        let mut globals = Globals::zeroed();
        globals.view_proj = view.view_proj.to_cols_array_2d();
        globals.cam_pos = view.pos.extend(1.0).to_array();
        globals.cam_forward = view.forward.extend(0.0).to_array();
        let [a, b, c, e] = depth_terms(&globals);
        let clip: Vec4 = view.view_proj * (view.pos + view.forward * away).extend(1.0);
        let d = clip.z / clip.w;
        (b - d * e) / (d * c - a)
    }

    #[test]
    fn depth_buffer_reads_back_as_distance() {
        let mut camera = Camera::default();
        (camera.yaw, camera.pitch) = (30.0, 20.0);
        for ortho in [false, true] {
            camera.ortho = ortho;
            for away in [0.5, 2.0, 7.5, 40.0] {
                let got = read_back(&camera, away);
                assert!((got - away).abs() < away * 1e-3, "ortho {ortho}: {away} read back as {got}");
            }
        }
    }

    #[test]
    fn disc_is_the_pipelines_42_taps_within_its_radius() {
        let taps = disc(7.0, 480.0);
        assert_eq!(taps.len(), TAPS);
        for [x, y, away, _] in taps {
            // Whole pixels, no further out than the radius (give or take
            // the rounding), and `away` is that distance over the height.
            assert_eq!((x.fract(), y.fract()), (0.0, 0.0));
            assert!(x.hypot(y) <= 7.0 + 0.75);
            assert!((away * 480.0 - x.hypot(y)).abs() < 1e-4);
        }
    }
}
