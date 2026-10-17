//! Drawing painted objects: polypaint colour and painted materials (B.7 of
//! the paint design). An object without paint is drawn exactly as before,
//! by the per-finish pipelines of pipelines.rs; one with paint by a
//! "painted" pipeline from here.
//!
//! **What the card gets.** A mesh with paint has one more vertex buffer, the
//! paint composite of `mesh::paint` (16 bytes a vertex: colour and coverage,
//! the roughness and metallic overrides with their weights, eight material
//! weights), sent again only where it changed, as the mask is. The scene's
//! palette is a storage buffer of [`materials::rows::Row`]s, and each drawn
//! object's local material table (which palette rows its eight weights are
//! of) is two `vec4<u32>`s in another, read by the object's instance number:
//! the 64-byte buffer of objects' matrices is left as it is.
//!
//! **One pipeline per palette.** The painted shader is painted.wgsl and a
//! part generated for the palette in use (`generated`): `eval_slot`, a
//! switch with a case only for the finishes the palette has, and the custom
//! materials' functions. So a palette of clays does not carry the plastic's
//! flake tracing, and a custom material's code is only in the scenes that
//! use it. Each is built when first needed, then kept.
//!
//! **Custom shaders cannot close the app.** Their source has been through
//! naga before it gets here (materials/custom.rs). The whole painted source
//! is validated by naga again, then created on the card inside an error
//! scope; anything refused is noted, the pipeline is built again without the
//! custom materials (which then show as their swatch), and the status line
//! says why.

use std::collections::{BTreeSet, HashMap, HashSet};
use std::ops::Range;

use super::cache::Stamp;
use super::pipelines::{self, Cull};
use super::Gpu;
use crate::materials::Kind;
use crate::materials::rows::{Code, Row, Upload};

/// The paint composite's bytes a vertex (see `mesh::paint::composite`).
pub const STRIDE: u64 = 16;

/// The painted pipelines' sixth vertex buffer: the paint composite, as four
/// `Unorm8x4`s at locations 8 to 11 (the main pass uses 0 to 7).
pub fn vertex_buffer() -> wgpu::VertexBufferLayout<'static> {
    const PAINT: [wgpu::VertexAttribute; 4] =
        wgpu::vertex_attr_array![8 => Unorm8x4, 9 => Unorm8x4, 10 => Unorm8x4, 11 => Unorm8x4];
    wgpu::VertexBufferLayout { array_stride: STRIDE, step_mode: wgpu::VertexStepMode::Vertex, attributes: &PAINT }
}

/// Whether an object is drawn by a painted pipeline: only one whose mesh has
/// paint on the card, and only while painted colour or materials show.
/// Every other object is drawn exactly as before there was paint.
pub fn drawn_painted(has_paint: bool, show_colour: bool, show_materials: bool) -> bool {
    has_paint && (show_colour || show_materials)
}

/// The weight below which a material is not evaluated: one step of a byte.
#[cfg(test)]
pub const EPS: f32 = 1.0 / 255.0;

/// The CPU twin of `paint_at`'s weights in painted.wgsl: the eight
/// interpolated weights `raw` of a pixel made a mixture. Each is clamped to
/// 0..1, one below a byte's step or with no palette row (`present`) is
/// dropped, all are scaled down if together they come to more than one, and
/// the Look's finish gets what is left, unless that is below a byte's step,
/// when it goes to the materials. Gives back the Look's share and each
/// material's: never negative, and summing to one.
#[cfg(test)]
pub fn resolve_weights(raw: [f32; 8], present: [bool; 8]) -> (f32, [f32; 8]) {
    let mut w = [0.0f32; 8];
    let mut total = 0.0;
    for k in 0..8 {
        let x = raw[k].clamp(0.0, 1.0);
        w[k] = if present[k] && x >= EPS { x } else { 0.0 };
        total += w[k];
    }
    if total > 1.0 {
        w.iter_mut().for_each(|x| *x /= total);
        total = 1.0;
    }
    let mut base = 1.0 - total;
    if base < EPS {
        if total > 0.0 {
            w.iter_mut().for_each(|x| *x /= total);
        }
        base = 0.0;
    }
    (base, w)
}

/// What to send of a mesh's paint this frame.
#[derive(Clone, Debug, PartialEq)]
pub enum Sync {
    Nothing,
    /// The mesh has no paint (any more): its buffer goes.
    Drop,
    /// A new buffer, holding the whole composite.
    Create,
    /// These vertex ranges of the buffer it has.
    Write(Vec<Range<u32>>),
}

