//! Ray tracing for Render Picture (Phase 4 of docs/renderer-plan.md): the
//! graphics card's own ray tracing, through wgpu's ray queries, which are
//! still marked experimental. Nothing here runs while sculpting: only a
//! render with Ray Traced on, on a card that has them, traces rays; any
//! other card renders pictures as before.
//!
//! **What is traced.** The picture is still drawn as the viewport draws it,
//! every finish and painted material as they are. One pass before the
//! shading (ray.wgsl) starts from each pixel of the normals prepass and
//! traces, against the model itself rather than what the screen shows of
//! it:
//! - a ray to a point of each light, picked anew each sample: soft shadows
//!   from every light, as wide as the softbox or the light's size makes them,
//!   in place of the key's shadow map and the contact shadows;
//! - a ray over the surface toward the sky: the sky's light where it meets
//!   nothing, and the light bounced off the model (or the floor) where it
//!   does, in place of the screen's ambient occlusion;
//! - a ray along the reflection, spread as the surface is rough: what of the
//!   model or the floor the reflection meets, in place of guessing how much
//!   of the surroundings a crease hides;
//! - with resin in the picture, the view into it, across it and out of its
//!   far side, bent both ways, and what it meets beyond.
//!
//! A sample traces one ray of each kind per pixel, each sample a different
//! one, so the picture's refinement (refine.rs) averages them: a render of
//! many samples is a clean one.
//!
//! **What the rays meet.** One acceleration structure per mesh, built from
//! the very buffers it is drawn from (the faces that show), and one over the
//! objects as they are placed. A ray that meets the model sees each object
//! as one colour, metallic and smooth as its finish is on the whole
//! (`hit_material`): bounced light and the model's reflection in itself are
//! made of that, not of the finish's patterns or paint.

use std::collections::HashMap;

use bytemuck::{Pod, Zeroable};

use super::{DrawItem, Gpu, MeshBuffers};
use crate::look::{self, Finish, Look};

/// What ray tracing asks of the card: rays traced from shaders, and the
/// corners of the triangle a ray met (for its normal).
pub const FEATURES: wgpu::Features =
    wgpu::Features::EXPERIMENTAL_RAY_QUERY.union(wgpu::Features::EXPERIMENTAL_RAY_HIT_VERTEX_RETURN);

/// Whether to ask `adapter` for ray tracing: it has it, and it has not been
/// turned off (`SCULPT_NO_RAYS`, should a driver misbehave with it).
pub fn wanted_by(adapter: &wgpu::Adapter) -> bool {
    adapter.features().contains(FEATURES) && std::env::var_os("SCULPT_NO_RAYS").is_none()
}

/// The ray pass's shader: the surface shader's inputs and functions, then
/// ray.wgsl, with the ray query extensions enabled ahead of them all.
pub fn shader_source() -> String {
    let mut out = String::from("enable wgpu_ray_query;\nenable wgpu_ray_query_vertex_return;\n");
    out.push_str(include_str!("shader.wgsl"));
    out.push_str(include_str!("sculpt_pbr.wgsl"));
    out.push_str(include_str!("ray.wgsl"));
    out
}

/// What a ray that meets an object sees it made of; `HitMaterial` in ray.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct HitMaterial {
    /// rgb colour (linear), a metallic.
    pub color: [f32; 4],
    /// x smoothness.
    pub params: [f32; 4],
}

