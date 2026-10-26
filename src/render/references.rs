//! The scene's reference pictures in the viewport (see src/references.rs):
//! each a flat picture with its image on it, standing where the Reference
//! Images card puts it. Self-contained (its own shader, references.wgsl, its
//! own uniforms and buffers), like the tools' previews.
//!
//! They are drawn in the overlay pass, after the scene has been shaded,
//! bloomed and graded, so a photo shows in its own colours: the lights, the
//! exposure, the tone curve and the colour grade are the model's business,
//! not the photo's. A picture behind the model is tested against the scene's
//! depth and hidden wherever the model stands in front of it; one set In
//! Front is drawn over everything, see-through by its Opacity, for tracing
//! an outline. Neither writes depth, so the wireframe and the other
//! overlays still find the model's. See-through pictures are drawn farthest
//! first, so they lie over one another as they stand, and those In Front
//! after all the rest.
//!
//! A picture shows only from the side it faces. Orbit round to the back of
//! the model and the front picture, now between the eye and the model, is
//! not drawn, rather than standing in the way.
//!
//! None of this is in a picture made to keep (`Renderer::picture`: a render,
//! a turntable loop, a timelapse), nor in a recent file's tile (`hide`):
//! reference pictures are a view aid, not part of the work.
//!
//! An image goes to the graphics card once, the first time its picture is
//! drawn, and stays there while the picture is in the scene, shown or not.
//! It goes with every half size below it (mips.rs), so a photo seen small
//! does not sparkle, and premultiplied by its own alpha, so the soft edge of
//! a cut-out PNG does not darken as the card blends between its pixels.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};
use image::RgbaImage;

use super::cache::Stamp;
use super::{DEPTH_FORMAT, Gpu, MSAA_SAMPLES, mips};
use crate::camera::View;
use crate::references::Picture;

/// One corner of a picture; mirrors `In` in references.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
struct Corner {
    position: [f32; 3],
    uv: [f32; 2],
    opacity: f32,
}

/// The corners a picture is drawn with: two triangles.
const CORNERS: u32 = 6;

/// One picture to draw this frame: its image, which of the two pipelines,
/// and where its corners start in the buffer.
struct Quad {
    id: u64,
    in_front: bool,
    first: u32,
}

pub struct Layer {
    uniforms: wgpu::Buffer,
    bind: wgpu::BindGroup,
    image_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    /// Behind the model (tested against its depth), and In Front (not).
    pipelines: [wgpu::RenderPipeline; 2],
    /// Each picture's image on the card, by picture.
    images: HashMap<u64, wgpu::BindGroup>,
    corners: Option<(wgpu::Buffer, u64)>,
    quads: Vec<Quad>,
    /// Moves when what is drawn changes (see cache.rs).
    pub(super) stamp: Stamp,
}

/// The six corners of `picture`'s two triangles, each going round
/// anticlockwise seen from the view the picture faces: the side the
/// pipelines keep.
fn corners_of(picture: &Picture) -> [Corner; 6] {
    let at = picture.corners();
    let uv = picture.place.image_corners();
    let opacity = picture.place.opacity.clamp(0.0, 1.0);
    let corner = |i: usize| Corner { position: at[i].to_array(), uv: uv[i], opacity };
    [0, 1, 2, 0, 2, 3].map(corner)
}

/// The order to draw `pictures` in, seen from `view`: those behind the model
/// first, then those In Front, each farthest first. Pictures hidden, or
/// faded to nothing, are left out.
fn drawing_order<'a>(pictures: &'a [Picture], view: &View) -> Vec<&'a Picture> {
    let far = |p: &Picture| (p.place.centre() - view.pos).dot(view.forward);
    let mut drawn: Vec<&Picture> = pictures.iter().filter(|p| p.place.shown && p.place.opacity > 0.0).collect();
    drawn.sort_by(|a, b| a.place.in_front.cmp(&b.place.in_front).then(far(b).total_cmp(&far(a))));
    drawn
}

/// `image` with each pixel's colour multiplied by its own alpha.
fn premultiplied(image: &RgbaImage) -> RgbaImage {
    let mut out = image.clone();
    for pixel in out.pixels_mut() {
        let a = pixel[3] as u32;
        if a < 255 {
            for c in &mut pixel.0[..3] {
                *c = ((*c as u32 * a + 127) / 255) as u8;
            }
        }
    }
    out
}