impl Sync {
    /// What to do, given whether the mesh's buffers have paint, how many
    /// records the composite has (0 without paint), how many vertices the
    /// mesh has, whether its buffers were made afresh or outgrown this frame
    /// (`rebuilt`), and the ranges of the composite that changed.
    pub fn plan(has_buffer: bool, stream: usize, vertices: usize, rebuilt: bool, dirty: &[Range<u32>]) -> Sync {
        if stream == 0 || stream != vertices {
            return if has_buffer { Sync::Drop } else { Sync::Nothing };
        }
        if !has_buffer || rebuilt {
            Sync::Create
        } else if dirty.is_empty() {
            Sync::Nothing
        } else {
            Sync::Write(dirty.to_vec())
        }
    }

    /// Whether the picture is drawn from something new afterwards (the
    /// scene cache must not hand the old one out again).
    pub fn changes_picture(&self) -> bool {
        *self != Sync::Nothing
    }
}

/// The part of the painted shader made for a palette: `eval_slot` with a
/// case for each finish in `kinds`, and `eval_custom` with one for each of
/// `customs` (whose functions follow).
pub fn generated(kinds: &BTreeSet<Kind>, customs: &[Code]) -> String {
    let mut out = String::from(
        "\n// ---- made for the palette in use (render/paint.rs) ----\n\n\
         fn eval_slot(row: u32, s: Surface) -> MaterialSample {\n    let e = palette[row];\n    \
         var m: MaterialSample;\n    switch u32(e.head.x + 0.5) {\n",
    );
    for kind in kinds {
        let case = match kind {
            Kind::Plain => "m = plain_of(e, s, e.base.rgb);",
            Kind::Lure => "m = finish_plastic(s, e.params);",
            Kind::Metal => "m = finish_metal(s, e.params);",
            Kind::Clay => "m = finish_clay(s, e.params);",
            Kind::Wood => "m = finish_wood(s, e.params);",
            Kind::Resin => "m = finish_resin(s, e.params);",
            Kind::Glow => {
                "m = plain_of(e, s, e.base.rgb);\n            let glow = finish_glow(s, e.params);\n            \
                 m.emission += glow.emission;\n            m.dim *= glow.dim;"
            }
            Kind::Custom if !customs.is_empty() => "m = eval_custom(u32(e.head.w + 0.5), e, s);",
            Kind::Custom | Kind::Swatch => continue,
        };
        out.push_str(&format!("        case {}u: {{\n            {case}\n        }}\n", *kind as u32));
    }
    out.push_str("        default: {\n            m = plain_of(e, s, e.swatch.rgb);\n        }\n    }\n    return m;\n}\n");
    if !customs.is_empty() {
        out.push_str("\nfn eval_custom(i: u32, e: PaletteRow, s: Surface) -> MaterialSample {\n    var m: MaterialSample;\n    switch i {\n");
        for (i, code) in customs.iter().enumerate() {
            out.push_str(&format!("        case {i}u: {{\n            m = {}(s, e.params);\n        }}\n", code.function));
        }
        out.push_str("        default: {\n            m = plain_of(e, s, e.swatch.rgb);\n        }\n    }\n    return m;\n}\n");
        for code in customs {
            out.push_str(&format!("\n// ---- custom material {} ----\n{}\n", code.id, code.source));
        }
    }
    out
}

/// The whole painted shader for a palette.
pub fn source(kinds: &BTreeSet<Kind>, customs: &[Code]) -> String {
    let mut out = String::new();
    out.push_str(include_str!("shader.wgsl"));
    out.push_str(include_str!("sculpt_pbr.wgsl"));
    out.push_str(include_str!("painted.wgsl"));
    out.push_str(&generated(kinds, customs));
    out
}

/// Checks a shader with naga as the card would, without the card.
pub fn validate(source: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), naga::valid::Capabilities::default())
        .validate(&module)
        .map_err(|e| e.emit_to_string(source))?;
    Ok(())
}

/// Which palette a painted shader is made for: its finishes, and its
/// custom materials by id and the hash of their files.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct ModuleKey {
    pub kinds: Vec<u32>,
    pub customs: Vec<(String, u64)>,
}

impl ModuleKey {
    pub fn of(upload: &Upload) -> ModuleKey {
        ModuleKey {
            kinds: upload.kinds.iter().map(|k| *k as u32).collect(),
            customs: upload.customs.iter().map(|c| (c.id.clone(), c.hash)).collect(),
        }
    }
}

/// One painted pipeline: the palette's shader, the Look's finish (or the
/// matcap) as `FINISH`, the glow, and the culling.
#[derive(Clone, PartialEq, Eq, Hash, Debug)]
pub struct Key {
    pub module: ModuleKey,
    pub surface: u32,
    pub glow: bool,
    pub cull: Cull,
}

