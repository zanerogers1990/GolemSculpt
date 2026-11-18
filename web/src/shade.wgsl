
// ==== shadows and ambient occlusion (WebGPU only) =================================
//
// Joined onto shader.wgsl and finish.wgsl when the browser draws with WebGPU
// (shadows.rs); WebGL2 gets shade_off.wgsl in its place. The key light's
// soft shadow from its map and the occlusion from the passes in
// shade_passes.wgsl, as the desktop's sculpt_pbr.wgsl reads them: the
// finishes take them through the private values `shade_begin` sets.

struct ShadeU {
    shadow_view_proj: mat4x4<f32>,
    shadow_params: vec4<f32>,
    shadow_soft: vec4<f32>,
    flags: vec4<f32>,
    key: vec4<f32>,
    viewport: vec4<f32>,
    ao: vec4<f32>,
    occlusion: vec4<f32>,
    picture: vec4<f32>,
};

@group(2) @binding(0) var<uniform> shade: ShadeU;
@group(2) @binding(1) var shadow_tex: texture_depth_2d;
@group(2) @binding(2) var shadow_cmp: sampler_comparison;
@group(2) @binding(3) var shadow_pt: sampler;
@group(2) @binding(4) var ao_tex: texture_2d<f32>;
@group(2) @binding(5) var ao_smp: sampler;

// What reaches the surface being shaded: the key light (1 lit, 0 in its
// shadow), the open sky (1 all) and what direct light keeps of it, and which
// way the open sky lies (world space).
var<private> shade_shadow: f32 = 1.0;
var<private> shade_ao: f32 = 1.0;
var<private> shade_ao_direct: f32 = 1.0;
var<private> shade_bent: vec3<f32>;

const SHADOW_TAPS: i32 = 16;
const CASTER_TAPS: i32 = 8;
const GOLDEN_TURN: mat2x2<f32> = mat2x2<f32>(-0.7373688, 0.6754903, -0.6754903, -0.7373688);

fn shadow_uv(at: vec2<f32>) -> vec2<f32> {
    return clamp(at, vec2<f32>(0.0), vec2<f32>(1.0)) * shade.shadow_soft.w;
}

fn shade_noise(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// How much of the key light reaches `world` (the desktop's
// `key_light_reaching`): crisp where the shadow meets what casts it, softer
// the farther that is (percentage-closer soft shadows).
fn key_light_reaching(world: vec3<f32>, n: vec3<f32>, pixel: vec2<f32>) -> f32 {
    let texel = shade.shadow_params.x;
    let world_per_uv = shade.shadow_soft.x;
    let world_per_depth = shade.shadow_soft.y;
    let light_tan = shade.shadow_soft.z;
    let l = shade.key.xyz;
    let lift = world_per_uv * texel * (1.0 + 2.0 * (1.0 - saturate(dot(n, l))));
    let clip = shade.shadow_view_proj * vec4<f32>(world + n * lift, 1.0);
    let pos = vec3<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5, clip.z);
    if (pos.z <= 0.0 || pos.z >= 1.0 || any(pos.xy <= vec2<f32>(0.0)) || any(pos.xy >= vec2<f32>(1.0))) {
        return 1.0;
    }
    let turn = shade_noise(pixel) * TAU;
    let start = vec2<f32>(cos(turn), sin(turn));
    let reach = max(pos.z - 0.25, 0.0) * world_per_depth * light_tan / world_per_uv;
    let search = clamp(reach, texel * 2.0, 0.08);
    let moving = shade.shadow_params.w > 0.5;
    let caster_taps = select(CASTER_TAPS, CASTER_TAPS / 2, moving);
    let filter_taps = select(SHADOW_TAPS, SHADOW_TAPS / 2, moving);
    var found = 0.0;
    var caster_depth = 0.0;
    var way = start;
    for (var i = 0; i < caster_taps; i++) {
        let r = sqrt((f32(i) + 0.5) / f32(caster_taps));
        let d = textureGather(shadow_tex, shadow_pt, shadow_uv(pos.xy + way * (r * search)));
        way = GOLDEN_TURN * way;
        let blocked = select(vec4<f32>(0.0), vec4<f32>(1.0), d < vec4<f32>(pos.z));
        found += dot(blocked, vec4<f32>(1.0));
        caster_depth += dot(d, blocked);
    }
    if (found < 0.5) {
        return 1.0;
    }
    if (found > f32(caster_taps * 4) - 0.5) {
        return 0.0;
    }
    let gap = (pos.z - caster_depth / found) * world_per_depth;
    let radius = max(gap * light_tan / world_per_uv, texel * 1.5);
    var lit = 0.0;
    way = vec2<f32>(-start.y, start.x);
    for (var i = 0; i < filter_taps; i++) {
        let r = sqrt((f32(i) + 0.5) / f32(filter_taps));
        lit += textureSampleCompareLevel(shadow_tex, shadow_cmp, shadow_uv(pos.xy + way * (r * radius)), pos.z);
        way = GOLDEN_TURN * way;
    }
    return lit / f32(filter_taps);
}

// Looks up what reaches the surface at `pixel` (on the picture being drawn),
// `world`, whose smooth normal is `smooth_n`. As the desktop's `lit_surface`.
fn shade_begin(pixel: vec2<f32>, world: vec3<f32>, smooth_n: vec3<f32>) {
    shade_bent = smooth_n;
    if (shade.flags.x > 0.5) {
        shade_shadow = mix(1.0, key_light_reaching(world, smooth_n, pixel), shade.flags.y);
    }
    if (shade.flags.z > 0.5) {
        let o = textureSampleLevel(ao_tex, ao_smp, pixel / max(shade.picture.xy, vec2<f32>(1.0)), 0.0);
        shade_ao = o.a;
        shade_ao_direct = mix(1.0, o.a, shade.flags.w);
        // Back from the occlusion's view space (z forward) to the world.
        let b = o.rgb * 2.0 - 1.0;
        let to_world = transpose(mat3x3<f32>(g.view[0].xyz, g.view[1].xyz, g.view[2].xyz));
        let bent = to_world * vec3<f32>(b.x, b.y, -b.z);
        if (dot(bent, bent) > 0.01) {
            let unit = normalize(bent);
            shade_bent = select(unit, -unit, dot(unit, smooth_n) < -0.5);
        }
    }
}
