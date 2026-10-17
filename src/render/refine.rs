//! Refining a still picture. While anything changes, every frame is drawn
//! once, as it always was. The moment the scene key (cache.rs) holds still,
//! each further frame draws another sample of the same picture instead of
//! copying the kept one: the camera moved a fraction of a pixel along a
//! Halton sequence, and every filter that works from noise (the soft
//! shadow's taps, the ambient occlusion's, depth of field's disc) turned a
//! new way. The samples are folded into a running average kept in full
//! floats, which the post effects read in place of the scene. After the
//! number of samples asked for, the kept picture is copied out again at no
//! cost. Any change to the key starts over, so a stroke or an orbit never
//! sees a sample.

use super::{target_texture, Gpu};

/// The running average: full floats, so the last of hundreds of samples
/// still moves it.
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba32Float;

/// The point of the Halton sequence of `base` at `index`: 0..1, each new
/// point filling the biggest gap the earlier ones left.
pub fn halton(mut index: u32, base: u32) -> f32 {
    let (mut result, mut fraction) = (0.0f32, 1.0f32);
    while index > 0 {
        fraction /= base as f32;
        result += fraction * (index % base) as f32;
        index /= base;
    }
    result
}

/// How far sample `k` moves the camera, in pixels across and down
/// (-0.5..0.5 each way). The first sample is the picture as drawn while
/// things moved, at the pixel's centre.
pub fn jitter(k: u32) -> [f32; 2] {
    if k == 0 { [0.0, 0.0] } else { [halton(k, 2) - 0.5, halton(k, 3) - 0.5] }
}

/// `view_proj` with the picture moved `pixels` across and down, for a
/// picture `size` pixels big.
pub fn jittered(view_proj: [[f32; 4]; 4], pixels: [f32; 2], size: (u32, u32)) -> [[f32; 4]; 4] {
    // In clip space a pixel is 2 / size across; down the screen is -y.
    let shift = glam::Vec3::new(pixels[0] * 2.0 / size.0.max(1) as f32, -pixels[1] * 2.0 / size.1.max(1) as f32, 0.0);
    (glam::Mat4::from_translation(shift) * glam::Mat4::from_cols_array_2d(&view_proj)).to_cols_array_2d()
}

/// The passes that fold samples together, shared by every picture size.
pub struct Refine {
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipe_seed: wgpu::RenderPipeline,
    pipe_fold: wgpu::RenderPipeline,
}

/// One picture size's running average, and where its samples are drawn.
pub struct Buffers {
    /// The average, twice: each fold reads one and writes the other.
    averages: [wgpu::TextureView; 2],
    /// Where the scene pass resolves a sample, instead of into the scene.
    pub sample: wgpu::TextureView,
    /// Per side of `averages`: the seed writing it from the scene, and the
    /// fold reading it and the sample.
    seed: [wgpu::BindGroup; 2],
    fold: [wgpu::BindGroup; 2],
    /// Which of `averages` holds the latest.
    front: usize,
    /// Samples in the average; 0 until refining starts, when the picture
    /// drawn last is the only one, in the scene texture.
    pub count: u32,
}

impl Buffers {
    /// Samples the picture shows now.
    pub fn shown(&self) -> u32 {
        self.count.max(1)
    }
}

impl Refine {
    pub fn new(gpu: &Gpu, scene_format: wgpu::TextureFormat) -> Refine {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("refine"),
            source: wgpu::ShaderSource::Wgsl(include_str!("refine.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("refine"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("refine"),
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
                texture_entry(1),
                texture_entry(2),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("refine"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &'static str, targets: &[Option<wgpu::ColorTargetState>]| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_refine"),
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
                    targets,
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Refine {
            pipe_seed: pipeline("fs_seed", &[Some(FORMAT.into())]),
            pipe_fold: pipeline("fs_accumulate", &[Some(FORMAT.into()), Some(scene_format.into())]),
            uniform,
            layout,
        }
    }

    /// The running average and sample texture for a picture of `size`
    /// whose scene is `scene` (of `scene_format`).
    pub fn buffers(
        &self,
        gpu: &Gpu,
        size: (u32, u32),
        scene: &wgpu::TextureView,
        scene_format: wgpu::TextureFormat,
    ) -> Buffers {
        let device = &gpu.device;
        let averages = [0, 1].map(|_| target_texture(device, "refined average", size, FORMAT, 1, true));
        let sample = target_texture(device, "refine sample", size, scene_format, 1, true);
        let bind = |a: &wgpu::TextureView, b: &wgpu::TextureView| {
            device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("refine"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(a) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::TextureView(b) },
                ],
            })
        };
        let seed = [bind(scene, &sample), bind(scene, &sample)];
        let fold = [bind(&averages[0], &sample), bind(&averages[1], &sample)];
        Buffers { averages, sample, seed, fold, front: 0, count: 0 }
    }