/// The model's surface on the whole, as the rays meet it: the finish's
/// colour (the base colour for plain lit shading), how metallic and how
/// smooth, from its preset and the settings that scale them. `base` is the
/// base colour (linear) and smoothness, as `Globals::base_color` has them.
pub fn hit_material(look: &Look, base: [f32; 4]) -> HitMaterial {
    let mix = |a: look::Rgb, b: look::Rgb| [(a[0] + b[0]) * 0.5, (a[1] + b[1]) * 0.5, (a[2] + b[2]) * 0.5];
    let pick = |i: usize, len: usize| i.min(len - 1);
    let (color, metallic, smoothness) = match look.finish {
        Finish::None => ([base[0], base[1], base[2]], look.metallic, base[3]),
        Finish::LurePlastic => {
            let p = &look::LURES[pick(look.lure.preset, look::LURES.len())];
            (look::linear(mix(p.thin, p.thick)), 0.0, 0.9 * look.lure.gloss)
        }
        Finish::Metal => {
            let p = &look::METALS[pick(look.metal.preset, look::METALS.len())];
            (look::linear(p.metal), p.metal_metallic, p.metal_smoothness * look.metal.gloss)
        }
        Finish::Clay => {
            let p = &look::CLAYS[pick(look.clay.preset, look::CLAYS.len())];
            (look::linear(p.color), 0.0, p.smoothness * look.clay.gloss)
        }
        Finish::Wood => {
            let p = &look::WOODS[pick(look.wood.preset, look::WOODS.len())];
            (look::linear(mix(p.early, p.late)), 0.0, p.smoothness)
        }
        Finish::Resin => {
            // Light mostly passes through: as a surface, a dim glossy tint.
            let p = &look::RESINS[pick(look.resin.preset, look::RESINS.len())];
            let c = look::linear(p.color);
            ([c[0] * 0.5, c[1] * 0.5, c[2] * 0.5], 0.0, p.smoothness * look.resin.gloss)
        }
    };
    HitMaterial {
        color: [color[0], color[1], color[2], metallic.clamp(0.0, 1.0)],
        params: [smoothness.clamp(0.0, 1.0), 0.0, 0.0, 0.0],
    }
}

/// The pass's own settings; `TraceParams` in ray.wgsl.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
pub struct TraceParams {
    /// Clip space back to the world, for the view as this sample draws it.
    pub unproject: [[f32; 4]; 4],
    /// x the sample, y 1 when the view through resin is traced, z how far a
    /// ray reaches, w the roughness reflections are traced with (GGX alpha).
    pub params: [f32; 4],
}

/// How far a ray reaches: past anything a scene holds.
const REACH: f32 = 1.0e6;

/// What the pass writes, at the picture's size, for the shading to read.
pub struct Outputs {
    pub size: (u32, u32),
    /// Whether lights 0 to 3 reach each pixel.
    pub lights: wgpu::TextureView,
    /// The light from over the surface, and whether light 4 reaches it.
    pub diffuse: wgpu::TextureView,
    /// What the reflection met.
    pub specular: wgpu::TextureView,
    /// What is seen through resin, and across how much of it.
    pub through: wgpu::TextureView,
}

impl Outputs {
    pub fn new(gpu: &Gpu, size: (u32, u32)) -> Outputs {
        let make = |label, format| {
            gpu.device
                .create_texture(&wgpu::TextureDescriptor {
                    label: Some(label),
                    size: wgpu::Extent3d { width: size.0.max(1), height: size.1.max(1), depth_or_array_layers: 1 },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format,
                    usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                })
                .create_view(&Default::default())
        };
        Outputs {
            size,
            lights: make("ray lights", wgpu::TextureFormat::Rgba8Unorm),
            diffuse: make("ray diffuse", wgpu::TextureFormat::Rgba16Float),
            specular: make("ray specular", wgpu::TextureFormat::Rgba16Float),
            through: make("ray through", wgpu::TextureFormat::Rgba16Float),
        }
    }
}

/// What the pass reads of the renderer's own: the shared uniforms, the
/// normals prepass, and the surroundings.
pub struct Inputs<'a> {
    pub globals: &'a wgpu::Buffer,
    pub look: &'a wgpu::Buffer,
    pub gbuf: &'a wgpu::TextureView,
    pub env: &'a wgpu::TextureView,
    pub env_sampler: &'a wgpu::Sampler,
    pub hdri: &'a wgpu::TextureView,
    pub hdri_sampler: &'a wgpu::Sampler,
}