/// The painted pipelines and what they read besides the main pass's inputs.
pub struct Painted {
    device: wgpu::Device,
    pub layout: wgpu::BindGroupLayout,
    pipeline_layout: wgpu::PipelineLayout,
    color: wgpu::TextureFormat,
    depth: wgpu::TextureFormat,
    samples: u32,
    palette: wgpu::Buffer,
    palette_room: usize,
    tables: wgpu::Buffer,
    tables_room: usize,
    flags: wgpu::Buffer,
    bind: Option<wgpu::BindGroup>,
    upload: Upload,
    show: [f32; 4],
    /// Moves whenever the palette's rows or the show switches change.
    pub stamp: Stamp,
    modules: HashMap<ModuleKey, Option<wgpu::ShaderModule>>,
    pipelines: HashMap<Key, Option<wgpu::RenderPipeline>>,
    /// Custom materials the card refused, by hash: built without from then on.
    refused: HashSet<u64>,
    /// What there is to say that has not been said (taken by the app).
    pub said: Vec<String>,
}

fn storage(device: &wgpu::Device, label: &str, size: u64) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(label),
        size: size.max(16),
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

impl Painted {
    pub fn new(
        gpu: &Gpu,
        layout_main: &wgpu::BindGroupLayout,
        color: wgpu::TextureFormat,
        depth: wgpu::TextureFormat,
        samples: u32,
    ) -> Painted {
        let device = &gpu.device;
        let read_only = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only: true },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("paint"),
            entries: &[
                read_only(0),
                read_only(1),
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("painted"),
            bind_group_layouts: &[Some(layout_main), Some(&layout)],
            immediate_size: 0,
        });
        let row = std::mem::size_of::<Row>();
        let flags = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("paint flags"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let upload = Upload::default();
        let palette = storage(device, "palette", (row * 16) as u64);
        gpu.queue.write_buffer(&palette, 0, bytemuck::cast_slice(&upload.rows));
        Painted {
            device: device.clone(),
            layout,
            pipeline_layout,
            color,
            depth,
            samples,
            palette,
            palette_room: 16,
            tables: storage(device, "paint tables", 32 * 16),
            tables_room: 16,
            flags,
            bind: None,
            upload,
            show: [-1.0; 4],
            stamp: Stamp::default(),
            modules: HashMap::new(),
            pipelines: HashMap::new(),
            refused: HashSet::new(),
            said: Vec::new(),
        }
    }

    /// The palette as the shader is to read it from now on.
    pub fn set_palette(&mut self, gpu: &Gpu, upload: &Upload) {
        if *upload == self.upload {
            return;
        }
        if upload.rows.len() > self.palette_room {
            self.palette_room = upload.rows.len().next_power_of_two();
            let size = (self.palette_room * std::mem::size_of::<Row>()) as u64;
            self.palette = storage(&gpu.device, "palette", size);
            self.bind = None;
        }
        gpu.queue.write_buffer(&self.palette, 0, bytemuck::cast_slice(&upload.rows));
        self.stamp.note(&[bytemuck::cast_slice(&upload.rows), bytemuck::cast_slice(&upload.slot_row)]);
        for failure in &upload.failures {
            if !self.upload.failures.contains(failure) {
                self.said.push(failure.clone());
            }
        }
        self.upload = upload.clone();
    }

    pub fn upload(&self) -> &Upload {
        &self.upload
    }

    /// Which of the painted colour and the painted materials show.
    pub fn set_show(&mut self, gpu: &Gpu, colour: bool, materials: bool) {
        let show = [colour as u32 as f32, materials as u32 as f32, 0.0, 0.0];
        if show != self.show {
            self.show = show;
            gpu.queue.write_buffer(&self.flags, 0, bytemuck::bytes_of(&show));
            self.stamp.note(&[bytemuck::bytes_of(&show)]);
        }
    }

    /// Whether an object whose mesh has paint (`has_paint`) is drawn with it.
    pub fn draws(&self, has_paint: bool) -> bool {
        drawn_painted(has_paint, self.show[0] > 0.5, self.show[1] > 0.5)
    }

    /// Each drawn object's local table as palette rows: `tables[i]` is
    /// object `i`'s.
    pub fn set_tables(&mut self, gpu: &Gpu, tables: &[[u32; 8]]) {
        if tables.len() > self.tables_room {
            self.tables_room = tables.len().next_power_of_two();
            self.tables = storage(&gpu.device, "paint tables", (self.tables_room * 32) as u64);
            self.bind = None;
        }
        if !tables.is_empty() {
            gpu.queue.write_buffer(&self.tables, 0, bytemuck::cast_slice(tables));
        }
    }

    /// The bind group of group 1.
    pub fn bind(&mut self) -> &wgpu::BindGroup {
        if self.bind.is_none() {
            self.bind = Some(self.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("paint"),
                layout: &self.layout,
                entries: &[
                    wgpu::BindGroupEntry { binding: 0, resource: self.palette.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 1, resource: self.tables.as_entire_binding() },
                    wgpu::BindGroupEntry { binding: 2, resource: self.flags.as_entire_binding() },
                ],
            }));
        }
        self.bind.as_ref().unwrap()
    }

    /// The module key for the palette as it is, less the custom materials
    /// the card has refused.
    pub fn module_key(&self) -> ModuleKey {
        let mut key = ModuleKey::of(&self.upload);
        key.customs.retain(|(_, hash)| !self.refused.contains(hash));
        key
    }

    /// The painted pipeline for `key`, built now if it has not been; `None`
    /// if it cannot be (the object is then drawn without its paint, and the
    /// status line says why).
    pub fn pipeline(&mut self, key: &Key) -> Option<wgpu::RenderPipeline> {
        if let Some(built) = self.pipelines.get(key) {
            return built.clone();
        }
        let module = self.module(&key.module)?;
        let built = match self.build_pipeline(&module, key) {
            Ok(p) => Some(p),
            Err(why) => {
                log::warn!("a painted pipeline was refused: {why}");
                self.refuse(&key.module, "The painted surface could not be drawn");
                None
            }
        };
        self.pipelines.insert(key.clone(), built.clone());
        built
    }

    /// The custom materials of `key` will not do: they are left out from now
    /// on, and the status line says so.
    fn refuse(&mut self, key: &ModuleKey, what: &str) {
        if key.customs.is_empty() {
            self.said.push(format!("{what}; it shows unpainted."));
            return;
        }
        for (id, hash) in &key.customs {
            self.refused.insert(*hash);
            self.said.push(format!(
                "Custom material {id} was refused by the graphics card. It shows as plain colour until its file changes."
            ));
        }
    }

    fn module(&mut self, key: &ModuleKey) -> Option<wgpu::ShaderModule> {
        if let Some(built) = self.modules.get(key) {
            return built.clone();
        }
        let kinds: BTreeSet<Kind> = self.upload.kinds.iter().copied().filter(|k| key.kinds.contains(&(*k as u32))).collect();
        let customs: Vec<Code> =
            self.upload.customs.iter().filter(|c| key.customs.iter().any(|(id, h)| *id == c.id && *h == c.hash)).cloned().collect();
        let source = source(&kinds, &customs);
        let built = validate(&source).and_then(|()| {
            let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
            let module = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("painted"),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            match pollster::block_on(scope.pop()) {
                Some(e) => Err(e.to_string()),
                None => Ok(module),
            }
        });
        let built = match built {
            Ok(module) => Some(module),
            Err(why) => {
                log::warn!("the painted shader was refused: {why}");
                self.refuse(key, "The painted surface's shader could not be made");
                None
            }
        };
        self.modules.insert(key.clone(), built.clone());
        built
    }

    fn build_pipeline(&self, module: &wgpu::ShaderModule, key: &Key) -> Result<wgpu::RenderPipeline, String> {
        let constants = [("FINISH", key.surface as f64), ("GLOW", if key.glow { 1.0 } else { 0.0 })];
        let [a, b, c, d, e] = pipelines::mesh_vertex_buffers();
        let buffers = [a, b, c, d, e, Some(vertex_buffer())];
        let scope = self.device.push_error_scope(wgpu::ErrorFilter::Validation);
        let pipeline = self.device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("painted"),
            layout: Some(&self.pipeline_layout),
            vertex: wgpu::VertexState {
                module,
                entry_point: Some("vs_painted"),
                compilation_options: Default::default(),
                buffers: &buffers,
            },
            primitive: wgpu::PrimitiveState { cull_mode: key.cull.face(), ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState {
                format: self.depth,
                depth_write_enabled: Some(true),
                depth_compare: Some(wgpu::CompareFunction::GreaterEqual),
                stencil: wgpu::StencilState::default(),
                bias: wgpu::DepthBiasState::default(),
            }),
            multisample: wgpu::MultisampleState { count: self.samples, mask: !0, alpha_to_coverage_enabled: false },
            fragment: Some(wgpu::FragmentState {
                module,
                entry_point: Some("fs_painted"),
                compilation_options: wgpu::PipelineCompilationOptions { constants: &constants, ..Default::default() },
                targets: &[Some(self.color.into())],
            }),
            multiview_mask: None,
            cache: None,
        });
        match pollster::block_on(scope.pop()) {
            Some(e) => Err(e.to_string()),
            None => Ok(pipeline),
        }
    }
}

#[cfg(test)]
mod tests;
