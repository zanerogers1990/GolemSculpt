// Viewport shading, painted surfaces: polypaint colour and painted materials
// (render/paint.rs builds these pipelines). Compiled after shader.wgsl and
// sculpt_pbr.wgsl, and before a part made for the palette in use: the
// `eval_slot` switch with a case for each finish the palette has, and the
// custom materials' own functions.
//
// Per vertex the paint brings a colour with its coverage, a roughness and
// a metallic override each with its weight, and the weights of up to eight
// materials: the object's local table, whose palette rows the `tables`
// buffer holds per drawn object. The Look's own finish has what the
// materials leave.
//
// Why dense weights, one per table entry, and not a material number per
// vertex: numbers cannot be blended. Where three materials meet on one
// triangle, interpolating "which material" between its corners would make
// up materials that are at none of them. Each weight is interpolated on
// its own instead, so at any point of a triangle the weights are the
// barycentric mixture of its corners' weights: never negative, never a
// material no corner has, and summing to what the corners' sums give. The
// pixel is then that mixture of the materials' samples (`MaterialSample`
// is linear in every field, sculpt_pbr.wgsl), lit once.

const KIND_PLAIN: u32 = 0u;
const KIND_LURE: u32 = 1u;
const KIND_METAL: u32 = 2u;
const KIND_CLAY: u32 = 3u;
const KIND_WOOD: u32 = 4u;
const KIND_GLOW: u32 = 6u;
const KIND_CUSTOM: u32 = 7u;

// A weight below one step of a byte is no weight: it is not worth a finish
// evaluation, and the vertex data cannot say anything smaller.
const PAINT_EPS: f32 = 0.0039215686;

// One palette entry; mirrors `materials::rows::Row`.
struct PaletteRow {
    // x the finish (KIND_), y tint by paint (0 off, 1 multiply, 2 replace),
    // z tint amount, w which custom material
    head: vec4<f32>,
    // The plain surface's linear colour: a plain material, under a glow.
    base: vec4<f32>,
    // x metallic, y smoothness of that plain surface
    extra: vec4<f32>,
    // The swatch, linear: the matcap's tint, and what shows when a
    // material cannot be drawn.
    swatch: vec4<f32>,
    params: FinishParams,
};

struct PaintFlags {
    // x painted colour shows (Show Polypaint), y painted materials show
    // (Show Materials)
    show: vec4<f32>,
};

@group(1) @binding(0) var<storage, read> palette: array<PaletteRow>;
// Two per drawn object: the palette rows of its local table's eight
// entries (0 = no material).
@group(1) @binding(1) var<storage, read> tables: array<vec4<u32>>;
@group(1) @binding(2) var<uniform> paint_flags: PaintFlags;

struct PaintedIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    @location(7) curv: f32,
    @location(3) model_x: vec4<f32>,
    @location(4) model_y: vec4<f32>,
    @location(5) model_z: vec4<f32>,
    @location(6) model_w: vec4<f32>,
    // sRGB colour and its coverage
    @location(8) colour: vec4<f32>,
    // rough, metal, rough weight, metal weight
    @location(9) rough_metal: vec4<f32>,
    // the eight materials' weights
    @location(10) weights_a: vec4<f32>,
    @location(11) weights_b: vec4<f32>,
    @builtin(instance_index) item: u32,
};

struct PaintedOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    @location(3) local: vec3<f32>,
    @location(4) is_active: f32,
    @location(5) curv: f32,
    @location(6) @interpolate(flat) model_x: vec3<f32>,
    @location(7) @interpolate(flat) model_y: vec3<f32>,
    @location(8) @interpolate(flat) model_z: vec3<f32>,
    // Linear colour times coverage, and coverage: premultiplied, so a
    // painted vertex beside a bare one blends to a half-covered pixel of the
    // painted colour and not to a darker one.
    @location(9) colour: vec4<f32>,
    // rough times its weight, rough weight, metal times its weight, metal
    // weight: premultiplied for the same reason.
    @location(10) rough_metal: vec4<f32>,
    @location(11) weights_a: vec4<f32>,
    @location(12) weights_b: vec4<f32>,
    @location(13) @interpolate(flat) item: u32,
};

fn paint_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@vertex
fn vs_painted(in: PaintedIn) -> PaintedOut {
    var plain: VsIn;
    plain.position = in.position;
    plain.normal = in.normal;
    plain.mask = in.mask;
    plain.curv = in.curv;
    plain.model_x = in.model_x;
    plain.model_y = in.model_y;
    plain.model_z = in.model_z;
    plain.model_w = in.model_w;
    let m = mesh_vertex(plain);
    var out: PaintedOut;
    out.pos = m.pos;
    out.world = m.world;
    out.normal = m.normal;
    out.mask = m.mask;
    out.local = m.local;
    out.is_active = m.is_active;
    out.curv = m.curv;
    out.model_x = m.model_x;
    out.model_y = m.model_y;
    out.model_z = m.model_z;
    out.colour = vec4<f32>(paint_linear(in.colour.rgb) * in.colour.a, in.colour.a);
    let rm = in.rough_metal;
    out.rough_metal = vec4<f32>(rm.x * rm.z, rm.z, rm.y * rm.w, rm.w);
    out.weights_a = in.weights_a;
    out.weights_b = in.weights_b;
    out.item = in.item;
    return out;
}