/// One mesh built for tracing, and the size it was built at.
struct Built {
    blas: wgpu::Blas,
    size: wgpu::BlasTriangleGeometrySizeDescriptor,
}

/// The ray pass, and the scene as the rays meet it.
pub struct Rays {
    pipeline: wgpu::ComputePipeline,
    inputs: wgpu::BindGroupLayout,
    outputs: wgpu::BindGroupLayout,
    params: wgpu::Buffer,
    materials: wgpu::Buffer,
    materials_room: usize,
    /// Each mesh in the picture, built from its buffers as they were at the
    /// renderer's upload count `built_from`; all built again once any mesh
    /// is sent again.
    meshes: HashMap<u64, Built>,
    built_from: u64,
    /// The objects as placed, and its room for objects; what it was built from.
    tlas: Option<(wgpu::Tlas, usize)>,
    placed: Vec<u8>,
}

impl Rays {
    /// The pass, if the card takes its shader: a driver refusing it leaves
    /// renders as they are without rays, never an error the app stops on.
    pub fn new(gpu: &Gpu) -> Option<Rays> {
        let device = &gpu.device;
        let source = shader_source();
        if let Err(why) = validate(&source) {
            log::warn!("the ray tracing shader was refused: {why}");
            return None;
        }
        let scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let compute = wgpu::ShaderStages::COMPUTE;
        let uniform = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let texture = |binding, sample_type, view_dimension| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Texture { sample_type, view_dimension, multisampled: false },
            count: None,
        };
        let sampler = |binding| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
            count: None,
        };
        let written = |binding, format| wgpu::BindGroupLayoutEntry {
            binding,
            visibility: compute,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
            count: None,
        };
        let smooth = wgpu::TextureSampleType::Float { filterable: true };
        let exact = wgpu::TextureSampleType::Float { filterable: false };
        let flat = wgpu::TextureViewDimension::D2;
        // The surface shader's own binding numbers, for what this reads of it.
        let inputs = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray inputs"),
            entries: &[
                uniform(0),
                texture(3, exact, flat),
                uniform(4),
                texture(5, smooth, wgpu::TextureViewDimension::Cube),
                sampler(6),
                texture(11, smooth, flat),
                sampler(12),
            ],
        });
        let outputs = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("ray outputs"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: compute,
                    ty: wgpu::BindingType::AccelerationStructure { vertex_return: true },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: compute,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                uniform(2),
                written(3, wgpu::TextureFormat::Rgba8Unorm),
                written(4, wgpu::TextureFormat::Rgba16Float),
                written(5, wgpu::TextureFormat::Rgba16Float),
                written(6, wgpu::TextureFormat::Rgba16Float),
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ray tracing"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("ray tracing"),
            bind_group_layouts: &[Some(&inputs), Some(&outputs)],
            immediate_size: 0,
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("ray tracing"),
            layout: Some(&layout),
            module: &module,
            entry_point: Some("cs_trace"),
            compilation_options: Default::default(),
            cache: None,
        });
        if let Some(e) = pollster::block_on(scope.pop()) {
            log::warn!("the ray tracing pass could not be made: {e}");
            return None;
        }
        let buffer = |label, size, usage| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        let room = 16;
        Some(Rays {
            pipeline,
            inputs,
            outputs,
            params: buffer("ray params", std::mem::size_of::<TraceParams>() as u64, wgpu::BufferUsages::UNIFORM),
            materials: buffer(
                "ray materials",
                (room * std::mem::size_of::<HitMaterial>()) as u64,
                wgpu::BufferUsages::STORAGE,
            ),
            materials_room: room,
            meshes: HashMap::new(),
            built_from: u64::MAX,
            tlas: None,
            placed: Vec::new(),
        })
    }

    /// Builds what the rays meet for `items` (what of it is not built
    /// already), each object seen as `material`, into `encoder` ahead of the
    /// pass. `uploads` is the renderer's upload count, which moves whenever
    /// any mesh is sent again. Gives back whether there is anything to meet.
    pub fn prepare(
        &mut self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        meshes: &HashMap<u64, MeshBuffers>,
        items: &[DrawItem],
        uploads: u64,
        material: HitMaterial,
    ) -> bool {
        let device = &gpu.device;
        if self.built_from != uploads {
            self.meshes.clear();
            self.placed.clear();
            self.built_from = uploads;
        }
        let flags = wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
            | wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN;
        let update_mode = wgpu::AccelerationStructureUpdateMode::Build;
        // Each mesh not built yet: the faces that show, as they are drawn.
        let mut fresh = Vec::new();
        for item in items {
            let Some(mesh) = meshes.get(&item.mesh) else { continue };
            if self.meshes.contains_key(&item.mesh) || mesh.index_count < 3 {
                continue;
            }
            let size = wgpu::BlasTriangleGeometrySizeDescriptor {
                vertex_format: wgpu::VertexFormat::Float32x3,
                vertex_count: mesh.vertex_room,
                index_format: Some(wgpu::IndexFormat::Uint32),
                index_count: Some(mesh.index_count),
                flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
            };
            let blas = device.create_blas(
                &wgpu::CreateBlasDescriptor { label: Some("mesh rays"), flags, update_mode },
                wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: vec![size.clone()] },
            );
            self.meshes.insert(item.mesh, Built { blas, size });
            fresh.push(item.mesh);
        }

        // The objects as placed, built again when they move or change.
        let traced: Vec<(usize, &DrawItem)> =
            items.iter().enumerate().filter(|(_, i)| self.meshes.contains_key(&i.mesh)).collect();
        if traced.is_empty() {
            return false;
        }
        let mut placed = uploads.to_le_bytes().to_vec();
        for item in items {
            placed.extend_from_slice(&item.mesh.to_le_bytes());
            placed.extend_from_slice(bytemuck::bytes_of(&item.model));
        }
        if placed != self.placed || !fresh.is_empty() {
            if self.tlas.as_ref().is_none_or(|(_, room)| *room < traced.len()) {
                let room = traced.len().next_power_of_two().max(16);
                let tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
                    label: Some("objects rays"),
                    max_instances: room as u32,
                    flags,
                    update_mode,
                });
                self.tlas = Some((tlas, room));
            }
            let (tlas, room) = self.tlas.as_mut().unwrap();
            for slot in 0..*room {
                tlas[slot] = traced.get(slot).map(|(index, item)| {
                    let m = item.model;
                    // The object's matrix (stored by columns) by rows, less its last.
                    let transform = [
                        m[0][0], m[1][0], m[2][0], m[3][0], m[0][1], m[1][1], m[2][1], m[3][1], m[0][2], m[1][2],
                        m[2][2], m[3][2],
                    ];
                    wgpu::TlasInstance::new(&self.meshes[&item.mesh].blas, transform, *index as u32, 0xFF)
                });
            }
            let builds: Vec<wgpu::BlasBuildEntry> = fresh
                .iter()
                .map(|id| {
                    let (built, mesh) = (&self.meshes[id], &meshes[id]);
                    wgpu::BlasBuildEntry {
                        blas: &built.blas,
                        geometry: wgpu::BlasGeometries::TriangleGeometries(vec![wgpu::BlasTriangleGeometry {
                            size: &built.size,
                            vertex_buffer: &mesh.positions,
                            first_vertex: 0,
                            vertex_stride: 12,
                            index_buffer: Some(&mesh.indices),
                            first_index: Some(0),
                            transform_buffer: None,
                            transform_buffer_offset: None,
                        }]),
                    }
                })
                .collect();
            encoder.build_acceleration_structures(builds.iter(), std::iter::once(&self.tlas.as_ref().unwrap().0));
            self.placed = placed;
        }

        // What each object is made of, by its place among `items`.
        if self.materials_room < items.len() {
            self.materials_room = items.len().next_power_of_two();
            self.materials = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("ray materials"),
                size: (self.materials_room * std::mem::size_of::<HitMaterial>()) as u64,
                usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
        }
        let table = vec![material; items.len()];
        gpu.queue.write_buffer(&self.materials, 0, bytemuck::cast_slice(&table));
        true
    }

    /// Traces the picture's rays into `outputs`, after `prepare`.
    pub fn trace(
        &self,
        gpu: &Gpu,
        encoder: &mut wgpu::CommandEncoder,
        inputs: &Inputs,
        outputs: &Outputs,
        params: &TraceParams,
    ) {
        let Some((tlas, _)) = &self.tlas else { return };
        let device = &gpu.device;
        gpu.queue.write_buffer(&self.params, 0, bytemuck::bytes_of(params));
        let view = wgpu::BindingResource::TextureView;
        let sampler = wgpu::BindingResource::Sampler;
        let read = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray inputs"),
            layout: &self.inputs,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: inputs.globals.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: view(inputs.gbuf) },
                wgpu::BindGroupEntry { binding: 4, resource: inputs.look.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 5, resource: view(inputs.env) },
                wgpu::BindGroupEntry { binding: 6, resource: sampler(inputs.env_sampler) },
                wgpu::BindGroupEntry { binding: 11, resource: view(inputs.hdri) },
                wgpu::BindGroupEntry { binding: 12, resource: sampler(inputs.hdri_sampler) },
            ],
        });
        let write = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("ray outputs"),
            layout: &self.outputs,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: tlas.as_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: self.materials.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 2, resource: self.params.as_entire_binding() },
                wgpu::BindGroupEntry { binding: 3, resource: view(&outputs.lights) },
                wgpu::BindGroupEntry { binding: 4, resource: view(&outputs.diffuse) },
                wgpu::BindGroupEntry { binding: 5, resource: view(&outputs.specular) },
                wgpu::BindGroupEntry { binding: 6, resource: view(&outputs.through) },
            ],
        });
        let mut pass =
            encoder.begin_compute_pass(&wgpu::ComputePassDescriptor { label: Some("ray trace"), timestamp_writes: None });
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &read, &[]);
        pass.set_bind_group(1, &write, &[]);
        pass.dispatch_workgroups(outputs.size.0.div_ceil(8), outputs.size.1.div_ceil(8), 1);
    }

    /// Lets go of what the rays met, once a render is done with it: a dense
    /// model's acceleration structures take a good deal of the card's memory.
    pub fn release(&mut self) {
        self.meshes.clear();
        self.tlas = None;
        self.placed.clear();
        self.built_from = u64::MAX;
    }
}