impl Layer {
    pub fn new(gpu: &Gpu) -> Layer {
        let device = &gpu.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("reference pictures"),
            source: wgpu::ShaderSource::Wgsl(include_str!("references.wgsl").into()),
        });
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("reference picture uniforms"),
            size: 64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uniform_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reference pictures"),
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
            label: Some("reference pictures"),
            layout: &uniform_layout,
            entries: &[wgpu::BindGroupEntry { binding: 0, resource: uniforms.as_entire_binding() }],
        });
        let image_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("reference picture image"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("reference pictures"),
            bind_group_layouts: &[Some(&uniform_layout), Some(&image_layout)],
            immediate_size: 0,
        });
        let buffers = [Some(wgpu::VertexBufferLayout {
            array_stride: std::mem::size_of::<Corner>() as u64,
            step_mode: wgpu::VertexStepMode::Vertex,
            attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Float32x2, 2 => Float32],
        })];
        let pipeline = |in_front: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(if in_front { "reference picture in front" } else { "reference picture" }),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_reference"),
                    compilation_options: Default::default(),
                    buffers: &buffers,
                },
                // Only the side it faces (see the top of this file).
                primitive: wgpu::PrimitiveState { cull_mode: Some(wgpu::Face::Back), ..Default::default() },
                // Reverse depth, as in the main pass: behind the model, it
                // shows where it is nearer than what was drawn there.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(if in_front {
                        wgpu::CompareFunction::Always
                    } else {
                        wgpu::CompareFunction::GreaterEqual
                    }),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState { count: MSAA_SAMPLES, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_reference"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: gpu.format,
                        blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("reference pictures"),
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            anisotropy_clamp: 4,
            ..Default::default()
        });
        Layer {
            uniforms,
            bind,
            image_layout,
            sampler,
            pipelines: [pipeline(false), pipeline(true)],
            images: HashMap::new(),
            corners: None,
            quads: Vec::new(),
            stamp: Stamp::default(),
        }
    }

    /// Sets what the layer draws this frame: `pictures` (the scene's, shown
    /// or not) as `view` sees them. An image is sent the first time its
    /// picture is drawn; the images of pictures no longer in the list are
    /// let go.
    pub fn show(&mut self, gpu: &Gpu, view: &View, pictures: &[Picture]) {
        self.images.retain(|id, _| pictures.iter().any(|p| p.id == *id));
        self.quads.clear();
        let mut corners: Vec<Corner> = Vec::new();
        for picture in drawing_order(pictures, view) {
            if !self.images.contains_key(&picture.id) {
                let image = self.upload(gpu, &picture.image);
                self.images.insert(picture.id, image);
            }
            self.quads.push(Quad { id: picture.id, in_front: picture.place.in_front, first: corners.len() as u32 });
            corners.extend(corners_of(picture));
        }
        let view_proj = view.view_proj.to_cols_array();
        let bytes: &[u8] = bytemuck::cast_slice(&corners);
        // A scene with nothing to draw (most) sends the card nothing at all.
        if !self.quads.is_empty() {
            gpu.queue.write_buffer(&self.uniforms, 0, bytemuck::cast_slice(&view_proj));
            if self.corners.as_ref().is_none_or(|(_, room)| *room < bytes.len() as u64) {
                let room = (bytes.len() as u64).next_power_of_two().max(1024);
                let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("reference picture corners"),
                    size: room,
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.corners = Some((buffer, room));
            }
            if let Some((buffer, _)) = &self.corners {
                gpu.queue.write_buffer(buffer, 0, bytes);
            }
        }
        let drawn: Vec<[u64; 2]> = self.quads.iter().map(|q| [q.id, q.in_front as u64]).collect();
        let view_proj = if self.quads.is_empty() { &[][..] } else { bytemuck::cast_slice(&view_proj) };
        self.stamp.note(&[view_proj, bytes, bytemuck::cast_slice(&drawn)]);
    }

    /// Draws nothing until the next [`Self::show`], keeping the images on
    /// the card: for a picture of the scene without its reference pictures
    /// (a recent file's tile).
    pub fn hide(&mut self) {
        self.quads.clear();
        self.stamp.note(&[]);
    }

    /// Whether any picture is drawn this frame.
    pub(super) fn active(&self) -> bool {
        !self.quads.is_empty()
    }

    /// Sends `image` to the card with its smaller sizes, premultiplied, no
    /// larger than the card takes, and gives back what the shader reads it
    /// through.
    fn upload(&self, gpu: &Gpu, image: &RgbaImage) -> wgpu::BindGroup {
        let fitted = mips::fitted(image, gpu.device.limits().max_texture_dimension_2d);
        let image = premultiplied(&fitted);
        let smaller = mips::below(&image);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("reference picture"),
            size: wgpu::Extent3d { width: image.width(), height: image.height(), depth_or_array_layers: 1 },
            mip_level_count: 1 + smaller.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            // Not the sRGB format: the bytes go out to the screen as they
            // are (see references.wgsl).
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        for (level, image) in std::iter::once(&image).chain(&smaller).enumerate() {
            gpu.queue.write_texture(
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
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        gpu.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("reference picture"),
            layout: &self.image_layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: wgpu::BindingResource::TextureView(&view) },
                wgpu::BindGroupEntry { binding: 1, resource: wgpu::BindingResource::Sampler(&self.sampler) },
            ],
        })
    }

    /// Draws into the overlay pass, before the other overlays.
    pub fn draw(&self, pass: &mut wgpu::RenderPass) {
        let Some((corners, _)) = &self.corners else { return };
        if self.quads.is_empty() {
            return;
        }
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, corners.slice(..));
        for quad in &self.quads {
            let Some(image) = self.images.get(&quad.id) else { continue };
            pass.set_pipeline(&self.pipelines[quad.in_front as usize]);
            pass.set_bind_group(1, image, &[]);
            pass.draw(quad.first..quad.first + CORNERS, 0..1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::camera::Camera;
    use crate::references::Placement;
    use glam::{Vec2, Vec3};
    use std::sync::Arc;

    fn picture(place: Placement) -> Picture {
        let image = Arc::new(RgbaImage::from_pixel(4, 2, image::Rgba([10, 20, 30, 255])));
        Picture::new("p.png".into(), Arc::from(&b"p"[..]), image, place)
    }

    fn front_view() -> View {
        Camera::default().view(Vec2::ZERO, Vec2::new(800.0, 600.0), 5.0, 2.0)
    }

    #[test]
    fn a_pictures_triangles_face_its_view_and_carry_its_image_and_opacity() {
        let place = Placement { size: 2.0, opacity: 0.25, ..Placement::default() };
        let corners = corners_of(&picture(place));
        let toward = place.facing.axes().2;
        for triangle in corners.chunks_exact(3) {
            let [a, b, c] = [0, 1, 2].map(|i| Vec3::from_array(triangle[i].position));
            assert!((b - a).cross(c - a).dot(toward) > 0.0, "anticlockwise seen from its view: kept by the culling");
        }
        assert!(corners.iter().all(|c| c.opacity == 0.25));
        // The image's top left at the picture's top left, its bottom right at the bottom right.
        let top_left = corners.iter().find(|c| c.uv == [0.0, 0.0]).unwrap();
        let bottom_right = corners.iter().find(|c| c.uv == [1.0, 1.0]).unwrap();
        assert!(top_left.position[0] < bottom_right.position[0] && top_left.position[1] > bottom_right.position[1]);
        // Four pixels wide for two tall: twice as wide as it is tall.
        let xs: Vec<f32> = corners.iter().map(|c| c.position[0]).collect();
        let wide = xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min);
        assert!((wide - 4.0).abs() < 1e-5, "{wide}");
    }

    #[test]
    fn pictures_are_drawn_behind_ones_first_then_in_front_ones_each_farthest_first_and_hidden_ones_not_at_all() {
        let at = |depth: f32, in_front: bool| picture(Placement { depth, in_front, ..Placement::default() });
        let pictures = [
            at(1.0, true),
            at(3.0, false),
            at(5.0, true),
            at(2.0, false),
            picture(Placement { shown: false, ..Placement::default() }),
            picture(Placement { opacity: 0.0, ..Placement::default() }),
        ];
        let order: Vec<(f32, bool)> =
            drawing_order(&pictures, &front_view()).iter().map(|p| (p.place.depth, p.place.in_front)).collect();
        assert_eq!(order, [(3.0, false), (2.0, false), (5.0, true), (1.0, true)]);
    }

    #[test]
    fn a_see_through_edge_is_premultiplied_and_a_solid_one_left_as_it_is() {
        let mut image = RgbaImage::from_pixel(2, 1, image::Rgba([200, 100, 50, 255]));
        image.put_pixel(1, 0, image::Rgba([200, 100, 50, 128]));
        let out = premultiplied(&image);
        assert_eq!(out.get_pixel(0, 0).0, [200, 100, 50, 255]);
        assert_eq!(out.get_pixel(1, 0).0, [100, 50, 25, 128]);
    }
}
