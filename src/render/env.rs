//! The model's surroundings: a cube map of the default sky, the background
//! gradient or the loaded HDRI, blurred level by level for reflections,
//! plus the HDRI's picture for the backdrop (and the sharpest reflections,
//! which the surface shader reads from it directly) and its ambient light
//! as spherical harmonics.

use bytemuck::{Pod, Zeroable};

use super::Gpu;
use crate::look::HdriImage;

/// Levels of the blurred result: one per step of roughness the shader looks
/// up, the largest for a mirror. (The plain source has every level down to
/// one texel: `source_levels`.)
const ENV_LEVELS: u32 = 7;
const FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba16Float;
/// Larger pictures are shrunk to this width: sharper gains nothing on screen.
const MAX_HDRI_WIDTH: u32 = 4096;

/// What the cube is drawn from; mirrors `EnvU` in env.wgsl. A change in any
/// of it redraws the cube.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable, PartialEq)]
pub struct EnvParams {
    /// x source (0 sky, 1 gradient, 2 HDRI), y HDRI turn in radians,
    /// z HDRI exposure, w gradient bias
    pub params: [f32; 4],
    pub bottom: [f32; 4],
    pub top: [f32; 4],
    pub sun_dir: [f32; 4],
    pub sun_color: [f32; 4],
}

pub struct Env {
    uniform: wgpu::Buffer,
    layout: wgpu::BindGroupLayout,
    pipe_source: wgpu::RenderPipeline,
    pipe_shrink: wgpu::RenderPipeline,
    pipe_blur: wgpu::RenderPipeline,
    source: wgpu::Texture,
    blurred: wgpu::Texture,
    /// The side of the cube's largest level (render/quality.rs).
    size: u32,
    /// The blurred cube, for the surface shader.
    pub view: wgpu::TextureView,
    pub sampler: wgpu::Sampler,
    /// The HDRI itself (one black texel when none is loaded).
    pub hdri_view: wgpu::TextureView,
    pub hdri_sampler: wgpu::Sampler,
    /// A small copy of the HDRI for working out its ambient light.
    hdri_small: Option<(usize, usize, Vec<f32>)>,
    baked: Option<EnvParams>,
    /// Counts HDRI and cube changes, so the renderer knows to rebind.
    pub generation: u32,
}

/// How many levels a cube of `size` has down to one texel.
fn source_levels(size: u32) -> u32 {
    32 - size.max(1).leading_zeros()
}

/// The plain source cube and the blurred one, at `size`, and the view of
/// the blurred one the surface shader reads.
fn cubes(device: &wgpu::Device, size: u32) -> (wgpu::Texture, wgpu::Texture, wgpu::TextureView) {
    let cube = |label, levels| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d { width: size, height: size, depth_or_array_layers: 6 },
            mip_level_count: levels,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        })
    };
    let source = cube("environment source", source_levels(size));
    let blurred = cube("environment", ENV_LEVELS);
    let view = blurred.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::Cube),
        ..Default::default()
    });
    (source, blurred, view)
}

pub(super) fn f32_to_f16(v: f32) -> u16 {
    let bits = v.to_bits();
    let sign = ((bits >> 16) & 0x8000) as u16;
    let exp = ((bits >> 23) & 0xff) as i32 - 127 + 15;
    let frac = bits & 0x007f_ffff;
    if exp <= 0 {
        if exp < -10 {
            return sign;
        }
        let m = (frac | 0x0080_0000) >> (1 - exp);
        return sign | ((m + 0x1000) >> 13) as u16;
    }
    if exp >= 31 {
        // Too large for a half float: its largest finite value.
        return sign | 0x7bff;
    }
    let half = sign as u32 | ((exp as u32) << 10) | (frac >> 13);
    // Round to nearest; a carry out of the fraction bumps the exponent, as it should.
    (half + ((frac >> 12) & 1)) as u16
}

/// Halves a picture, averaging each 2x2 block.
fn halve(width: usize, height: usize, src: &[f32]) -> (usize, usize, Vec<f32>) {
    let (w, h) = ((width / 2).max(1), (height / 2).max(1));
    let mut out = vec![0.0f32; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let (x0, y0) = ((x * 2).min(width - 1), (y * 2).min(height - 1));
            let (x1, y1) = ((x0 + 1).min(width - 1), (y0 + 1).min(height - 1));
            for c in 0..4 {
                let at = |x: usize, y: usize| src[(y * width + x) * 4 + c];
                out[(y * w + x) * 4 + c] = (at(x0, y0) + at(x1, y0) + at(x0, y1) + at(x1, y1)) * 0.25;
            }
        }
    }
    (w, h, out)
}