// The interpolated inputs as the main pass's own, for the shared helpers.
fn plain_out(in: PaintedOut) -> VsOut {
    var out: VsOut;
    out.pos = in.pos;
    out.world = in.world;
    out.normal = in.normal;
    out.mask = in.mask;
    out.local = in.local;
    out.is_active = in.is_active;
    out.curv = in.curv;
    out.model_x = in.model_x;
    out.model_y = in.model_y;
    out.model_z = in.model_z;
    return out;
}

// The paint at a pixel, ready to shade with. The eight materials' weights
// and rows are two vec4s each rather than arrays: an array indexed by a loop
// counter is kept in slow memory on some cards, a vector's lane is picked
// from registers.
struct Paint {
    // Linear colour and how much of it there is.
    colour: vec3<f32>,
    cover: f32,
    rough: f32,
    rough_w: f32,
    metal: f32,
    metal_w: f32,
    // The Look's own finish's share, and each local material's (entries 0
    // to 3, then 4 to 7).
    base: f32,
    w_a: vec4<f32>,
    w_b: vec4<f32>,
    // Each local material's palette row (0 = none).
    rows_a: vec4<u32>,
    rows_b: vec4<u32>,
};

// The paint at this pixel. The weights are made a proper mixture, as
// `resolve_weights` in render/paint.rs (its twin on the CPU, which the
// tests hold to it) does: each clamped to 0..1, any below a byte's step
// or naming no palette row dropped, all scaled down if together they come
// to more than one (bytes rounded at the vertices can), and the Look's
// finish given what is left; a sliver of it below a byte's step goes to
// the materials instead. The result is never negative and sums to one.
fn paint_at(in: PaintedOut) -> Paint {
    var p: Paint;
    let show_colour = paint_flags.show.x > 0.5;
    let show_materials = paint_flags.show.y > 0.5;
    let cover = saturate(in.colour.a);
    p.cover = select(0.0, cover, show_colour && cover > 1e-5);
    p.colour = select(vec3<f32>(0.0), in.colour.rgb / max(in.colour.a, 1e-5), p.cover > 0.0);
    let rm = in.rough_metal;
    p.rough_w = select(0.0, saturate(rm.y), show_materials);
    p.metal_w = select(0.0, saturate(rm.w), show_materials);
    p.rough = select(0.0, saturate(rm.x / max(rm.y, 1e-5)), rm.y > 1e-5);
    p.metal = select(0.0, saturate(rm.z / max(rm.w, 1e-5)), rm.w > 1e-5);

    p.rows_a = tables[in.item * 2u];
    p.rows_b = tables[in.item * 2u + 1u];
    let zero = vec4<f32>(0.0);
    let a = saturate(in.weights_a);
    let b = saturate(in.weights_b);
    var w_a = select(zero, a, (a >= vec4<f32>(PAINT_EPS)) & (p.rows_a != vec4<u32>(0u)));
    var w_b = select(zero, b, (b >= vec4<f32>(PAINT_EPS)) & (p.rows_b != vec4<u32>(0u)));
    if (!show_materials) {
        w_a = zero;
        w_b = zero;
    }
    var total = dot(w_a, vec4<f32>(1.0)) + dot(w_b, vec4<f32>(1.0));
    if (total > 1.0) {
        w_a /= total;
        w_b /= total;
        total = 1.0;
    }
    var base = 1.0 - total;
    if (base < PAINT_EPS) {
        if (total > 0.0) {
            w_a /= total;
            w_b /= total;
        }
        base = 0.0;
    }
    p.w_a = w_a;
    p.w_b = w_b;
    p.base = base;
    return p;
}

// A sample with every field zero, to add weighted samples to.
fn no_sample() -> MaterialSample {
    var m: MaterialSample;
    m.albedo = vec3<f32>(0.0);
    m.metallic = 0.0;
    m.smoothness = 0.0;
    m.occlusion = 0.0;
    m.normal = vec3<f32>(0.0);
    m.emission = vec3<f32>(0.0);
    m.sheen_amount = 0.0;
    m.sheen_power = 0.0;
    m.scatter_color = vec3<f32>(0.0);
    m.scatter_wrap = 0.0;
    m.scatter_shadow = 0.0;
    m.dim = 0.0;
    return m;
}