/// The pass's settings for a sample: `view_proj` the view as the sample
/// draws it (jittered), `sample` its number, `through` whether resin is seen
/// through, `material` what the model is made of.
pub fn params(view_proj: &[[f32; 4]; 4], sample: u32, through: bool, material: &HitMaterial) -> TraceParams {
    let unproject = glam::Mat4::from_cols_array_2d(view_proj).inverse();
    let roughness = (1.0 - material.params[0]).clamp(0.0, 1.0);
    TraceParams {
        unproject: unproject.to_cols_array_2d(),
        params: [sample as f32, if through { 1.0 } else { 0.0 }, REACH, (roughness * roughness).max(0.002)],
    }
}

/// Checks the ray shader with naga as the card would, without the card.
pub fn validate(source: &str) -> Result<(), String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
    let capabilities = naga::valid::Capabilities::RAY_QUERY | naga::valid::Capabilities::RAY_HIT_VERTEX_POSITION;
    naga::valid::Validator::new(naga::valid::ValidationFlags::all(), capabilities)
        .validate(&module)
        .map_err(|e| e.emit_to_string(source))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use wgpu::util::DeviceExt;

    const WANTED: wgpu::Features = super::FEATURES;

    #[test]
    fn the_ray_shader_is_valid() {
        if let Err(why) = super::validate(&super::shader_source()) {
            panic!("{why}");
        }
    }

    /// The fastest card's device with ray tracing on, if it has it.
    fn ray_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            ..Default::default()
        }))
        .ok()?;
        let info = adapter.get_info();
        println!("{} ({:?}): {:?}", info.name, info.backend, adapter.features() & WANTED);
        if !adapter.features().contains(WANTED) {
            return None;
        }
        pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            required_features: WANTED,
            // The default limits allow no acceleration structures at all.
            required_limits: adapter.limits(),
            // SAFETY: the experimental features are asked for knowingly; this
            // test is there to find out whether they work on this card.
            experimental_features: unsafe { wgpu::ExperimentalFeatures::enabled() },
            ..Default::default()
        }))
        .ok()
    }

    const PROBE: &str = r#"