fn hdri_texture(gpu: &Gpu, levels: &[(usize, usize, Vec<f32>)]) -> wgpu::TextureView {
    let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("hdri"),
        size: wgpu::Extent3d { width: levels[0].0 as u32, height: levels[0].1 as u32, depth_or_array_layers: 1 },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: FORMAT,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (level, (w, h, pixels)) in levels.iter().enumerate() {
        let half: Vec<u16> = pixels.iter().map(|&v| f32_to_f16(v)).collect();
        gpu.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&half),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(8 * *w as u32),
                rows_per_image: Some(*h as u32),
            },
            wgpu::Extent3d { width: *w as u32, height: *h as u32, depth_or_array_layers: 1 },
        );
    }
    texture.create_view(&wgpu::TextureViewDescriptor::default())
}

impl Env {
    pub fn new(gpu: &Gpu) -> Env {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("environment"),
            source: wgpu::ShaderSource::Wgsl(include_str!("env.wgsl").into()),
        });
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("environment"),
            size: std::mem::size_of::<EnvParams>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let texture_entry = |binding, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension,
                multisampled: false,
            },
            count: None,
        };
        let sampler_entry = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("environment"),
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
                texture_entry(1, wgpu::TextureViewDimension::D2),
                sampler_entry(2),
                texture_entry(3, wgpu::TextureViewDimension::Cube),
                sampler_entry(4),
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("environment"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = |entry: &'static str| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(entry),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_face"),
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
        let size = crate::render::quality::Quality::default().reflection_size();
        let (source, blurred, view) = cubes(device, size);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("environment"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        // Wraps around the horizon, never over the poles.
        let hdri_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("hdri"),
            address_mode_u: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        Env {
            uniform,
            layout,
            pipe_source: pipeline("fs_source"),
            pipe_shrink: pipeline("fs_shrink"),
            pipe_blur: pipeline("fs_blur"),
            source,
            blurred,
            size,
            view,
            sampler,
            hdri_view: hdri_texture(gpu, &[(1, 1, vec![0.0, 0.0, 0.0, 1.0])]),
            hdri_sampler,
            hdri_small: None,
            baked: None,
            generation: 0,
        }
    }

    /// Makes the cubes afresh at `size` if they are not that already; they
    /// are drawn again on the next `update`.
    pub fn set_size(&mut self, gpu: &Gpu, size: u32) {
        if size == self.size {
            return;
        }
        (self.source, self.blurred, self.view) = cubes(&gpu.device, size);
        self.size = size;
        self.baked = None;
        self.generation += 1;
    }

    /// Takes a new HDRI picture, or drops the current one.
    pub fn set_hdri(&mut self, gpu: &Gpu, image: Option<HdriImage>) {
        self.baked = None;
        self.generation += 1;
        let Some(image) = image else {
            self.hdri_view = hdri_texture(gpu, &[(1, 1, vec![0.0, 0.0, 0.0, 1.0])]);
            self.hdri_small = None;
            return;
        };
        let mut level = (image.width as usize, image.height as usize, image.pixels);
        let max = MAX_HDRI_WIDTH.min(gpu.device.limits().max_texture_dimension_2d) as usize;
        while level.0 > max {
            level = halve(level.0, level.1, &level.2);
        }
        let mut levels = vec![level];
        while levels.last().is_some_and(|l| l.0 > 1 || l.1 > 1) {
            let l = levels.last().unwrap();
            levels.push(halve(l.0, l.1, &l.2));
        }
        self.hdri_view = hdri_texture(gpu, &levels);
        // Ambient light is so smooth that a small level holds all of it.
        let small = levels.iter().position(|l| l.0 <= 128).unwrap_or(levels.len() - 1);
        self.hdri_small = Some(levels.swap_remove(small));
    }

    /// The HDRI's ambient light: nine terms for the shader's `ambient`,
    /// already folded with the cosine lobe. `turn` is the picture's yaw in
    /// radians and `scale` its exposure times the ambient strength.
    pub fn ambient(&self, turn: f32, scale: f32) -> [[f32; 4]; 9] {
        let mut sh = [[0.0f64; 3]; 9];
        let Some((w, h, pixels)) = &self.hdri_small else { return [[0.0; 4]; 9] };
        let (sin_t, cos_t) = (turn as f64).sin_cos();
        let mut total = 0.0f64;
        for y in 0..*h {
            // Latitude runs from +90 degrees at the top row to -90 at the bottom.
            let lat = (0.5 - (y as f64 + 0.5) / *h as f64) * std::f64::consts::PI;
            let (sin_lat, cos_lat) = lat.sin_cos();
            for x in 0..*w {
                let phi = ((x as f64 + 0.5) / *w as f64 - 0.5) * std::f64::consts::TAU;
                // The picture's own direction, turned back into the world.
                let d = [phi.cos() * cos_lat, sin_lat, phi.sin() * cos_lat];
                let n = [cos_t * d[0] + sin_t * d[2], d[1], -sin_t * d[0] + cos_t * d[2]];
                let basis = [
                    0.282095,
                    0.488603 * n[1],
                    0.488603 * n[2],
                    0.488603 * n[0],
                    1.092548 * n[0] * n[1],
                    1.092548 * n[1] * n[2],
                    0.315392 * (3.0 * n[2] * n[2] - 1.0),
                    1.092548 * n[0] * n[2],
                    0.546274 * (n[0] * n[0] - n[1] * n[1]),
                ];
                let px = &pixels[(y * w + x) * 4..];
                for (term, b) in sh.iter_mut().zip(basis) {
                    for c in 0..3 {
                        term[c] += px[c] as f64 * b * cos_lat;
                    }
                }
                total += cos_lat;
            }
        }
        // Each texel's share of the sphere, then the basis constants again
        // (the shader evaluates bare polynomials) and the cosine lobe's
        // weight per band, over pi.
        let texel = 4.0 * std::f64::consts::PI / total.max(1e-9);
        let fold = [
            0.282095,
            0.488603 * 2.0 / 3.0,
            0.488603 * 2.0 / 3.0,
            0.488603 * 2.0 / 3.0,
            1.092548 * 0.25,
            1.092548 * 0.25,
            0.315392 * 0.25,
            1.092548 * 0.25,
            0.546274 * 0.25,
        ];
        let mut out = [[0.0f32; 4]; 9];
        for i in 0..9 {
            for c in 0..3 {
                out[i][c] = (sh[i][c] * texel * fold[i] * scale as f64) as f32;
            }
        }
        out
    }

    /// Redraws the cube if what it is drawn from changed.
    pub fn update(&mut self, gpu: &Gpu, encoder: &mut wgpu::CommandEncoder, params: &EnvParams) {
        if self.baked.as_ref() == Some(params) {
            return;
        }
        self.baked = Some(*params);
        gpu.queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(params));

        let face = |texture: &wgpu::Texture, level: u32, face: u32| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                base_mip_level: level,
                mip_level_count: Some(1),
                base_array_layer: face,
                array_layer_count: Some(1),
                ..Default::default()
            })
        };
        let cube = |texture: &wgpu::Texture, level: Option<u32>| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::Cube),
                base_mip_level: level.unwrap_or(0),
                mip_level_count: level.map(|_| 1),
                ..Default::default()
            })
        };
        let bind = |from: &wgpu::TextureView| {
            gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("environment"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.uniform.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::TextureView(&self.hdri_view) },
                    wgpu::BindGroupEntry { binding: 2, resource: wgpu::BindingResource::Sampler(&self.hdri_sampler) },
                    wgpu::BindGroupEntry { binding: 3, resource: wgpu::BindingResource::TextureView(from) },
                    wgpu::BindGroupEntry { binding: 4, resource: wgpu::BindingResource::Sampler(&self.sampler) },
                ],
            })
        };
        let mut draw = |pipeline: &wgpu::RenderPipeline, group: &wgpu::BindGroup, to: &wgpu::Texture, level: u32| {
            for f in 0..6 {
                let view = face(to, level, f);
                let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("environment"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                });
                pass.set_pipeline(pipeline);
                pass.set_bind_group(0, group, &[]);
                let instance = level * 6 + f;
                pass.draw(0..3, instance..instance + 1);
            }
        };
        // The source, then each smaller level of it from the one above. (The
        // first pass reads no cube, but one must be bound: the other one.)
        draw(&self.pipe_source, &bind(&self.view), &self.source, 0);
        for level in 1..source_levels(self.size) {
            draw(&self.pipe_shrink, &bind(&cube(&self.source, Some(level - 1))), &self.source, level);
        }
        let all = bind(&cube(&self.source, None));
        for level in 0..ENV_LEVELS {
            draw(&self.pipe_blur, &all, &self.blurred, level);
        }
    }
}

#[cfg(test)]
mod shader_tests {
    #[test]
    fn the_environment_shader_compiles() {
        crate::render::paint::validate(include_str!("env.wgsl")).unwrap();
    }
}