// `acc` plus `w` of `m`.
fn add_sample(acc: MaterialSample, m: MaterialSample, w: f32) -> MaterialSample {
    var out = acc;
    out.albedo += m.albedo * w;
    out.metallic += m.metallic * w;
    out.smoothness += m.smoothness * w;
    out.occlusion += m.occlusion * w;
    out.normal += m.normal * w;
    out.emission += m.emission * w;
    out.sheen_amount += m.sheen_amount * w;
    // Weighted by its amount too: a material with no sheen has no say in
    // how sharp the others' is (see `evaluate_surface`).
    out.sheen_power += m.sheen_power * m.sheen_amount * w;
    out.scatter_color += m.scatter_color * w;
    out.scatter_wrap += m.scatter_wrap * w;
    out.scatter_shadow += m.scatter_shadow * w;
    out.dim += m.dim * w;
    return out;
}

// A palette row's plain surface (a glow's, a swatch's) as `finish_pbr`
// reads its settings.
fn plain_params(e: PaletteRow) -> FinishParams {
    var p: FinishParams;
    p.v[0] = vec4<f32>(e.extra.x, e.extra.y, 0.0, 0.0);
    return p;
}

// A palette row's plain surface lit as itself: what a glow shines over,
// and what a material that cannot be drawn shows as.
fn plain_of(e: PaletteRow, s: Surface, albedo: vec3<f32>) -> MaterialSample {
    var b = s;
    b.base_color = albedo;
    return finish_pbr(b, plain_params(e));
}

// Palette row `row`'s material at this pixel, tinted by the paint.
fn painted_material(row: u32, s: Surface, p: Paint) -> MaterialSample {
    let e = palette[row];
    var m = eval_slot(row, s);
    let mode = u32(e.head.y + 0.5);
    let amount = saturate(e.head.z) * p.cover;
    if (mode == 1u) {
        m.albedo *= mix(vec3<f32>(1.0), p.colour, amount);
    } else if (mode == 2u) {
        m.albedo = mix(m.albedo, p.colour, amount);
    }
    return m;
}

// What the surface is made of at this pixel: the Look's own finish for its
// share, with the painted colour over it, and each painted material for
// its own, mixed into one sample; then the roughness and metallic painted
// over the lot. The bake hook: a texture bake would call this per texel.
fn evaluate_surface(s: Surface, p: Paint) -> MaterialSample {
    var acc = no_sample();
    if (p.base > 0.0) {
        var m = surface_material(s);
        m.albedo = mix(m.albedo, p.colour, p.cover);
        acc = add_sample(acc, m, p.base);
    }
    for (var k = 0u; k < 8u; k++) {
        let lane = k & 3u;
        let w = select(p.w_b, p.w_a, k < 4u)[lane];
        if (w > 0.0) {
            let row = select(p.rows_b, p.rows_a, vec4<bool>(k < 4u))[lane];
            acc = add_sample(acc, painted_material(row, s, p), w);
        }
    }
    acc.normal = select(s.n, normalize(acc.normal), dot(acc.normal, acc.normal) > 1e-12);
    // The sheen's sharpness as the sheens mixed have it, and never below 1:
    // `light_sample` raises to it, and a power of 0 of a 0 (a light behind
    // the highlight) is not a number on every card. A finish with no sheen
    // leaves 0 there, which a single pipeline's compiler folds away, but in a
    // mixture it is a number worked out per pixel.
    acc.sheen_power = max(acc.sheen_power / max(acc.sheen_amount, 1e-6), 1.0);
    acc.smoothness = mix(acc.smoothness, 1.0 - p.rough, p.rough_w);
    acc.metallic = mix(acc.metallic, p.metal, p.metal_w);
    return acc;
}

// Under the matcap, which has its own lighting: the painted colour
// multiplies it, and each painted material's swatch for its share.
fn matcap_tint(p: Paint) -> vec3<f32> {
    var swatches = vec3<f32>(p.base);
    for (var k = 0u; k < 8u; k++) {
        let lane = k & 3u;
        let w = select(p.w_b, p.w_a, k < 4u)[lane];
        if (w > 0.0) {
            let row = select(p.rows_b, p.rows_a, vec4<bool>(k < 4u))[lane];
            swatches += palette[row].swatch.rgb * w;
        }
    }
    return mix(vec3<f32>(1.0), p.colour, p.cover) * swatches;
}

@fragment
fn fs_painted(in: PaintedOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    let plain = plain_out(in);
    let front = is_front(handedness(in.model_x, in.model_y, in.model_z), facing);
    var s = mesh_surface(plain, front);
    let p = paint_at(in);
    var color: vec3<f32>;
    if (FINISH == FINISH_MATCAP) {
        color = matcap_shade(s.n) * matcap_tint(p);
    } else {
        s = lit_surface(s);
        color = light_sample(s, evaluate_surface(s, p));
    }
    return mesh_finish(color, s, plain, front);
}