enable wgpu_ray_query;
enable wgpu_ray_query_vertex_return;

@group(0) @binding(0) var scene: acceleration_structure<vertex_return>;
@group(0) @binding(1) var<storage, read_write> found: array<vec4<f32>>;

// A ray straight down +z from (x, 0, -1): the triangle spans x -1..1 at z = 2.
fn trace(x: f32) -> vec4<f32> {
    var q: ray_query<vertex_return>;
    rayQueryInitialize(&q, scene, RayDesc(RAY_FLAG_NONE, 0xFFu, 0.0, 100.0, vec3<f32>(x, 0.0, -1.0), vec3<f32>(0.0, 0.0, 1.0)));
    while (rayQueryProceed(&q)) {}
    let hit = rayQueryGetCommittedIntersection(&q);
    if (hit.kind != RAY_QUERY_INTERSECTION_TRIANGLE) {
        return vec4<f32>(-1.0);
    }
    let corners = getCommittedHitVertexPositions(&q);
    return vec4<f32>(hit.t, f32(hit.primitive_index), f32(hit.instance_custom_data), corners[1].x);
}

@compute @workgroup_size(1)
fn cs(@builtin(global_invocation_id) id: vec3<u32>) {
    found[id.x] = trace(f32(id.x) * 3.0);
}