    fn pass(
        encoder: &mut wgpu::CommandEncoder,
        targets: &[&wgpu::TextureView],
        pipeline: &wgpu::RenderPipeline,
        group: &wgpu::BindGroup,
    ) {
        let attachments: Vec<_> = targets
            .iter()
            .map(|view| {
                Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })
            })
            .collect();
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("refine"),
            color_attachments: &attachments,
            ..Default::default()
        });
        pass.set_pipeline(pipeline);
        pass.set_bind_group(0, group, &[]);
        pass.draw(0..3, 0..1);
    }

    /// Folds the sample just drawn into `buffers`' average and writes the
    /// result into `scene` for the post effects. On the first sample the
    /// picture already in `scene` starts the average. Each call must go to
    /// the graphics card in a submission of its own (the weight is shared).
    pub fn fold(&self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, buffers: &mut Buffers, scene: &wgpu::TextureView) {
        if buffers.count == 0 {
            let front = buffers.front;
            Self::pass(encoder, &[&buffers.averages[front]], &self.pipe_seed, &buffers.seed[front]);
            buffers.count = 1;
        }
        let weight = 1.0 / (buffers.count as f32 + 1.0);
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&[weight, 0.0, 0.0, 0.0]));
        let (front, back) = (buffers.front, 1 - buffers.front);
        Self::pass(encoder, &[&buffers.averages[back], scene], &self.pipe_fold, &buffers.fold[front]);
        buffers.front = back;
        buffers.count += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_halton_points_fill_the_pixel_evenly() {
        assert_eq!(jitter(0), [0.0, 0.0], "the first sample is the picture as it was");
        assert_eq!(halton(1, 2), 0.5);
        assert_eq!(halton(2, 2), 0.25);
        assert_eq!(halton(3, 2), 0.75);
        assert!((halton(1, 3) - 1.0 / 3.0).abs() < 1e-6);
        // 64 samples: every quarter of the pixel gets a quarter of them, and
        // their average is the pixel's centre.
        let points: Vec<[f32; 2]> = (1..=64).map(jitter).collect();
        for p in &points {
            assert!(p.iter().all(|v| (-0.5..0.5).contains(v)), "{p:?}");
        }
        for quarter in [(false, false), (false, true), (true, false), (true, true)] {
            let n = points.iter().filter(|p| (p[0] >= 0.0, p[1] >= 0.0) == quarter).count();
            assert!((12..=20).contains(&n), "{quarter:?}: {n}");
        }
        let mean = points.iter().fold([0.0f32; 2], |m, p| [m[0] + p[0] / 64.0, m[1] + p[1] / 64.0]);
        assert!(mean[0].abs() < 0.03 && mean[1].abs() < 0.03, "{mean:?}");
    }

    #[test]
    fn the_folding_shader_compiles() {
        crate::render::paint::validate(include_str!("refine.wgsl")).unwrap();
    }

    #[test]
    fn a_jitter_moves_the_picture_by_that_many_pixels() {
        // A perspective projection (reverse depth, as the viewport's).
        let view_proj = glam::Mat4::from_cols(
            glam::Vec4::new(1.2, 0.0, 0.0, 0.0),
            glam::Vec4::new(0.0, 1.8, 0.0, 0.0),
            glam::Vec4::new(0.0, 0.0, 0.0, -1.0),
            glam::Vec4::new(0.0, 0.0, 0.1, 0.0),
        );
        let moved = glam::Mat4::from_cols_array_2d(&jittered(view_proj.to_cols_array_2d(), [0.5, 0.25], (300, 200)));
        let point = glam::Vec3::new(0.3, -0.2, -5.0);
        let (a, b) = (view_proj.project_point3(point), moved.project_point3(point));
        // Half a pixel right is 1/300 of clip space across; a quarter down
        // is 1/400 of it, downward.
        assert!(((b.x - a.x) - 1.0 / 300.0).abs() < 1e-6, "{a} {b}");
        assert!(((b.y - a.y) + 1.0 / 400.0).abs() < 1e-6, "{a} {b}");
        assert!((b.z - a.z).abs() < 1e-6, "depth is untouched");
    }
}
