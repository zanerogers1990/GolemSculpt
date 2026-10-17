//! The floor under the model (Backdrop & Effects, Floor): a shadow catcher.
//! It is not a surface lit like the model but the backdrop itself, darkened
//! where the key light's shadow falls on it and where the ambient occlusion
//! finds the model close above it, and, glossy, carrying a soft reflection
//! of the model; all of it fading out into the backdrop toward its edge, so
//! it has no edge to see. Its shader is `vs_floor` / `fs_floor` in
//! sculpt_pbr.wgsl, and its place and settings are in the look uniform.
//!
//! The reflection is the classic planar one: the model drawn again with
//! every object's placement mirrored in the floor's plane and the lights
//! mirrored with it, seen by the real camera, into a half-size texture the
//! floor then reads at its own pixel, blurred by its roughness. That pass
//! has uniforms of its own, so it goes to the graphics card as a batch of
//! work before the frame's, and only while the reflection is asked for.

use super::{target_texture, Globals, Gpu, DEPTH_FORMAT, HDR_FORMAT, MSAA_SAMPLES};
use crate::look::LookUniform;

/// Corners the floor is drawn with: two triangles.
pub const VERTICES: u32 = 6;

pub struct Floor {
    /// Over the backdrop in the scene pass.
    pub pipe: wgpu::RenderPipeline,
    /// In the normals prepass, so the occlusion sees where the model meets it.
    pub pipe_prepass: wgpu::RenderPipeline,
    /// A texture of nothing, bound where the reflection would be while the
    /// reflection itself is being drawn.
    pub nothing: wgpu::TextureView,
}

/// The reflection's textures for one picture size.
pub struct Reflection {
    /// Half the picture's size: multisampled colour and depth, and the
    /// colour resolved for the floor to read.
    msaa: wgpu::TextureView,
    depth: wgpu::TextureView,
}

/// The size the reflection is drawn at, for a picture of `size`.
pub fn reflection_size(size: (u32, u32)) -> (u32, u32) {
    (size.0.div_ceil(2).max(1), size.1.div_ceil(2).max(1))
}

/// Where a point goes mirrored in the floor at `height`, as a matrix (its
/// own inverse).
pub fn mirror(height: f32) -> glam::Mat4 {
    glam::Mat4::from_translation(glam::Vec3::new(0.0, 2.0 * height, 0.0)) * glam::Mat4::from_scale(glam::Vec3::new(1.0, -1.0, 1.0))
}

/// The frame's uniforms for the reflection: the lights mirrored as the
/// model is (so its underside is the dark side it really is), and none of
/// what reads the frame's own helper textures, which are of the unmirrored
/// picture at full size: the key's shadow, the occlusion, the cavity, the
/// plastic's back faces. A blurred reflection shows none of them anyway.
pub fn mirrored_uniforms(globals: &Globals, look: &LookUniform) -> (Globals, LookUniform) {
    let mut g = *globals;
    g.shading[2] = 0.0;
    g.shading[3] = 0.0;
    g.symmetry[0] = 0.0;
    g.symmetry[1] = 0.0;
    g.symmetry[2] = 0.0;
    g.radial_axis[3] = 0.0;
    let mut l = *look;
    for dir in &mut l.light_dir {
        dir[1] = -dir[1];
    }
    // Softboxes mirrored too, one side turned back so they still face the
    // mirrored model (a mirror reverses which way round their corners go).
    let height = look.floor[1];
    for i in 0..l.light_pos.len() {
        l.light_pos[i][1] = 2.0 * height - l.light_pos[i][1];
        let side = &mut l.light_side[i];
        (side[0], side[1], side[2]) = (-side[0], side[1], -side[2]);
        l.light_up[i][1] = -l.light_up[i][1];
    }
    l.light_params[1] = 0.0;
    l.env_params[2] = 0.0;
    l.matcap_params[3] = 0.0;
    l.floor[0] = 0.0;
    (g, l)
}

impl Floor {
    pub fn new(device: &wgpu::Device, shader: &wgpu::ShaderModule, layout_main: &wgpu::PipelineLayout, layout_pre: &wgpu::PipelineLayout, gbuf: wgpu::TextureFormat) -> Floor {
        let pipeline = |label, layout, entry, format: wgpu::TextureFormat, samples, blend, write| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(layout),
                vertex: wgpu::VertexState {
                    module: shader,
                    entry_point: Some("vs_floor"),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                // Reverse depth: in front of what was drawn, or level with it.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(write),
                    depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState { count: samples, mask: !0, alpha_to_coverage_enabled: false },
                fragment: Some(wgpu::FragmentState {
                    module: shader,
                    entry_point: Some(entry),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState { format, blend, write_mask: wgpu::ColorWrites::ALL })],
                }),
                multiview_mask: None,
                cache: None,
            })
        };
        Floor {
            // Premultiplied over the backdrop: the shadow darkens what is
            // there, the reflection is added to it.
            pipe: pipeline("floor", layout_main, "fs_floor", HDR_FORMAT, MSAA_SAMPLES, Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING), false),
            pipe_prepass: pipeline("floor prepass", layout_pre, "fs_floor_prepass", gbuf, 1, None, true),
            nothing: target_texture(device, "no reflection", (1, 1), HDR_FORMAT, 1, true),
        }
    }
}