@vertex
fn vs(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let uv = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    return vec4<f32>(uv * 2.0 - 1.0, 0.0, 1.0);
}

@fragment
fn fs(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return trace(floor(pos.x) * 3.0);
}
"#;

    /// Whether this card traces rays as the renderer will: a one-triangle
    /// scene built, and rays sent from a compute shader and from a fragment
    /// shader, one hitting and one missing. Needs the card, so it is run by
    /// name: `cargo test --release ray::tests -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn rays_hit_a_triangle_from_compute_and_fragment_shaders() {
        let Some((device, queue)) = ray_device() else {
            println!("no ray tracing on this card");
            return;
        };
        let input = wgpu::BufferUsages::BLAS_INPUT;
        let corners: [f32; 9] = [-1.0, -1.0, 2.0, 1.0, -1.0, 2.0, 0.0, 1.0, 2.0];
        let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&corners),
            usage: input,
        });
        let indices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: None,
            contents: bytemuck::cast_slice(&[0u32, 1, 2]),
            usage: input,
        });
        let size = wgpu::BlasTriangleGeometrySizeDescriptor {
            vertex_format: wgpu::VertexFormat::Float32x3,
            vertex_count: 3,
            index_format: Some(wgpu::IndexFormat::Uint32),
            index_count: Some(3),
            flags: wgpu::AccelerationStructureGeometryFlags::OPAQUE,
        };
        let flags = wgpu::AccelerationStructureFlags::PREFER_FAST_TRACE
            | wgpu::AccelerationStructureFlags::ALLOW_RAY_HIT_VERTEX_RETURN;
        let blas = device.create_blas(
            &wgpu::CreateBlasDescriptor { label: None, flags, update_mode: wgpu::AccelerationStructureUpdateMode::Build },
            wgpu::BlasGeometrySizeDescriptors::Triangles { descriptors: vec![size.clone()] },
        );
        let mut tlas = device.create_tlas(&wgpu::CreateTlasDescriptor {
            label: None,
            max_instances: 1,
            flags,
            update_mode: wgpu::AccelerationStructureUpdateMode::Build,
        });
        let identity = [1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0];
        tlas[0] = Some(wgpu::TlasInstance::new(&blas, identity, 7, 0xFF));

        let found = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 32,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: None,
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::AccelerationStructure { vertex_return: true },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: None,
            source: wgpu::ShaderSource::Wgsl(PROBE.into()),
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: None,
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let compute = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            module: &module,
            entry_point: Some("cs"),
            compilation_options: Default::default(),
            cache: None,
        });
        let target = device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let render = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: None,
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &module,
                entry_point: Some("vs"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &module,
                entry_point: Some("fs"),
                targets: &[Some(wgpu::TextureFormat::Rgba32Float.into())],
                compilation_options: Default::default(),
            }),
            primitive: Default::default(),
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });

        let mut encoder = device.create_command_encoder(&Default::default());
        let geometry = wgpu::BlasTriangleGeometry {
            size: &size,
            vertex_buffer: &vertices,
            first_vertex: 0,
            vertex_stride: 12,
            index_buffer: Some(&indices),
            first_index: Some(0),
            transform_buffer: None,
            transform_buffer_offset: None,
        };
        let build = wgpu::BlasBuildEntry { blas: &blas, geometry: wgpu::BlasGeometries::TriangleGeometries(vec![geometry]) };
        encoder.build_acceleration_structures(std::iter::once(&build), std::iter::once(&tlas));
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout: &layout,
            entries: &[
                wgpu::BindGroupEntry { binding: 0, resource: tlas.as_binding() },
                wgpu::BindGroupEntry { binding: 1, resource: found.as_entire_binding() },
            ],
        });
        {
            let mut pass = encoder.begin_compute_pass(&Default::default());
            pass.set_pipeline(&compute);
            pass.set_bind_group(0, &bind, &[]);
            pass.dispatch_workgroups(2, 1, 1);
        }
        let view = target.create_view(&Default::default());
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: None,
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations { load: wgpu::LoadOp::Clear(wgpu::Color::BLACK), store: wgpu::StoreOp::Store },
                })],
                ..Default::default()
            });
            pass.set_pipeline(&render);
            pass.set_bind_group(0, &bind, &[]);
            pass.draw(0..3, 0..1);
        }
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: 256 + 32,
            usage: wgpu::BufferUsages::MAP_READ | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        encoder.copy_buffer_to_buffer(&found, 0, &readback, 0, Some(32));
        encoder.copy_texture_to_buffer(
            target.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout { offset: 256, bytes_per_row: Some(256), rows_per_image: None },
            },
            wgpu::Extent3d { width: 2, height: 1, depth_or_array_layers: 1 },
        );
        queue.submit([encoder.finish()]);
        readback.map_async(wgpu::MapMode::Read, .., |_| {});
        device.poll(wgpu::PollType::Wait { submission_index: None, timeout: None }).unwrap();
        let data = readback.get_mapped_range(..).expect("mapped");
        let values: &[f32] = bytemuck::cast_slice(&data);
        let (computed, drawn) = (&values[..8], &values[64..72]);
        println!("compute: {computed:?}\nfragment: {drawn:?}");
        for got in [computed, drawn] {
            // The first ray hits the triangle 3 along, triangle 0 of the
            // instance marked 7, whose second corner is at x = 1.
            assert!((got[0] - 3.0).abs() < 1e-4 && got[1] == 0.0 && got[2] == 7.0 && got[3] == 1.0, "{got:?}");
            // The second misses.
            assert_eq!(&got[4..8], &[-1.0; 4]);
        }
    }
}
