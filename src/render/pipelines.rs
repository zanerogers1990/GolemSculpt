//! The main pass's pipelines, one for each way a surface can be shaded and
//! each way its faces can be culled.
//!
//! **One per finish.** sculpt_pbr.wgsl has every finish in it, and with the
//! finish picked at run time every pixel of plain clay paid for the
//! registers the plastic's flake tracing needs, which limits how many
//! pixels the card shades at once. The finish (or the matcap) and whether
//! the glow goes over it are pipeline constants instead (`FINISH`, `GLOW`),
//! so each pipeline is compiled with only its own code.
//!
//! **Culling.** The faces turned away from the camera are not drawn, unless
//! Show Backfaces is on: a closed model's are hidden behind its front anyway,
//! so this halves the triangles the card rasterises for nothing. A live
//! mirror copy is drawn through a reflection, which turns every face's
//! corners the other way round on the screen, so for a copy it is the
//! hardware's front faces that are culled.
//!
//! **When they are built.** Twelve surfaces times three ways of culling is
//! thirty-six pipelines, a few seconds of compiling. The app has them built
//! on a thread of their own from the start ([`MeshPipelines::prewarm`]), the
//! likeliest first, so picking a finish later does not stop the window for a
//! moment; any that is asked for before it has arrived is built there and
//! then.

use std::collections::HashMap;
use std::sync::mpsc;

/// Which faces a draw leaves out.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Cull {
    /// None: both sides are drawn (Show Backfaces).
    Nothing,
    /// Those turned away from the camera, for an ordinary placement.
    Back,
    /// The hardware's front faces, which are the turned-away ones of a
    /// mirrored placement.
    Front,
}

impl Cull {
    pub const ALL: [Cull; 3] = [Cull::Nothing, Cull::Back, Cull::Front];

    /// How an object is culled: nothing with Show Backfaces on, otherwise
    /// its far side, which for a mirrored object is the hardware's front.
    pub fn of(show_backfaces: bool, mirrored: bool) -> Cull {
        match (show_backfaces, mirrored) {
            (true, _) => Cull::Nothing,
            (false, false) => Cull::Back,
            (false, true) => Cull::Front,
        }
    }

    /// The opposite face: what the plastic's back-face pass keeps.
    pub fn flipped(self) -> Cull {
        match self {
            Cull::Nothing => Cull::Nothing,
            Cull::Back => Cull::Front,
            Cull::Front => Cull::Back,
        }
    }

    pub fn face(self) -> Option<wgpu::Face> {
        match self {
            Cull::Nothing => None,
            Cull::Back => Some(wgpu::Face::Back),
            Cull::Front => Some(wgpu::Face::Front),
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }
}

/// Whether an object's matrix (by columns, as in `DrawItem`) mirrors, as
/// `handedness` in shader.wgsl works it out.
pub fn mirrored(model: &[[f32; 4]; 4]) -> bool {
    let col = |i: usize| glam::Vec3::new(model[i][0], model[i][1], model[i][2]);
    col(0).dot(col(1).cross(col(2))) < 0.0
}

/// What the main pass shades with: `FINISH` in sculpt_pbr.wgsl (0 plain,
/// 1 plastic, 2 metal, 3 clay, 4 wood, 5 the matcap), the glow over it or
/// not, and the culling.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct MeshKey {
    pub surface: u32,
    pub glow: bool,
    pub cull: Cull,
}

pub const SURFACE_MATCAP: u32 = 5;

/// The main pass's vertex buffers: positions, normals, mask, the objects'
/// matrices (per instance) and curvature, at the shader's locations.
pub fn mesh_vertex_buffers() -> [Option<wgpu::VertexBufferLayout<'static>>; 5] {
    const POSITION: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![0 => Float32x3];
    const NORMAL: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![1 => Float32x3];
    const MASK: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![2 => Float32];
    const OBJECT: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![3 => Float32x4, 4 => Float32x4, 5 => Float32x4, 6 => Float32x4];
    const CURVATURE: [wgpu::VertexAttribute; 1] = wgpu::vertex_attr_array![7 => Float32];
    let vertex = |stride, attributes| {
        Some(wgpu::VertexBufferLayout { array_stride: stride, step_mode: wgpu::VertexStepMode::Vertex, attributes })
    };
    [
        vertex(12, &POSITION),
        vertex(12, &NORMAL),
        vertex(4, &MASK),
        Some(wgpu::VertexBufferLayout {
            array_stride: 64,
            step_mode: wgpu::VertexStepMode::Instance,
            attributes: &OBJECT,
        }),
        vertex(4, &CURVATURE),
    ]
}