impl Reflection {
    pub fn new(gpu: &Gpu, size: (u32, u32)) -> Reflection {
        let size = reflection_size(size);
        Reflection {
            msaa: target_texture(&gpu.device, "floor reflection", size, HDR_FORMAT, MSAA_SAMPLES, false),
            depth: target_texture(&gpu.device, "floor reflection depth", size, DEPTH_FORMAT, MSAA_SAMPLES, false),
        }
    }

    /// A pass into the reflection, cleared to nothing (alpha 0), resolving
    /// into `resolved`.
    pub fn pass<'a>(&'a self, encoder: &'a mut wgpu::CommandEncoder, resolved: &'a wgpu::TextureView) -> wgpu::RenderPass<'a> {
        encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("floor reflection"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &self.msaa,
                depth_slice: None,
                resolve_target: Some(resolved),
                ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT), store: wgpu::StoreOp::Discard },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &self.depth,
                depth_ops: Some(wgpu::Operations { load: wgpu::LoadOp::Clear(0.0), store: wgpu::StoreOp::Discard }),
                stencil_ops: None,
            }),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;
    use glam::Vec3;

    #[test]
    fn the_mirror_reflects_in_the_floor_and_undoes_itself() {
        let m = mirror(-0.5);
        assert_eq!(m.transform_point3(Vec3::new(1.0, 0.5, 2.0)), Vec3::new(1.0, -1.5, 2.0));
        assert_eq!(m.transform_point3(Vec3::new(3.0, -0.5, 0.0)), Vec3::new(3.0, -0.5, 0.0), "the floor stays put");
        assert!((m * m).abs_diff_eq(glam::Mat4::IDENTITY, 1e-6));
        assert!(m.determinant() < 0.0, "a mirror: culled the other way round");
    }

    #[test]
    fn the_reflection_is_lit_by_mirrored_lights_and_reads_none_of_the_frames_helpers() {
        let mut look = LookUniform::zeroed();
        look.light_dir[0] = [0.3, 0.8, 0.5, 0.0];
        look.light_params[1] = 1.0;
        look.env_params[2] = 1.0;
        look.floor[0] = 1.0;
        let mut globals = Globals::zeroed();
        globals.shading = [0.0, 0.0, 1.0, 1.0];
        globals.symmetry = [1.0, 1.0, 1.0, 1.0];
        let (g, l) = mirrored_uniforms(&globals, &look);
        assert_eq!(l.light_dir[0], [0.3, -0.8, 0.5, 0.0]);
        // A softbox: mirrored in the floor, and still lighting the side the
        // mirrored model is on (its corners' winding normal still points away).
        let mut boxed = look;
        boxed.floor[1] = -1.0;
        boxed.light_pos[0] = [0.0, 3.0, 2.0, 1.0];
        boxed.light_side[0] = [1.0, 0.0, 0.0, 5.0];
        boxed.light_up[0] = [0.0, 0.6, -0.8, 0.0];
        let (_, m) = mirrored_uniforms(&globals, &boxed);
        assert_eq!(m.light_pos[0], [0.0, -5.0, 2.0, 1.0]);
        let normal = |l: &LookUniform| {
            let (s, u) = (glam::Vec3::from_slice(&l.light_side[0][..3]), glam::Vec3::from_slice(&l.light_up[0][..3]));
            s.cross(u)
        };
        let reflect = glam::Vec3::new(1.0, -1.0, 1.0);
        assert!(normal(&m).abs_diff_eq(normal(&boxed) * reflect, 1e-6), "{} {}", normal(&m), normal(&boxed));
        assert_eq!(m.light_side[0][3], 5.0, "the same radiance");
        assert_eq!((l.light_params[1], l.env_params[2], l.floor[0]), (0.0, 0.0, 0.0));
        assert_eq!(g.shading, [0.0, 0.0, 0.0, 0.0]);
        assert_eq!(g.symmetry, [0.0, 0.0, 0.0, 1.0], "the cavity's pixel offset is left alone");
        assert_eq!(reflection_size((1921, 1080)), (961, 540));
    }
}