/// Everything a main-pass pipeline is built from, cheap to hand to another thread.
#[derive(Clone)]
struct Recipe {
    device: wgpu::Device,
    shader: wgpu::ShaderModule,
    layout: wgpu::PipelineLayout,
    color: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    samples: u32,
}

impl Recipe {
    fn build(&self, key: MeshKey) -> wgpu::RenderPipeline {
        let constants = [("FINISH", key.surface as f64), ("GLOW", if key.glow { 1.0 } else { 0.0 })];
        let buffers = mesh_vertex_buffers();
        self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("mesh"),
            layout: Some(&self.layout),
            vertex: wgpu::VertexState {
                module: &self.shader,
                entry_point: Some("vs_mesh"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState { cull_mode: key.cull.face(), ..Default::default() },
            // Reverse depth: nearer fragments have larger depth values.
            depth_stencil: Some(wgpu::DepthStencilState {
                format: self.depth,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: self.samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module: &self.shader,
                entry_point: Some("fs_mesh"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &[Some(self.color.into())],
            }),
            multiview_mask: None,
            cache: None,
        })
    }
}

/// The main pass's pipelines, built as they are asked for or ahead of time.
pub struct MeshPipelines {
    recipe: Recipe,
    ready: HashMap<MeshKey, wgpu::RenderPipeline>,
    arriving: Option<mpsc::Receiver<(MeshKey, wgpu::RenderPipeline)>>,
}

impl MeshPipelines {
    pub fn new(
        device: &wgpu::Device,
        shader: &wgpu::ShaderModule,
        layout: &wgpu::PipelineLayout,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> MeshPipelines {
        let recipe = Recipe {
            device: device.clone(),
            shader: shader.clone(),
            layout: layout.clone(),
            color,
            depth,
            samples,
        };
        MeshPipelines { recipe, ready: HashMap::new(), arriving: None }
    }

    /// Every pipeline, in the order they are likeliest to be wanted (the
    /// matcap and the plain lit look first, culled as for an ordinary
    /// object), for [`Self::prewarm`].
    pub fn every_key() -> Vec<MeshKey> {
        let surfaces = [SURFACE_MATCAP, 0, 3, 2, 4, 1];
        let mut keys = Vec::new();
        for cull in [Cull::Back, Cull::Front, Cull::Nothing] {
            for glow in [false, true] {
                for surface in surfaces {
                    keys.push(MeshKey { surface, glow, cull });
                }
            }
        }
        keys
    }

    /// Starts building every pipeline on a thread of its own.
    pub fn prewarm(&mut self) {
        if self.arriving.is_some() {
            return;
        }
        let (send, receive) = mpsc::channel();
        let recipe = self.recipe.clone();
        let started = std::thread::Builder::new().name("pipelines".into()).spawn(move || {
            for key in Self::every_key() {
                if send.send((key, recipe.build(key))).is_err() {
                    return;
                }
            }
        });
        if started.is_ok() {
            self.arriving = Some(receive);
        }
    }

    /// The pipeline for `key`, built now if it has not been.
    pub fn get(&mut self, key: MeshKey) -> &wgpu::RenderPipeline {
        if let Some(arriving) = &self.arriving {
            for (k, pipeline) in arriving.try_iter() {
                self.ready.entry(k).or_insert(pipeline);
            }
        }
        let recipe = &self.recipe;
        self.ready.entry(key).or_insert_with(|| recipe.build(key))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_pipeline_is_built_once_and_the_likeliest_first() {
        let keys = MeshPipelines::every_key();
        assert_eq!(keys.len(), 6 * 2 * 3);
        let unique: std::collections::HashSet<MeshKey> = keys.iter().copied().collect();
        assert_eq!(unique.len(), keys.len());
        assert_eq!(keys[0], MeshKey { surface: SURFACE_MATCAP, glow: false, cull: Cull::Back });
        assert_eq!(keys[1], MeshKey { surface: 0, glow: false, cull: Cull::Back });
    }

    #[test]
    fn a_mirrored_object_is_culled_on_the_hardware_front_and_nothing_is_with_backfaces_shown() {
        let plain = glam::Mat4::from_rotation_y(0.7).to_cols_array_2d();
        let mirror = (glam::Mat4::from_scale(glam::Vec3::new(-1.0, 1.0, 1.0)) * glam::Mat4::from_rotation_y(0.7))
            .to_cols_array_2d();
        assert!(!mirrored(&plain));
        assert!(mirrored(&mirror));
        assert_eq!(Cull::of(false, mirrored(&plain)), Cull::Back);
        assert_eq!(Cull::of(false, mirrored(&mirror)), Cull::Front);
        assert_eq!(Cull::of(true, true), Cull::Nothing);
        assert_eq!(Cull::Back.flipped(), Cull::Front);
        assert_eq!(Cull::Nothing.flipped(), Cull::Nothing);
        // The active flag in the last element does not change the answer.
        let mut flagged = mirror;
        flagged[3][3] = 0.0;
        assert!(mirrored(&flagged));
    }
}

/// The surface shader's `Look` uniform and finish functions against the Rust
/// side that fills them (look.rs): the same layout, and each finish reading
/// no more of its settings than its packer writes.
#[cfg(test)]
mod layout_tests {
    use crate::look::{self, FinishParams, Look, LookUniform};

    const SHADER: &str = include_str!("sculpt_pbr.wgsl");

    /// The bytes a WGSL type of the `Look` struct takes (all are 16-byte multiples).
    fn size_of_wgsl(ty: &str) -> usize {
        let ty = ty.trim();
        if ty == "vec4<f32>" {
            16
        } else if ty == "mat4x4<f32>" {
            64
        } else if ty == "FinishParams" {
            256
        } else if let Some(n) = ty.strip_prefix("array<vec4<f32>,").and_then(|r| r.strip_suffix('>')) {
            16 * n.trim().parse::<usize>().unwrap()
        } else {
            panic!("a type this test does not know: {ty}")
        }
    }

    /// Each field of WGSL struct `name`, with its byte offset.
    fn wgsl_fields(name: &str) -> Vec<(String, usize, usize)> {
        let start = SHADER.find(&format!("struct {name} {{")).unwrap();
        let body = &SHADER[start..start + SHADER[start..].find("};").unwrap()];
        let mut at = 0;
        let mut out = Vec::new();
        for line in body.lines().skip(1) {
            let line = line.trim();
            if line.starts_with("//") || line.is_empty() {
                continue;
            }
            let (field, ty) = line.trim_end_matches(',').split_once(':').unwrap();
            let size = size_of_wgsl(ty);
            out.push((field.trim().to_string(), at, size));
            at += size;
        }
        out
    }

    #[test]
    fn the_look_uniform_is_laid_out_as_the_shader_reads_it() {
        assert_eq!(std::mem::size_of::<FinishParams>(), 256);
        assert_eq!(wgsl_fields("FinishParams").iter().map(|f| f.2).sum::<usize>(), 256);
        let fields = wgsl_fields("Look");
        let total: usize = fields.iter().map(|f| f.2).sum();
        assert_eq!(total, std::mem::size_of::<LookUniform>());
        let offset = |name: &str| fields.iter().find(|f| f.0 == name).map(|f| f.1).expect(name);
        macro_rules! same {
            ($($field:ident),*) => {
                $(assert_eq!(offset(stringify!($field)), std::mem::offset_of!(LookUniform, $field), stringify!($field));)*
                assert_eq!(fields.len(), [$(stringify!($field)),*].len(), "the shader has fields the Rust side does not");
            };
        }
        same!(
            light_dir, light_color, light_params, shadow_view_proj, shadow_params, shadow_soft, sh, ambient_sky, ambient_equator,
            ambient_ground, env_params, cam_params, bg_params, base_params, matcap_params, mask_tint, finish, glow,
            floor, floor_more
        );
    }

    /// The highest `params.v[N]` the shader's `fn finish_<name>` reads, plus one.
    fn slots_read(name: &str) -> usize {
        let start = SHADER.find(&format!("fn finish_{name}(")).unwrap();
        let end = start + SHADER[start..].find("\n}").unwrap();
        let body = &SHADER[start..end];
        let mut most = 0;
        for (i, _) in body.match_indices("params.v[") {
            let rest = &body[i + "params.v[".len()..];
            let n: usize = rest[..rest.find(']').unwrap()].parse().unwrap();
            most = most.max(n + 1);
        }
        assert!(!body.contains("look."), "finish_{name} reads the look uniform: its settings come in `params`");
        most
    }

    #[test]
    fn each_finish_reads_the_slots_its_packer_writes_and_no_more() {
        assert_eq!(slots_read("pbr"), look::PBR_SLOTS);
        assert_eq!(slots_read("plastic"), look::PLASTIC_SLOTS);
        assert_eq!(slots_read("metal"), look::METAL_SLOTS);
        assert_eq!(slots_read("clay"), look::CLAY_SLOTS);
        assert_eq!(slots_read("wood"), look::WOOD_SLOTS);
        // The glow reads the clock, which is not one of its settings.
        let glow = &SHADER[SHADER.find("fn finish_glow(").unwrap()..];
        let glow = &glow[..glow.find("\n}").unwrap()];
        assert_eq!(glow.matches("look.").count(), 1, "only the clock");
        assert!(glow.contains("look.light_params.w"));
    }

    fn frame() -> look::Frame {
        look::Frame {
            cam_right: glam::Vec3::X,
            cam_up: glam::Vec3::Y,
            cam_forward: glam::Vec3::NEG_Z,
            ortho_half_height: None,
            eye_offset: 0.0,
            scene_min: glam::Vec3::splat(-1.0),
            scene_max: glam::Vec3::splat(1.0),
            model_size: 2.0,
            time: 3.0,
            gradient_background: false,
            smoothness: 0.37,
            shadow_size: 2048,
        }
    }

    #[test]
    fn the_active_finish_and_the_glow_go_into_the_uniform_as_their_packers_make_them() {
        let mut look = Look::default();
        let u = look.uniform(&frame());
        assert_eq!(u.finish.v[0], [0.0, 0.37, 0.0, 0.0], "plain: metallic, then the model's smoothness");
        assert_eq!(u.glow, bytemuck::Zeroable::zeroed(), "no glow");
        look.metallic = 0.4;
        look.set_finish(look::Finish::Clay, 2.0);
        look.clay.recess = 0.5;
        let u = look.uniform(&frame());
        assert_eq!(u.matcap_params[2], 3.0);
        assert_eq!(u.finish, look.pack_clay(2.0));
        let preset = &look::CLAYS[look.clay.preset];
        assert_eq!(u.finish.v[4], [preset.recess_depth * 0.5, preset.ridge_burnish, preset.mottle, preset.mottle_size * 2.0]);
        assert!(u.finish.v[look::CLAY_SLOTS..].iter().all(|v| *v == [0.0; 4]));
        look.set_glow(true, 2.0);
        let u = look.uniform(&frame());
        assert_eq!(u.glow, look.pack_glow(2.0));
        assert_eq!(u.glow.v[2][0], 1.0, "on");
        for finish in [look::Finish::None, look::Finish::LurePlastic, look::Finish::Metal, look::Finish::Wood] {
            look.set_finish(finish, 2.0);
            assert_eq!(look.uniform(&frame()).finish, look.finish_params(finish, &frame()));
        }
        // Metallic belongs to the plain finish alone.
        look.set_finish(look::Finish::None, 2.0);
        assert_eq!(look.uniform(&frame()).finish.v[0][0], 0.4);
    }
}
