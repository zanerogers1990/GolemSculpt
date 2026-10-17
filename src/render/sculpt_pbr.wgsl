// Viewport shading, second half: the surface itself. A port of the Unity
// app's SculptPBR shader - physically based lighting under the preset rig,
// matcaps, the lure plastic / aged metal / clay / wood finishes and the glow
// layer - on top of the inputs and helpers in shader.wgsl.
//
// Every finish is a function of one shape:
//
//     fn finish_x(s: Surface, params: FinishParams) -> MaterialSample
//
// `Surface` is what the pixel is (where, which way it faces, how it is lit
// by the key's shadow and the occlusion), `FinishParams` the finish's own
// settings, sixteen vec4s each finish reads in its own way (packed by the
// matching `Look::pack_*` in look.rs), and `MaterialSample` what the
// surface is made of there: colour, metal, gloss, the bent normal, light it
// gives off, and the extra lobes clay and wood use. One `light_sample` then
// lights any sample. Every field of a sample is a plain number to be lit
// later, so two samples blended by weight are a meaningful mixture of two
// materials: that is what painting materials onto a model will build on.
//
// The finish is fixed per pipeline (`FINISH`, and `GLOW` for the glow layer
// over it): one is compiled for each, so plain PBR and the matcap do not
// carry the registers the plastic's flake tracing needs.

const MAX_LIGHTS: i32 = 5;
const TAU: f32 = 6.2831853;

const FINISH_PBR: u32 = 0u;
const FINISH_PLASTIC: u32 = 1u;
const FINISH_METAL: u32 = 2u;
const FINISH_CLAY: u32 = 3u;
const FINISH_WOOD: u32 = 4u;
const FINISH_MATCAP: u32 = 5u;
const FINISH_RESIN: u32 = 6u;

// Which surface this pipeline shades (one of the FINISH_ values).
override FINISH: u32 = 0u;
// Whether the glow layer goes over it.
override GLOW: bool = false;

// A finish's own settings: sixteen vec4s, each finish reading them its own
// way (see each finish_ function and the matching Look::pack_ in look.rs).
struct FinishParams {
    v: array<vec4<f32>, 16>,
};

struct Look {
    // World direction toward each light, and its linear colour times intensity.
    light_dir: array<vec4<f32>, 5>,
    light_color: array<vec4<f32>, 5>,
    // x count, y shadows on, z shadow strength, w time in seconds
    light_params: vec4<f32>,
    shadow_view_proj: mat4x4<f32>,
    // x one shadow texel in uv, y depth bias, z normal bias (world units),
    // w 1 while the shadow changes frame after frame (half the taps then)
    shadow_params: vec4<f32>,
    // x the map's width in world units (world per uv), y world units per unit
    // of map depth, z tangent of the key light's angular radius, w the part
    // of the texture the map fills (less than 1 while it is drawn small)
    shadow_soft: vec4<f32>,
    // Ambient light as nine spherical harmonic terms, ready to evaluate.
    sh: array<vec4<f32>, 9>,
    ambient_sky: vec4<f32>,
    ambient_equator: vec4<f32>,
    ambient_ground: vec4<f32>,
    // x ambient from sh (1) or sky / equator / ground (0), y reflection
    // strength, z ambient occlusion on, w its strength on direct light
    env_params: vec4<f32>,
    // x orthographic half height (0 = perspective)
    cam_params: vec4<f32>,
    // x backdrop (0 flat, 1 gradient, 2 HDRI), y HDRI turn in radians,
    // z HDRI exposure, w gradient bias
    bg_params: vec4<f32>,
    // z normal detail strength, w normal detail scale
    base_params: vec4<f32>,
    // x intensity, y tint by base colour, z finish (0 none, 1 plastic,
    // 2 metal, 3 clay, 4 wood; the pipeline is picked by it), w back-face
    // depth buffer is valid
    matcap_params: vec4<f32>,
    mask_tint: vec4<f32>,
    // The finish's settings, and the glow's (glow.v[2].x is 1 while it is on).
    finish: FinishParams,
    glow: FinishParams,
    // The floor (render/floor.rs): x 1 when it shows, y its height, z how far
    // it reaches from its middle, w how dark shadows on it get
    floor: vec4<f32>,
    // xy (world x and z) its middle, z how strongly it reflects, w how blurred
    floor_more: vec4<f32>,
    // Each light as a softbox (render/ltc.rs): its middle, w 1 for a softbox
    // (0 for a plain direction)...
    light_pos: array<vec4<f32>, 5>,
    // ...half its width as a vector, w the radiance that gives the light's
    // intensity at the subject (pi over its solid angle)...
    light_side: array<vec4<f32>, 5>,
    // ...and half its height, w its distance from the subject squared.
    light_up: array<vec4<f32>, 5>,
    // x how far the contact shadows march toward each light, as a fraction of
    // the view's height at the surface's depth (0 for none)
    contact: vec4<f32>,
};

@group(0) @binding(4) var<uniform> look: Look;
// The surroundings, blurrier in each smaller level, for reflections.
@group(0) @binding(5) var env_cube: texture_cube<f32>;
@group(0) @binding(6) var env_smp: sampler;
@group(0) @binding(7) var shadow_tex: texture_depth_2d;
@group(0) @binding(8) var shadow_smp: sampler_comparison;
// The ambient occlusion (post.wgsl): the bent normal in rgb (view space, z
// forward, as 0.5 + 0.5 n) and how open in alpha (1 = open).
@group(0) @binding(9) var ao_tex: texture_2d<f32>;
// Eye depth of the nearest back face under each pixel, 0 = none.
@group(0) @binding(10) var back_tex: texture_2d<f32>;
@group(0) @binding(11) var hdri_tex: texture_2d<f32>;
@group(0) @binding(12) var hdri_smp: sampler;
// The split-sum reflection table (render/dfg.rs): view angle across,
// roughness down; x what grazing fresnel adds, y all a white mirror reflects.
@group(0) @binding(13) var dfg_tex: texture_2d<f32>;
// The model mirrored in the floor, at half size (render/floor.rs).
@group(0) @binding(14) var floor_tex: texture_2d<f32>;
// The softboxes' fitted highlight matrices (render/ltc.rs): roughness
// across, sqrt(1 - cos) of the view angle down.
@group(0) @binding(15) var ltc_tex: texture_2d<f32>;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126729, 0.7151522, 0.0721750));
}

// ---- background -----------------------------------------------------------

// Where in an equirectangular picture a world direction lands, with the
// picture turned about the vertical by `turn`.
fn hdri_uv(dir: vec3<f32>, turn: f32) -> vec2<f32> {
    let v = dir;
    let s = sin(turn);
    let c = cos(turn);
    let d = vec3<f32>(c * v.x - s * v.z, v.y, s * v.x + c * v.z);
    return vec2<f32>(atan2(d.z, d.x) / TAU + 0.5, 0.5 - asin(clamp(d.y, -1.0, 1.0)) / 3.14159265);
}

@fragment
fn fs_background(in: BgOut) -> @location(0) vec4<f32> {
    var dir = g.cam_forward.xyz;
    if (g.viewport.z > 0.0) {
        dir = normalize(g.cam_forward.xyz
            + g.cam_right.xyz * (in.ndc.x * g.viewport.z * g.viewport.w)
            + g.cam_up.xyz * (in.ndc.y * g.viewport.z));
    }
    let mode = look.bg_params.x;
    var color = g.bg_bottom.rgb;
    if (mode > 1.5) {
        let uv = hdri_uv(dir, look.bg_params.y);
        // u wraps along one vertical line, where the derivative is enormous
        // and would collapse that column to the smallest level: unwrap it.
        var dx = dpdx(uv);
        var dy = dpdy(uv);
        dx.x = fract(dx.x + 0.5) - 0.5;
        dy.x = fract(dy.x + 0.5) - 0.5;
        color = textureSampleGrad(hdri_tex, hdri_smp, uv, dx, dy).rgb * look.bg_params.z;
    } else if (mode > 0.5) {
        // Sky-dome gradient by view direction, so it stays put as you orbit.
        var t = in.ndc.y * 0.5 + 0.5;
        if (g.viewport.z > 0.0) {
            t = clamp(dir.y * 0.5 + 0.5, 0.0, 1.0);
        }
        t = pow(t, 1.0 / max(look.bg_params.w, 0.001));
        color = mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
    }
    // Alpha 0 marks the backdrop (the model is drawn with 1): a backdrop
    // colour is shown as it was picked, outside the tone curve (post.wgsl).
    return vec4<f32>(color, 0.0);
}

// ---- noise ----------------------------------------------------------------

fn hash13(p_in: vec3<f32>) -> f32 {
    var p = fract(p_in * 0.1031);
    p += dot(p, p.yzx + 33.33);
    return fract((p.x + p.y) * p.z);
}

fn hash33(p_in: vec3<f32>) -> vec3<f32> {
    var p = fract(p_in * vec3<f32>(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return fract((p.xxy + p.yxx) * p.zyx);
}

fn value_noise(p: vec3<f32>) -> f32 {
    let i = floor(p);
    var f = fract(p);
    f = f * f * (3.0 - 2.0 * f);
    let n000 = hash13(i + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = hash13(i + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash13(i + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash13(i + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash13(i + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash13(i + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash13(i + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash13(i + vec3<f32>(1.0, 1.0, 1.0));
    let nxy0 = mix(mix(n000, n100, f.x), mix(n010, n110, f.x), f.y);
    let nxy1 = mix(mix(n001, n101, f.x), mix(n011, n111, f.x), f.y);
    return mix(nxy0, nxy1, f.z);
}

// Three octaves of value noise, stretched to roughly fill 0..1.
fn fbm(p: vec3<f32>) -> f32 {
    var sum = value_noise(p) * 0.5;
    sum += value_noise(p * 2.03 + 17.1) * 0.25;
    sum += value_noise(p * 4.11 + 31.7) * 0.125;
    return saturate((sum / 0.875 - 0.5) * 1.8 + 0.5);
}

// Bumps a normal by the gradient of value noise at p, `strength` deep.
fn perturb_normal(normal: vec3<f32>, p: vec3<f32>, strength: f32) -> vec3<f32> {
    let e = 0.05;
    let h0 = value_noise(p);
    let grad = vec3<f32>(
        value_noise(p + vec3<f32>(e, 0.0, 0.0)) - h0,
        value_noise(p + vec3<f32>(0.0, e, 0.0)) - h0,
        value_noise(p + vec3<f32>(0.0, 0.0, e)) - h0) / e;
    let n = normalize(normal);
    return normalize(n - (grad - n * dot(grad, n)) * strength);
}

// ---- lighting -------------------------------------------------------------

// What a finish reads of the pixel it shades.
struct Surface {
    world: vec3<f32>,
    // Toward the eye.
    v: vec3<f32>,
    pixel: vec2<i32>,
    // Position in the object's own space at world size, and how much of it
    // one pixel covers: patterns live here so they stay on the model.
    p: vec3<f32>,
    footprint: f32,
    // How much p changes to the next pixel right and down: what dpdx(p) and
    // dpdy(p) would say, worked out where they mean something. A finish may
    // run inside a branch chosen pixel by pixel (a painted material), where
    // screen derivatives give garbage, so finishes use these instead (see
    // `surface_width`).
    p_dx: vec3<f32>,
    p_dy: vec3<f32>,
    scale: vec3<f32>,
    // The object's axes (its matrix by columns).
    mx: vec3<f32>,
    my: vec3<f32>,
    mz: vec3<f32>,
    // Smooth normal (never the faceted one), and the normal to shade with
    // (faceted under flat shading, with the surface grain in it).
    smooth_n: vec3<f32>,
    n: vec3<f32>,
    // Recess (above 0.5) or ridge (below), from the mesh's own curvature.
    curv: f32,
    // Light reaching the surface from the key: 1 lit, 0 in its shadow.
    shadow: f32,
    // Each light's contact shadow (lights 0 to 3, then 4): 1 lit.
    contact: vec4<f32>,
    contact_last: f32,
    // Ambient occlusion (1 = open), and what direct light keeps of it.
    ao: f32,
    ao_direct: f32,
    // Which way the open sky lies (the occlusion's bent normal, world
    // space): the smooth normal where nothing closes it in.
    bent: vec3<f32>,
    // The model's colour here, before any finish: the base colour (later,
    // whatever is painted on).
    base_color: vec3<f32>,
    // Eye depth of the nearest back face under this pixel, 0 for none (or
    // when no back faces were drawn this frame).
    back_depth: f32,
};

// What a finish makes of a pixel, to be lit by `light_sample`. Every field
// is linear, so a weighted blend of two samples is a blend of two materials.
struct MaterialSample {
    albedo: vec3<f32>,
    metallic: f32,
    smoothness: f32,
    // How much of the ambient and reflected light reaches it (1 = all).
    occlusion: f32,
    // The normal it is lit with, bumped by the finish's own detail.
    normal: vec3<f32>,
    // Light added after the lighting: what the finish works out lit by
    // itself (glitter, light through the plastic) and what it gives off.
    emission: vec3<f32>,
    // A second, sharper highlight over the first: clay's wet film, wood's
    // wax. Amount (0 = none) and the power of its lobe.
    sheen_amount: f32,
    sheen_power: f32,
    // Light scattered inside reaching past the shadow line: its colour and
    // amount (0 = none), how far round it wraps (0..1), and how much of the
    // key's shadow it keeps (0 none, 1 all).
    scatter_color: vec3<f32>,
    scatter_wrap: f32,
    scatter_shadow: f32,
    // What the lit result is multiplied by (recesses that see less of the
    // lights); the emission is added after it.
    dim: f32,
};

// A sample of nothing but `albedo`, for a finish to fill in.
fn material(s: Surface, albedo: vec3<f32>) -> MaterialSample {
    var m: MaterialSample;
    m.albedo = albedo;
    m.metallic = 0.0;
    m.smoothness = 0.0;
    m.occlusion = 1.0;
    m.normal = s.n;
    m.emission = vec3<f32>(0.0);
    m.sheen_amount = 0.0;
    m.sheen_power = 0.0;
    m.scatter_color = vec3<f32>(0.0);
    m.scatter_wrap = 0.0;
    m.scatter_shadow = 0.0;
    m.dim = 1.0;
    return m;
}

// What `fwidth` of a pattern would be at this pixel: how much a value with
// gradient `grad` (per unit of p, the object's own space at world size)
// changes across one pixel, from the pixel's own steps in p.
fn surface_width(s: Surface, grad: vec3<f32>) -> f32 {
    return abs(dot(grad, s.p_dx)) + abs(dot(grad, s.p_dy));
}

fn to_world_dir(s: Surface, v: vec3<f32>) -> vec3<f32> {
    return normalize(s.mx * v.x + s.my * v.y + s.mz * v.z);
}

// A world direction in the object's space (not renormalised).
fn to_object_dir(s: Surface, v: vec3<f32>) -> vec3<f32> {
    let cx = cross(s.my, s.mz);
    let cy = cross(s.mz, s.mx);
    let cz = cross(s.mx, s.my);
    let det = dot(s.mx, cx);
    return vec3<f32>(dot(cx, v), dot(cy, v), dot(cz, v)) / det;
}

fn to_object_normal(s: Surface, n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(s.mx, n), dot(s.my, n), dot(s.mz, n));
}

// A number in 0..1 that changes from pixel to pixel without a pattern the
// eye picks out (Jimenez's interleaved gradient noise), moved on for each
// sample of a picture being refined, so every sample turns its filters a
// different way and their average is smooth.
fn pixel_noise(pixel: vec2<f32>) -> f32 {
    let p = pixel + 5.588238 * look.cam_params.z;
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

// The soft shadow's taps go round Vogel's spiral, which spreads any number
// of points evenly over a disc: each is a golden angle on from the one
// before, at the radius that gives it an equal share of the area. Sixteen
// for the filter, eight for finding the casters (each of those reads four
// texels at once); half as many while the shadow changes every frame, when
// it costs what the fixed filter before it did.
const SHADOW_TAPS: i32 = 16;
const CASTER_TAPS: i32 = 8;

// Where a point of the shadow map (0..1 across it) is in its texture: the
// map fills only the texture's corner while it is drawn small, and a tap
// off its edge reads the edge.
fn shadow_uv(at: vec2<f32>) -> vec2<f32> {
    return clamp(at, vec2<f32>(0.0), vec2<f32>(1.0)) * look.shadow_soft.w;
}
// A turn by the golden angle (2.39996 radians).
const GOLDEN_TURN: mat2x2<f32> = mat2x2<f32>(-0.7373688, 0.6754903, -0.6754903, -0.7373688);

// The key light's shadow at `world`, as dark as the preset lets it get.
fn key_shadow(world: vec3<f32>, n: vec3<f32>, pixel: vec2<f32>) -> f32 {
    return mix(1.0, key_light_reaching(world, n, pixel), look.light_params.z);
}

// How much of the key light reaches `world`, whose surface faces `n` (1 all,
// 0 none): a shadow crisp where it meets what casts it and softer the
// farther that is, as under a real light of the preset's size
// (percentage-closer soft shadows). The casters near this point are found
// first and their average distance sets how wide the filter is; `pixel`
// turns the filter's taps.
fn key_light_reaching(world: vec3<f32>, n: vec3<f32>, pixel: vec2<f32>) -> f32 {
    if (look.light_params.y < 0.5) {
        return 1.0;
    }
    let texel = look.shadow_params.x;
    let world_per_uv = look.shadow_soft.x;
    let world_per_depth = look.shadow_soft.y;
    let light_tan = look.shadow_soft.z;
    // A step off the surface along its normal, more where it turns from the
    // light: the wider filter reaches across a slope, which must not shadow
    // itself.
    let l = look.light_dir[0].xyz;
    let lift = world_per_uv * texel * (1.0 + 2.0 * (1.0 - saturate(dot(n, l))));
    let clip = look.shadow_view_proj * vec4<f32>(world + n * lift, 1.0);
    let pos = vec3<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5, clip.z);
    if (pos.z <= 0.0 || pos.z >= 1.0 || any(pos.xy <= vec2<f32>(0.0)) || any(pos.xy >= vec2<f32>(1.0))) {
        return 1.0;
    }
    let turn = pixel_noise(pixel) * TAU;
    let start = vec2<f32>(cos(turn), sin(turn));

    // The casters: over the widest the shadow could spread here, which is
    // from a caster at the near side of the scene (a quarter of the map's
    // depth). Each tap gathers the four texels around it.
    let reach = max(pos.z - 0.25, 0.0) * world_per_depth * light_tan / world_per_uv;
    let search = clamp(reach, texel * 2.0, 0.08);
    let moving = look.shadow_params.w > 0.5;
    let caster_taps = select(CASTER_TAPS, CASTER_TAPS / 2, moving);
    let filter_taps = select(SHADOW_TAPS, SHADOW_TAPS / 2, moving);
    var found = 0.0;
    var caster_depth = 0.0;
    var way = start;
    for (var i = 0; i < caster_taps; i++) {
        let r = sqrt((f32(i) + 0.5) / f32(caster_taps));
        let d = textureGather(shadow_tex, env_smp, shadow_uv(pos.xy + way * (r * search)));
        way = GOLDEN_TURN * way;
        let blocked = select(vec4<f32>(0.0), vec4<f32>(1.0), d < vec4<f32>(pos.z));
        found += dot(blocked, vec4<f32>(1.0));
        caster_depth += dot(d, blocked);
    }
    if (found < 0.5) {
        return 1.0;
    }
    if (found > f32(caster_taps * 4) - 0.5) {
        // Everything around is in shadow, wider than the edge could be.
        return 0.0;
    }
    // How wide the shadow's edge is at this distance from its casters.
    let gap = (pos.z - caster_depth / found) * world_per_depth;
    let radius = max(gap * light_tan / world_per_uv, texel * 1.5);
    var lit = 0.0;
    way = vec2<f32>(-start.y, start.x);
    for (var i = 0; i < filter_taps; i++) {
        let r = sqrt((f32(i) + 0.5) / f32(filter_taps));
        lit += textureSampleCompareLevel(shadow_tex, shadow_smp, shadow_uv(pos.xy + way * (r * radius)), pos.z);
        way = GOLDEN_TURN * way;
    }
    return lit / f32(filter_taps);
}

struct Light {
    dir: vec3<f32>,
    color: vec3<f32>,
    shadow: f32,
};

fn get_light(i: i32, s: Surface) -> Light {
    var l: Light;
    l.dir = look.light_dir[i].xyz;
    l.color = look.light_color[i].rgb;
    // Only the key casts a shadow from its map; every light has its contact
    // shadow.
    let contact = select(s.contact_last, s.contact[min(i, 3)], i < 4);
    l.shadow = select(1.0, s.shadow, i == 0) * contact;
    return l;
}

fn light_count() -> i32 {
    return min(i32(look.light_params.x + 0.5), MAX_LIGHTS);
}

// Diffuse light arriving from everywhere but the lights.
fn ambient(n: vec3<f32>) -> vec3<f32> {
    if (look.env_params.x > 0.5) {
        let c = look.sh[0].rgb
            + look.sh[1].rgb * n.y + look.sh[2].rgb * n.z + look.sh[3].rgb * n.x
            + look.sh[4].rgb * (n.x * n.y) + look.sh[5].rgb * (n.y * n.z)
            + look.sh[6].rgb * (3.0 * n.z * n.z - 1.0) + look.sh[7].rgb * (n.x * n.z)
            + look.sh[8].rgb * (n.x * n.x - n.y * n.y);
        return max(c, vec3<f32>(0.0));
    }
    // Sky from above, equator from the sides, ground from below: an even
    // equator light plus a soft light from straight up and one from straight
    // down, each as a low-order harmonic lobe.
    let up = (0.09375 + 0.5 * n.y + 0.46875 * n.y * n.y) * (16.0 / 17.0);
    let down = (0.09375 - 0.5 * n.y + 0.46875 * n.y * n.y) * (16.0 / 17.0);
    let eq = look.ambient_equator.rgb;
    return max(eq + (look.ambient_sky.rgb - eq) * up + (look.ambient_ground.rgb - eq) * down, vec3<f32>(0.0));
}

// The surroundings mirrored in a surface of the given roughness: the cube,
// each smaller level blurred for a rougher surface. A near-mirror under an
// HDRI reads the picture itself instead, as sharp as the reflection is
// (the cube holds it only at its own size), so chrome and gloss show the
// HDRI crisply.
fn environment(r: vec3<f32>, perceptual_roughness: f32) -> vec3<f32> {
    let level = perceptual_roughness * (1.7 - 0.7 * perceptual_roughness) * 6.0;
    var color = textureSampleLevel(env_cube, env_smp, r, level).rgb;
    if (look.env_params.x > 0.5 && level < 1.5) {
        // How wide the reflection's lobe is, or else the angle one pixel
        // spans (times a little, for a curved surface spreading it): read a
        // level of the picture as fine as that. No screen derivatives: a
        // painted material may be shaded in a branch of its own.
        let a = perceptual_roughness * perceptual_roughness;
        let pixel = select(2.0 * g.viewport.z / max(g.viewport.y, 1.0), 0.002, g.viewport.z <= 0.0) * 3.0;
        let width = f32(textureDimensions(hdri_tex).x);
        let lod = max(log2(max(a, pixel) * width / TAU), 0.0);
        let sharp = textureSampleLevel(hdri_tex, hdri_smp, hdri_uv(r, look.bg_params.y), lod).rgb * look.bg_params.z;
        color = mix(sharp, color, saturate(level / 1.5));
    }
    return color * look.env_params.y;
}

fn pow5(x: f32) -> f32 {
    let x2 = x * x;
    return x2 * x2 * x;
}

// What the split-sum table holds for a surface seen at `nv` with this
// perceptual roughness (see render/dfg.rs), read between its texel centres.
fn dfg_lookup(nv: f32, perceptual_roughness: f32) -> vec2<f32> {
    let size = f32(textureDimensions(dfg_tex).x);
    let uv = vec2<f32>(nv, perceptual_roughness) * ((size - 1.0) / size) + 0.5 / size;
    return textureSampleLevel(dfg_tex, env_smp, uv, 0.0).xy;
}

// The GGX spread of microfacet normals, for linear roughness `a`.
fn d_ggx(nh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = (nh * a2 - nh) * nh + 1.0;
    return a2 / (3.14159265 * d * d);
}

// Height-correlated Smith visibility for GGX: how much of the light the
// microfacets shadow and mask, with the 1 / (4 nl nv) of the highlight in it.
fn v_smith(nv: f32, nl: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = nl * sqrt(nv * nv * (1.0 - a2) + a2);
    let gl = nv * sqrt(nl * nl * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-6);
}

// How much of the reflected surroundings a crease still sees (Lagarde's fit
// from the ambient occlusion): a smooth surface's narrow reflection is
// blocked less than the open sky above it, a rough one's about as much.
fn specular_occlusion(nv: f32, ao: f32, a: f32) -> f32 {
    return saturate(pow(nv + ao, exp2(-16.0 * a - 1.0)) - 1.0 + ao);
}

// Whether the reflection points where the sky is open (Jimenez 2016): the
// open sky as a cone round the bent normal, as wide as the openness says,
// against the reflection's lobe as a cone round the reflected view; how
// much the two overlap (Unreal's approximation), 1 when one holds the other.
fn reflection_in_open_sky(open: f32, bent: vec3<f32>, r: vec3<f32>, a: f32) -> f32 {
    let sky = acos(sqrt(saturate(1.0 - open)));
    let lobe = clamp(a * 1.2, 0.05, 1.5707963);
    let between = acos(clamp(dot(bent, r), -1.0, 1.0));
    let difference = abs(sky - lobe);
    return smoothstep(0.0, 1.0, 1.0 - saturate((between - difference) / max(sky + lobe - difference, 1e-4)));
}

// ---- softboxes ------------------------------------------------------------
//
// Rectangle lights by linearly transformed cosines (render/ltc.rs): the
// highlight for this roughness and view angle is a cosine lobe put through a
// matrix, so its integral over a rectangle is a cosine's over the rectangle
// put through the inverse, which has a closed form edge by edge.

// The inverse lobe matrix for this view angle and roughness.
fn ltc_inverse(nv: f32, perceptual_roughness: f32) -> mat3x3<f32> {
    let size = f32(textureDimensions(ltc_tex).x);
    let uv = vec2<f32>(perceptual_roughness, sqrt(max(1.0 - nv, 0.0))) * ((size - 1.0) / size) + 0.5 / size;
    let t = textureSampleLevel(ltc_tex, env_smp, uv, 0.0);
    return mat3x3<f32>(vec3<f32>(t.x, 0.0, t.y), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(t.z, 0.0, t.w));
}

// One edge's part of the integral, from unit vector a to b: the angle it
// spans over its sine times their cross product's height. The angle over
// its sine is Hill's fit (steadier than acos near 1), which comes divided by
// 2 pi: that is put back.
fn ltc_edge(a: vec3<f32>, b: vec3<f32>) -> f32 {
    let x = dot(a, b);
    let y = abs(x);
    let p = 0.8543985 + (0.4965155 + 0.0145206 * y) * y;
    let q = 3.4175940 + (4.1616724 + y) * y;
    let v = p / q;
    let angle_over_sine = select(0.5 * inverseSqrt(max(1.0 - x * x, 1e-7)) - v, v, x > 0.0) * TAU;
    return cross(a, b).z * angle_over_sine;
}

// Walking round the light's outline clipped to above the surface, one
// corner at a time, adding up the edges as they come (no array of corners).
struct LtcWalk {
    first: vec3<f32>,
    last: vec3<f32>,
    started: bool,
    sum: f32,
};

fn ltc_add(w: LtcWalk, corner: vec3<f32>) -> LtcWalk {
    var out = w;
    let q = normalize(corner);
    if (w.started) {
        out.sum += ltc_edge(w.last, q);
    } else {
        out.first = q;
        out.started = true;
    }
    out.last = q;
    return out;
}

// The corner a if it is above the surface, and where the edge to b crosses
// the surface if it does.
fn ltc_side(w: LtcWalk, a: vec3<f32>, b: vec3<f32>) -> LtcWalk {
    var out = w;
    if (a.z > 0.0) {
        out = ltc_add(out, a);
    }
    if ((a.z > 0.0) != (b.z > 0.0)) {
        let above = select(b, a, a.z > 0.0);
        let below = select(a, b, a.z > 0.0);
        out = ltc_add(out, -below.z * above + above.z * below);
    }
    return out;
}

// The integral, times 2 pi, of the lobe `minv` undoes over the rectangle
// `c` +- `side` +- `up`, for a surface at `p` facing `n` seen from `v`. 0
// from behind the light. The GPU twin of `ltc::integral`.
fn ltc_integral(n: vec3<f32>, v: vec3<f32>, p: vec3<f32>, minv: mat3x3<f32>, c: vec3<f32>, side: vec3<f32>, up: vec3<f32>) -> f32 {
    var t1 = v - n * dot(v, n);
    if (dot(t1, t1) < 1e-10) {
        t1 = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(n.x) > 0.9);
        t1 -= n * dot(t1, n);
    }
    t1 = normalize(t1);
    let m = minv * transpose(mat3x3<f32>(t1, cross(n, t1), n));
    let l0 = m * (c - side - up - p);
    let l1 = m * (c + side - up - p);
    let l2 = m * (c + side + up - p);
    let l3 = m * (c - side + up - p);
    var w: LtcWalk;
    w.started = false;
    w.sum = 0.0;
    w = ltc_side(w, l0, l1);
    w = ltc_side(w, l1, l2);
    w = ltc_side(w, l2, l3);
    w = ltc_side(w, l3, l0);
    if (!w.started) {
        return 0.0;
    }
    return max(w.sum + ltc_edge(w.last, w.first), 0.0);
}

// A softbox's highlight by its representative point (Karis): the point of
// it nearest the reflection of the view, lit as a plain light with the lobe
// widened by how big the softbox looks and its peak lowered to keep the
// energy. For frames that change every frame; `ltc_integral` otherwise.
fn softbox_glint(s: Surface, n: vec3<f32>, nv: f32, a: f32, f0: vec3<f32>, energy: vec3<f32>,
                 c: vec3<f32>, side: vec3<f32>, up: vec3<f32>, far: f32) -> vec3<f32> {
    let r = reflect(-s.v, n);
    let facing = cross(side, up);
    let along = dot(r, facing);
    var q = c;
    let t = dot(c - s.world, facing) / select(along, 1e-6, abs(along) < 1e-6);
    if (t > 0.0) {
        let hit = s.world + r * t - c;
        q = c + side * clamp(dot(hit, side) / dot(side, side), -1.0, 1.0)
            + up * clamp(dot(hit, up) / dot(up, up), -1.0, 1.0);
    }
    let l = normalize(q - s.world);
    let nl = saturate(dot(n, l));
    let h = normalize(l + s.v);
    let nh = saturate(dot(n, h));
    let lh = saturate(dot(l, h));
    let size = sqrt(length(side) * length(up));
    let wide = saturate(a + size / (2.0 * sqrt(far)));
    let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow5(1.0 - lh);
    let keep = (a / wide) * (a / wide);
    return fresnel * (d_ggx(nh, wide) * v_smith(nv, nl, wide) * 3.14159265 * keep * nl) * energy;
}

// Physically based shading under the whole rig: ambient from the sky
// colours or the HDRI, the surroundings reflected through the split-sum
// table with the second bounce of rough surfaces given back, and each light
// through a GGX highlight with height-correlated visibility and Schlick's
// fresnel. Lights carry pi in their colour, as in the Unity pipeline this
// came from, so the highlight is the BRDF times pi and diffuse has no 1 / pi.
fn physically_shade(s: Surface, n: vec3<f32>, albedo: vec3<f32>, metallic: f32, smoothness: f32, occlusion: f32) -> vec3<f32> {
    let diffuse = albedo * (1.0 - metallic);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let perceptual_roughness = saturate(1.0 - smoothness);
    let a = max(perceptual_roughness * perceptual_roughness, 0.0078125);
    let nv = max(dot(n, s.v), 1e-4);
    let dfg = dfg_lookup(nv, perceptual_roughness);
    // What the surroundings give back, all of the lobe, and the factor that
    // returns what bounces between microfacets more than once.
    let reflected = mix(vec3<f32>(dfg.x), vec3<f32>(dfg.y), f0);
    let energy = 1.0 + f0 * (1.0 / max(dfg.y, 1e-3) - 1.0);

    // Indirect. A bumped normal can reflect a direction below the surface's
    // real horizon, which it could not see: that fades out.
    let r = reflect(-s.v, n);
    let horizon = min(1.0 + dot(r, s.smooth_n), 1.0);
    let ao = min(s.ao, occlusion);
    let reflection = environment(r, perceptual_roughness);
    // Ambient light from where the sky is open, the surface's own detail
    // kept on it; reflections dimmed as much as the crease hides them and
    // more where they point into it.
    let sky = normalize(n + (s.bent - s.smooth_n));
    var hidden = specular_occlusion(nv, ao, a);
    let moving = look.shadow_params.w > 0.5;
    if (!moving && look.env_params.z > 0.5) {
        // The finer test, once the picture holds still: how much of the lobe
        // the open sky holds, against how much a surface with nothing round
        // it would see (the table already counts what falls below its
        // horizon, so an open surface keeps all of its reflection).
        let unhidden = reflection_in_open_sky(1.0, s.smooth_n, r, a);
        hidden *= saturate(reflection_in_open_sky(s.ao, s.bent, r, a) / max(unhidden, 1e-3));
    }
    var color = ambient(sky) * diffuse * (1.0 - reflected) * ao
        + reflection * reflected * energy * (hidden * horizon * horizon);

    // Direct. Diffuse keeps what the surface does not reflect at normal
    // incidence (the Unity pipeline's 0.96 for a dielectric).
    let direct_diffuse = diffuse * (1.0 - 0.04);
    // The softboxes' lobe, read only when their highlights are integrated.
    var lobe = mat3x3<f32>(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), vec3<f32>(0.0, 0.0, 1.0));
    if (!moving) {
        lobe = ltc_inverse(nv, perceptual_roughness);
    }
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, s);
        if (look.light_pos[i].w > 0.5) {
            // A softbox. Diffuse light from its middle, falling off with
            // distance as it does (as near the integral over its face as
            // makes no difference at these sizes, for half the work); the
            // highlight integrated over its face, at the radiance that gives
            // its intensity, unless all of it is behind the surface.
            let c = look.light_pos[i].xyz;
            let side = look.light_side[i].xyz;
            let up = look.light_up[i].xyz;
            let to_light = c - s.world;
            let far = dot(to_light, to_light);
            let falloff = look.light_up[i].w / max(far, 1e-12);
            var lit = direct_diffuse * (saturate(dot(n, to_light) * inverseSqrt(far)) * falloff);
            let height = dot(n, to_light);
            let reach = abs(dot(n, side)) + abs(dot(n, up));
            if (moving) {
                // While the picture changes every frame (an orbit, a stroke):
                // the highlight of the softbox's point nearest the reflected
                // view, the lobe widened by the softbox's size (Karis). Close
                // to the integral, at a plain light's cost.
                lit += softbox_glint(s, n, nv, a, f0, energy, c, side, up, far) * falloff;
            } else if (height + reach > 0.0) {
                let spec = ltc_integral(n, s.v, s.world, lobe, c, side, up);
                lit += reflected * energy * (spec * look.light_side[i].w / TAU);
            }
            color += lit * light.color * (s.ao_direct * light.shadow);
            continue;
        }
        let nl = saturate(dot(n, light.dir));
        let h = normalize(light.dir + s.v);
        let nh = saturate(dot(n, h));
        let lh = saturate(dot(light.dir, h));
        let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow5(1.0 - lh);
        let spec = fresnel * (d_ggx(nh, a) * v_smith(nv, nl, a) * 3.14159265) * energy;
        color += (direct_diffuse + spec) * light.color * (s.ao_direct * light.shadow * nl);
    }
    return color;
}

// Lights a material sample: the physically based model, then the sheen and
// the scattered light where the sample has them, all dimmed by `dim`, and
// what the sample gives off on top.
fn light_sample(s: Surface, m: MaterialSample) -> vec3<f32> {
    var color = physically_shade(s, m.normal, m.albedo, m.metallic, m.smoothness, m.occlusion);
    if (m.sheen_amount != 0.0 || any(m.scatter_color != vec3<f32>(0.0))) {
        var sheen = vec3<f32>(0.0);
        var scatter = vec3<f32>(0.0);
        let count = light_count();
        for (var i = 0; i < count; i++) {
            let light = get_light(i, s);
            let ndl = dot(m.normal, light.dir);
            // A sharp lobe that keeps its energy, with a dielectric's fresnel:
            // a film of wet or wax, lit on the bumped normal so it breaks up.
            let h = normalize(light.dir + s.v);
            let spec = pow(saturate(dot(m.normal, h)), m.sheen_power) * (m.sheen_power + 8.0) / 8.0;
            let fresnel = 0.04 + 0.96 * pow(1.0 - saturate(dot(s.v, h)), 5.0);
            sheen += light.color * light.shadow * (spec * fresnel * saturate(ndl));
            // Scattered light gets a little past the terminator: the extra a
            // wrapped Lambert gets over the plain one.
            let wrapped = saturate((ndl + m.scatter_wrap) / (1.0 + m.scatter_wrap));
            scatter += light.color * mix(1.0, light.shadow, m.scatter_shadow) * max(wrapped - saturate(ndl), 0.0);
        }
        color += sheen * m.sheen_amount + scatter * m.scatter_color;
    }
    return color * m.dim + m.emission;
}

// ---- plain ----------------------------------------------------------------

// The model's own colour under the lights. v[0]: x metallic, y smoothness.
fn finish_pbr(s: Surface, params: FinishParams) -> MaterialSample {
    var m = material(s, s.base_color);
    m.metallic = params.v[0].x;
    m.smoothness = params.v[0].y;
    return m;
}

// ---- matcap ---------------------------------------------------------------

// A photo of a sphere shaded exactly how the surface should look, indexed by
// the normal in view space - which pins the lighting to the camera.
fn matcap_shade(n: vec3<f32>) -> vec3<f32> {
    let vn = normalize((g.view * vec4<f32>(n, 0.0)).xyz);
    let uv = vec2<f32>(vn.x * 0.5 + 0.5, 0.5 - vn.y * 0.5);
    // From the size of matcap that suits how fast the normal turns from one
    // pixel to the next, so fine detail far away does not sparkle.
    var color = textureSampleGrad(matcap_tex, matcap_smp, uv, dpdx(uv), dpdy(uv)).rgb * look.matcap_params.x;
    color *= mix(vec3<f32>(1.0), g.base_color.rgb, look.matcap_params.y);
    return color;
}

// ---- lure plastic ---------------------------------------------------------
//
// Soft-plastic fishing lures are a coloured, translucent plastisol with
// glitter cast into it. Thin parts are lighter and more saturated than the
// thick body, light shines through them, and the flakes sit at different
// depths, catch the light one by one and dim through the plastic above them.

// Rows of a rotation that takes the flake lattice off the model's axes.
const FLAKE_R0: vec3<f32> = vec3<f32>(0.36, 0.48, -0.80);
const FLAKE_R1: vec3<f32> = vec3<f32>(-0.80, 0.60, 0.00);
const FLAKE_R2: vec3<f32> = vec3<f32>(0.48, 0.64, 0.60);

fn flake_rot(v: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(FLAKE_R0, v), dot(FLAKE_R1, v), dot(FLAKE_R2, v));
}

fn flake_unrot(v: vec3<f32>) -> vec3<f32> {
    return FLAKE_R0 * v.x + FLAKE_R1 * v.y + FLAKE_R2 * v.z;
}

struct FlakeHit {
    // World distance below the surface, along the refracted ray.
    depth: f32,
    coverage: f32,
    color: vec3<f32>,
    normal: vec3<f32>,
};

// Once a pixel spans more than about half a cell the flakes cannot be
// resolved, and what is left is moire: fade them out there.
fn flake_lod_fade(footprint_in_cells: f32) -> f32 {
    return saturate(2.0 - 2.5 * footprint_in_cells);
}

// What the flakes a layer's fade has removed still add on average: their
// colour, faintly, so glitter too small to draw still tints the plastic.
fn faded_flake_tint(body: vec3<f32>, flake_color: vec3<f32>, footprint: f32, cell_size: f32, density: f32) -> vec3<f32> {
    if (cell_size <= 1e-6) {
        return body;
    }
    let gone = 1.0 - flake_lod_fade(footprint / cell_size);
    return mix(body, flake_color, gone * density * 0.12);
}

// Glitter is a flat platelet (hex or square) lying roughly along the skin.
// Each lattice cell holds at most one, kept inside its cell, so a short
// march along the refracted view ray tests only the cell it stands in. The
// ray meets the platelet's plane, so a flake shows its whole face at its
// true depth and slides with parallax as the model turns.
// `shape` blends hex (0) to square (1) platelets and `tilt` sets how far they
// turn from the skin. With `palette` each flake takes one of the colours
// `pal_a`, `pal_b` and `pal_c` by the weights in their alpha; without, all
// are `pal_a`.
fn trace_flakes(s: Surface, origin: vec3<f32>, dir: vec3<f32>, normal: vec3<f32>,
                cell_size: f32, density: f32, seed: f32, shape: f32, tilt: f32, palette: bool,
                pal_a: vec4<f32>, pal_b: vec4<f32>, pal_c: vec4<f32>) -> FlakeHit {
    var hit: FlakeHit;
    hit.depth = 1e20;
    hit.coverage = 0.0;
    hit.color = vec3<f32>(0.0);
    hit.normal = vec3<f32>(0.0, 1.0, 0.0);
    if (density <= 0.001 || cell_size <= 1e-6) {
        return hit;
    }
    let o = flake_rot(origin) / cell_size;
    let d = flake_rot(dir);
    let n = flake_rot(normal);
    let fp = max(s.footprint / cell_size, 1e-4);
    let lod_fade = flake_lod_fade(fp);
    if (lod_fade <= 0.0) {
        return hit;
    }
    var best_t = 1e20;
    for (var i = 0; i < 5; i++) {
        let cell = floor(o + d * (0.4 * f32(i) + 0.05));
        let h0 = hash33(cell + seed);
        if (h0.x > density) {
            continue;
        }
        let radius = mix(0.22, 0.45, h0.y);
        let h1 = hash33(cell + seed + 19.19);
        let centre = cell + radius + h1 * (1.0 - 2.0 * radius);
        let h2 = hash33(cell + seed + 47.47);
        let nf = normalize(n + (h2 * 2.0 - 1.0) * tilt);

        let denom = dot(d, nf);
        if (abs(denom) < 0.05) {
            continue;
        }
        let t = dot(centre - o, nf) / denom;
        if (t < 0.0 || t >= best_t) {
            continue;
        }
        let q = o + d * t - centre;
        let ta = normalize(cross(nf, select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(nf.y) < 0.95)));
        let tb = cross(nf, ta);
        let angle = hash33(cell + seed + 83.83).x * TAU;
        let sn = sin(angle);
        let cs = cos(angle);
        let uv = vec2<f32>(dot(q, ta), dot(q, tb));
        let a = abs(vec2<f32>(uv.x * cs - uv.y * sn, uv.x * sn + uv.y * cs));
        let hex_dist = max(a.x * 0.8660254 + a.y * 0.5, a.y);
        let square_dist = max(a.x, a.y);
        let dist = mix(hex_dist, square_dist, shape);
        // Antialiased over one pixel, and a flake smaller than a pixel faded
        // to its share of it, or zooming out turns glitter into crawling noise.
        let coverage = saturate((radius - dist) / fp + 0.5) * saturate(2.0 * radius / fp) * lod_fade;
        if (coverage <= 0.01) {
            continue;
        }
        best_t = t;
        hit.depth = t * cell_size;
        hit.coverage = coverage;
        hit.normal = to_world_dir(s, flake_unrot(nf) / s.scale);
        if (palette) {
            // Colour weights ride in the alpha channels.
            let total = max(pal_a.a + pal_b.a + pal_c.a, 1e-4);
            let pick = h0.z * total;
            if (pick < pal_a.a) {
                hit.color = pal_a.rgb;
            } else if (pick < pal_a.a + pal_b.a) {
                hit.color = pal_b.rgb;
            } else {
                hit.color = pal_c.rgb;
            }
        } else {
            hit.color = pal_a.rgb;
        }
    }
    return hit;
}

// A glitter platelet is nearly a mirror: a tight lobe, so each flake is
// either catching a light or not - that is the sparkle as the view turns.
fn flake_glint(nf: vec3<f32>, l: vec3<f32>, v: vec3<f32>) -> f32 {
    return pow(saturate(dot(nf, normalize(l + v))), 160.0) * 8.0;
}

fn face_viewer(n: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    return select(n, -n, dot(n, v) < 0.0);
}

fn flake_radiance(s: Surface, color: vec3<f32>, n: vec3<f32>, diffuse: vec3<f32>, glint: vec3<f32>, sparkle: f32) -> vec3<f32> {
    let env = environment(reflect(-s.v, n), 0.3);
    return color * (diffuse + ambient(n)) * 0.7
        + color * env * 0.6
        + mix(color, vec3<f32>(1.0), 0.3) * glint * sparkle;
}

// Colour left after light crosses `distance` of plastic twice (down to a
// flake and back up), for plastic of colour `thick` when thick.
fn plastic_transmittance(thick: vec3<f32>, distance: f32, depth_scale: f32) -> vec3<f32> {
    return pow(max(thick, vec3<f32>(1e-3)), vec3<f32>(2.0 * distance / depth_scale));
}

// v[0] the colour where thin, v[1] where thick; v[2] x absorption depth,
// y transmission, z gloss; v[3..6] the glitter colours with their weights
// in alpha; v[6] the pepper flakes' colour; v[7] x glitter cell size,
// y density, z shape, w tilt; v[8] x sparkle, y pepper cell size, z density.
fn finish_plastic(s: Surface, params: FinishParams) -> MaterialSample {
    let thin = params.v[0].rgb;
    let thick = params.v[1].rgb;
    let plastic = params.v[2];
    let flake_a = params.v[3];
    let flake_b = params.v[4];
    let flake_c = params.v[5];
    let micro_color = params.v[6];
    let flake_p0 = params.v[7];
    let flake_p1 = params.v[8];
    let n = s.n;
    let depth_scale = max(plastic.x, 1e-5);

    // How much plastic the view ray crosses here. With no buffer (or nothing
    // behind): treat it as solid body.
    var thickness = depth_scale * 4.0;
    if (s.back_depth > 0.0) {
        let front = dot(s.world - g.cam_pos.xyz, g.cam_forward.xyz);
        // A back face in front of this surface means an open or intersecting
        // mesh: the difference means nothing there.
        if (s.back_depth >= front - depth_scale * 0.02) {
            thickness = max(s.back_depth - front, 0.0);
        }
    }
    let through = exp(-thickness / depth_scale);
    var body = mix(thick, thin, through);

    // The smooth normal steers the flakes even under flat shading, so a
    // flake does not break along triangle edges.
    let dir_os = normalize(to_object_dir(s, refract(-s.v, s.smooth_n, 1.0 / 1.5)) * s.scale);
    let surface_os = normalize(to_object_normal(s, s.smooth_n) / s.scale);
    let big = trace_flakes(s, s.p, dir_os, surface_os, flake_p0.x, flake_p0.y, 0.0, flake_p0.z, flake_p0.w, true,
        flake_a, flake_b, flake_c);
    let micro = trace_flakes(s, s.p, dir_os, surface_os, flake_p1.y, flake_p1.z, 131.0, flake_p0.z, flake_p0.w, false,
        micro_color, micro_color, micro_color);
    let weights = max(flake_a.a + flake_b.a + flake_c.a, 1e-4);
    let palette_mean = (flake_a.rgb * flake_a.a + flake_b.rgb * flake_b.a + flake_c.rgb * flake_c.a) / weights;
    body = faded_flake_tint(body, palette_mean, s.footprint, flake_p0.x, flake_p0.y);
    body = faded_flake_tint(body, micro_color.rgb, s.footprint, flake_p1.y, flake_p1.z);
    let n_big = face_viewer(big.normal, s.v);
    let n_micro = face_viewer(micro.normal, s.v);

    var diffuse_a = vec3<f32>(0.0);
    var glint_a = vec3<f32>(0.0);
    var diffuse_b = vec3<f32>(0.0);
    var glint_b = vec3<f32>(0.0);
    var back_light = vec3<f32>(0.0);
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, s);
        let shadowed = light.color * light.shadow;
        diffuse_a += shadowed * saturate(dot(n_big, light.dir));
        glint_a += shadowed * flake_glint(n_big, light.dir, s.v);
        diffuse_b += shadowed * saturate(dot(n_micro, light.dir));
        // Pepper flakes are specks, not mirrors.
        glint_b += shadowed * flake_glint(n_micro, light.dir, s.v) * 0.35;
        // Light entering the far side and leaving toward the eye. Unshadowed
        // on purpose: this is exactly the light passing through the model.
        let through_dir = normalize(light.dir + n * 0.35);
        back_light += light.color * pow(saturate(dot(s.v, -through_dir)), 3.0);
    }

    // Front to back: whichever flake is shallower covers the other.
    var w_big = big.coverage;
    var w_micro = micro.coverage;
    if (micro.depth < big.depth) {
        w_big *= 1.0 - w_micro;
    } else {
        w_micro *= 1.0 - w_big;
    }
    let t_big = plastic_transmittance(thick, big.depth, depth_scale);
    let t_micro = plastic_transmittance(thick, micro.depth, depth_scale);
    let flakes = w_big * t_big * flake_radiance(s, big.color, n_big, diffuse_a, glint_a, flake_p1.x)
        + w_micro * t_micro * flake_radiance(s, micro.color, n_micro, diffuse_b, glint_b, flake_p1.x);
    // A flake blocks the plastic behind it, by as much of it as shows through.
    let occluded = saturate(w_big * luminance(t_big) + w_micro * luminance(t_micro));

    var m = material(s, body * (1.0 - occluded));
    m.smoothness = plastic.z;
    let transmission = plastic.y;
    let glow = thin * through * (back_light + ambient(-n) * 0.6) * transmission;
    m.emission = flakes + glow * (1.0 - occluded);
    // Light scattered inside the plastic reaches past the shadow line, and
    // leaves tinted by the plastic it wandered through.
    m.scatter_color = mix(body, thin, 0.3) * transmission * 0.5 * (1.0 - occluded);
    m.scatter_wrap = 0.6;
    m.scatter_shadow = 0.5;
    return m;
}

// ---- aged metal -----------------------------------------------------------
//
// Antiqued, rusted, verdigris and washed finishes are the same three layers,
// and what sells them is where each ends up: bare metal polished bright on
// the raised edges, a coat in broad patches thickest in the recesses, and a
// dark wash pooled in the deepest ones.

// -1 (deep recess) .. +1 (sharp ridge), from the mesh's own curvature, so it
// stays where it is as the camera moves. `detail` rescales it: how much
// relief counts as a groove.
fn surface_convexity(curv: f32, detail: f32) -> f32 {
    let s = 1.0 - 2.0 * curv;
    // Undo the soft saturation (s = x / (1 + |x|)), scale, and apply it again.
    let x = s / max(1.0 - abs(s), 0.02) * detail;
    return x / (1.0 + abs(x));
}

// v[0] the bare metal's colour, v[1] and v[2] the coat's two colours, v[3]
// the wash's, v[4] the pattern's offset; v[5] x metal smoothness, y metal
// metallic, z coat metallic, w coat smoothness; v[6] x wash, y coverage,
// z edge wear, w patch size; v[7] x patch contrast, y patch amount, z coat
// follows recesses, w grain size; v[8] x grain, y detail, z patch breakup,
// w sparkle; v[9] x undulation, y its size.
fn finish_metal(s: Surface, params: FinishParams) -> MaterialSample {
    let metal_color_in = params.v[0];
    let coat_a = params.v[1];
    let coat_b = params.v[2];
    let wash_color = params.v[3];
    let metal_offset = params.v[4];
    let metal_p0 = params.v[5];
    let metal_p1 = params.v[6];
    let metal_p2 = params.v[7];
    let metal_p3 = params.v[8];
    let metal_p4 = params.v[9];
    var n = s.n;
    let p = s.p;
    let cv = surface_convexity(s.curv, metal_p3.y);
    // The offset slides the model through the noise, moving every blotch.
    let patch_p = p / max(metal_p1.w, 1e-5) + metal_offset.xyz;
    var patches = fbm(patch_p);
    let tint = fbm(patch_p * 2.7 + 41.7);
    // Grain finer than a pixel is only shimmer: fade it out as it gets there.
    let grain_size = max(metal_p2.w, 1e-6);
    let grain_lod = saturate(1.5 - s.footprint / grain_size);
    let grain = value_noise(p / grain_size + 7.3);
    let breakup = metal_p3.z;
    if (breakup > 0.001) {
        // Etched steel goes dull in lacy clusters, not soft blobs.
        patches += ((fbm(patch_p * 3.7 + 11.3) - 0.5) + (grain - 0.5) * 0.4 * grain_lod) * breakup;
    }

    // Worn to bare metal along the ridges, broken up by the patches.
    let wear = saturate(smoothstep(0.1, 0.55, cv) * metal_p1.z * (0.55 + 0.9 * patches));
    // The coat covers wherever the patch noise sits under the coverage
    // level, and recesses hold more of it than raised areas.
    let coat_level = metal_p1.y - cv * metal_p2.z;
    let patch_level = mix(0.5, patches, metal_p2.y);
    let coat = saturate((coat_level - patch_level) * metal_p2.x + 0.5) * (1.0 - wear);
    let wash = smoothstep(0.03, 0.6, -cv) * metal_p1.x;

    var coat_color = mix(coat_a.rgb, coat_b.rgb, saturate((tint - 0.5) * 2.5 + 0.5 - cv * 0.5));
    coat_color *= mix(1.0, mix(0.7, 1.2, grain), metal_p3.x * grain_lod);
    var metal_color = metal_color_in.rgb * mix(0.88, 1.08, tint);
    let sparkle = metal_p3.w;
    if (sparkle > 0.001) {
        // Fine bright specks: the tops of the grain left between pits.
        let sparkle_lod = saturate(1.5 - s.footprint * 2.0 / grain_size);
        let speck = smoothstep(0.4, 0.9, value_noise(p / grain_size * 2.0 + 19.7));
        metal_color *= mix(1.0, 0.82 + 0.6 * speck, sparkle * sparkle_lod);
    }

    var albedo = mix(metal_color, coat_color, coat);
    var metallic = mix(metal_p0.y, metal_p0.z, coat);
    var smoothness = mix(metal_p0.x, metal_p0.w, coat);
    albedo = mix(albedo, wash_color.rgb, wash);
    metallic *= 1.0 - wash;
    smoothness *= 1.0 - 0.35 * wash;

    // Hand-worked metal is never optically flat: a slow ripple that bends
    // the reflections the way hammering and filing leave a surface.
    let undulation = metal_p4.x;
    if (undulation > 0.001) {
        let ripple_p = p / max(metal_p4.y, 1e-5) + metal_offset.xyz * 0.37;
        n = perturb_normal(n, ripple_p, undulation * 0.35);
        n = perturb_normal(n, ripple_p * 2.3 + 5.1, undulation * 0.2);
    }
    // Pitted where coated, much less on the polished metal.
    let bump = metal_p3.x * grain_lod * mix(0.25, 1.0, coat);
    if (bump > 0.001) {
        n = perturb_normal(n, p / grain_size, bump * 0.6);
    }
    var m = material(s, albedo);
    m.metallic = metallic;
    m.smoothness = smoothness;
    m.occlusion = 1.0 - 0.5 * wash;
    m.normal = n;
    return m;
}

// ---- sculptor's clay ------------------------------------------------------
//
// Oil clay reads as clay rather than grey plastic through a satin sheen, a
// thin wet highlight riding on it, light bleeding a little past the shadow
// line, and recesses darker and richer than the flats.

// v[0] the clay's colour, v[1] the recesses', v[2] the scattered light's;
// v[3] x smoothness, y wetness, z wet power, w subsurface; v[4] x recess,
// y ridge, z mottle, w mottle size; v[5] x grain, y grain size, z detail.
fn finish_clay(s: Surface, params: FinishParams) -> MaterialSample {
    let clay_color = params.v[0];
    let clay_recess = params.v[1];
    let clay_scatter = params.v[2];
    let clay_p0 = params.v[3];
    let clay_p1 = params.v[4];
    let clay_p2 = params.v[5];
    var n = s.n;
    let p = s.p;
    let cv = surface_convexity(s.curv, clay_p2.z);

    // Clay is never one flat colour: a broad, faint drift in tone.
    let mottle = fbm(p / max(clay_p1.w, 1e-5));
    var albedo = clay_color.rgb * (1.0 + (mottle - 0.5) * 2.0 * clay_p1.z);

    let recess = saturate(smoothstep(0.0, 0.45, -cv) * clay_p1.x);
    let ridge = smoothstep(0.1, 0.6, cv) * clay_p1.y;
    albedo = mix(albedo, clay_recess.rgb, recess);
    albedo *= 1.0 + 0.25 * ridge;
    let smoothness = saturate(clay_p0.x + 0.15 * ridge - 0.2 * recess);

    // Fine grit in the clay body, faded out before it is smaller than a pixel.
    let grain_size = max(clay_p2.y, 1e-6);
    let grain = clay_p2.x * saturate(1.5 - s.footprint / grain_size);
    if (grain > 0.001) {
        n = perturb_normal(n, p / grain_size, grain * 0.12);
        let speck = value_noise(p / grain_size * 1.7 + 11.3);
        albedo *= mix(1.0, mix(0.9, 1.06, speck), saturate(grain));
    }

    var m = material(s, albedo);
    m.smoothness = smoothness;
    m.occlusion = 1.0 - 0.6 * recess;
    m.normal = n;
    // A thin wet highlight riding on the satin, thinning out in the
    // recesses, where the tools did not drag it.
    m.sheen_power = clay_p0.z;
    m.sheen_amount = clay_p0.y * (1.0 - 0.7 * recess);
    // Scattered light gets a little past the terminator.
    m.scatter_color = clay_scatter.rgb * clay_p0.w;
    m.scatter_wrap = 0.5;
    m.scatter_shadow = 0.6;
    // Recesses see less of the lights as well as less sky.
    m.dim = 1.0 - 0.35 * recess;
    return m;
}

// ---- carved wood ----------------------------------------------------------
//
// An old waxed carving reads as wood through figure that belongs to a log
// (rings are shells around a pith line through the model, so a long-grain
// face shows flames and an end-grain face rings), grime settled dark into
// the recesses with the edges rubbed back lighter, and a dull wax shine.

// Fibres: value noise stretched about 30x along the grain.
fn wood_streak_height(q: vec3<f32>, a: vec3<f32>, u: vec3<f32>, w: vec3<f32>, streak_size: f32) -> f32 {
    let s = vec3<f32>(dot(q, u), dot(q, w), dot(q, a) / 30.0) / streak_size;
    return value_noise(s) * 0.65 + value_noise(s * 2.3 + 5.1) * 0.35;
}

// v[0] earlywood's colour, v[1] latewood's, v[2] the worn edges', v[3] the
// recesses', v[4] the scattered light's; v[5] the grain's axis, v[6] the
// pith's place; v[7] x ring size, y ring contrast, z latewood fraction,
// w wobble; v[8] x figure size, y streaks, z streak size, w pores; v[9]
// x end grain, y patina, z wear, w smoothness; v[10] x wax, y wax power,
// z subsurface, w checks; v[11] x detail.
fn finish_wood(s: Surface, params: FinishParams) -> MaterialSample {
    let wood_early = params.v[0];
    let wood_late = params.v[1];
    let wood_worn = params.v[2];
    let wood_recess = params.v[3];
    let wood_scatter = params.v[4];
    let wood_axis = params.v[5];
    let wood_pith = params.v[6];
    let wood_p0 = params.v[7];
    let wood_p1 = params.v[8];
    let wood_p2 = params.v[9];
    let wood_p3 = params.v[10];
    let wood_p4 = params.v[11];
    var n = s.n;
    let p = s.p;
    let cv = surface_convexity(s.curv, wood_p4.x);

    // The log's frame: `a` along the grain, (u, w) across it.
    let a = normalize(wood_axis.xyz);
    let u = normalize(cross(a, select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(a.y) < 0.9)));
    let w = cross(a, u);
    let q = p - wood_pith.xyz;
    let along = dot(q, a);
    let radial = vec2<f32>(dot(q, u), dot(q, w));
    let radius = length(radial);

    // Growth rings. A trunk is never a true cylinder: low-frequency noise
    // pushes the rings in and out, stretched along the axis so they stay
    // coherent along the grain - which draws the long flames.
    let ring_size = max(wood_p0.x, 1e-6);
    let fig_size = max(wood_p1.x, 1e-5);
    let sharp = wood_p0.z;
    let wob = fbm(vec3<f32>(radial.x, along * 0.3, radial.y) / fig_size) * 2.0 - 1.0;
    var phase = (radius + wob * wood_p0.w * ring_size * 4.0) / ring_size;
    // Good years and lean years: ring widths vary.
    phase += value_noise(vec3<f32>(phase * 0.3, along / fig_size * 0.4, 7.7)) * 1.5;
    let t = fract(phase);
    // How far the phase moves across one pixel. Not `fwidth(phase)`: a
    // finish may run inside a branch (a painted material, chosen pixel by
    // pixel), where screen derivatives mean nothing. The phase climbs one per
    // ring straight out from the pith, so it is the pixel's step along that
    // way, in rings (the wobble and the uneven years are slow beside it).
    let outward = (u * radial.x + w * radial.y) / max(radius, 1e-6);
    let fw = surface_width(s, outward / ring_size);
    // Earlywood eases into dark latewood, which stops dead at the next year.
    var late = smoothstep(1.0 - sharp * 1.8, 1.0 - sharp * 0.3, t)
        * (1.0 - smoothstep(1.0 - max(fw, 0.02), 1.0, t));
    // Once a pixel spans most of a ring, settle on the average.
    let ring_lod = saturate(1.3 - fw * 1.6);
    late = mix(sharp * 0.8, late, ring_lod);
    var wood = mix(wood_early.rgb, wood_late.rgb, saturate(late * wood_p0.y));

    // Fibres and pores, faded before they are smaller than a pixel.
    let streaks = wood_p1.y;
    let streak_size = max(wood_p1.z, 1e-6);
    let streak_lod = saturate(1.5 - s.footprint / streak_size);
    let streak = wood_streak_height(q, a, u, w, streak_size);
    wood *= 1.0 + (streak - 0.5) * 0.7 * streaks * streak_lod;
    let pores = wood_p1.w;
    if (pores > 0.001) {
        // Ring-porous: big vessels in the earlywood, cut open as dark dashes.
        let ps = vec3<f32>(radial.x, radial.y, along / 6.0) / (streak_size * 0.6);
        let pore = smoothstep(0.7, 0.82, value_noise(ps + 13.7)) * (1.0 - late)
            * saturate(1.5 - s.footprint / (streak_size * 0.6));
        wood *= 1.0 - 0.55 * saturate(pore * pores);
    }

    // End grain drinks the finish and goes darker.
    let surface_s = normalize(to_object_normal(s, s.smooth_n) / s.scale);
    let end_grain = smoothstep(0.55, 0.95, abs(dot(surface_s, a))) * wood_p2.x;
    wood *= 1.0 - 0.35 * end_grain;

    // Age: grime in the recesses, rubbed-back edges on the high points.
    let wear_noise = fbm(p / fig_size * 3.0 + 23.1);
    let wear = saturate(smoothstep(0.12, 0.55, cv) * wood_p2.z * (0.55 + 0.9 * wear_noise));
    // The worn colour keeps the figure: scaled by how light this bit of wood is.
    let wood_lum = luminance(wood) / max(luminance(wood_early.rgb), 1e-3);
    var albedo = mix(wood, wood_worn.rgb * wood_lum, wear);
    let recess = saturate(smoothstep(0.0, 0.4, -cv) * wood_p2.y);
    albedo = mix(albedo, wood_recess.rgb, recess);

    // Checks: seasoning cracks, radial from the pith, running along the grain.
    var crack = 0.0;
    let checks = wood_p3.w;
    if (checks > 0.001) {
        let sectors = 7.0;
        var angle = atan2(radial.y, radial.x) / TAU + 0.5;
        // Cracks wander a little rather than ruling a straight line.
        angle += (value_noise(vec3<f32>(along / fig_size * 2.5, 3.3, 1.1)) - 0.5) * 0.02;
        let sector = floor(angle * sectors);
        let h = hash33(vec3<f32>(sector, 17.0, 5.0));
        if (h.x < checks) {
            let crack_angle = (sector + 0.2 + 0.6 * h.y) / sectors;
            let dist = abs(sin((angle - crack_angle) * TAU)) * radius;
            // Checks open outward from the pith and run part of the log's length.
            let width = ring_size * 0.35 * saturate(radius / fig_size);
            let runs = smoothstep(0.35, 0.5, value_noise(vec3<f32>(along / fig_size * 0.6, h.z * 40.0, 9.0)));
            let fp = max(s.footprint, 1e-6);
            crack = saturate((width - dist) / fp + 0.5) * saturate(width * 2.0 / fp) * runs;
        }
    }
    albedo = mix(albedo, wood_recess.rgb * 0.5, crack);

    let smoothness = saturate(wood_p2.w + 0.12 * wear - 0.3 * recess) * (1.0 - crack);

    // Fibres raise the surface a hair, which breaks the sheen along the grain.
    let bump = streaks * streak_lod;
    if (bump > 0.001) {
        let e = streak_size * 0.35;
        let grad = vec3<f32>(
            wood_streak_height(q + vec3<f32>(e, 0.0, 0.0), a, u, w, streak_size) - streak,
            wood_streak_height(q + vec3<f32>(0.0, e, 0.0), a, u, w, streak_size) - streak,
            wood_streak_height(q + vec3<f32>(0.0, 0.0, e), a, u, w, streak_size) - streak) / e;
        let grad_ws = s.mx * (grad.x / s.scale.x) + s.my * (grad.y / s.scale.y) + s.mz * (grad.z / s.scale.z);
        n = normalize(n - (grad_ws - n * dot(grad_ws, n)) * (bump * 0.05 * streak_size));
    }

    var m = material(s, albedo);
    m.smoothness = smoothness;
    m.occlusion = 1.0 - 0.6 * recess;
    m.normal = n;
    // As the clay's wet film, but broad: rubbed wax is a soft glow. Wax
    // builds up where hands polish it and is dull with grime in the recesses.
    m.sheen_power = wood_p3.y;
    m.sheen_amount = wood_p3.x * (1.0 - 0.8 * recess) * (0.7 + 0.6 * wear) * (1.0 - crack);
    m.scatter_color = wood_scatter.rgb * wood_p3.z * (1.0 - recess);
    m.scatter_wrap = 0.4;
    m.scatter_shadow = 0.6;
    m.dim = 1.0 - 0.35 * recess;
    return m;
}

// ---- glow -----------------------------------------------------------------
//
// Light the surface gives off: a bright colour added over the finished
// shading, with the surface under it dimmed a little so the glow reads.
// Each pattern says how strongly a point glows (m) and how hot it is (heat:
// 0 = the cool colour at the glow's edge, 1 = the hot core).

struct Cells {
    f1: f32,
    f2: f32,
    nearest: vec3<f32>,
};

// Distances to the nearest and second-nearest jittered feature point, and
// the nearest one's cell. f2 - f1 is ~0 along the borders between cells.
fn glow_cells(p: vec3<f32>) -> Cells {
    let cell = floor(p);
    let f = p - cell;
    var out: Cells;
    out.f1 = 8.0;
    out.f2 = 8.0;
    out.nearest = cell;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let o = vec3<f32>(f32(x), f32(y), f32(z));
                let d = o + hash33(cell + o) - f;
                let dist = dot(d, d);
                if (dist < out.f1) {
                    out.f2 = out.f1;
                    out.f1 = dist;
                    out.nearest = cell + o;
                } else if (dist < out.f2) {
                    out.f2 = dist;
                }
            }
        }
    }
    out.f1 = sqrt(out.f1);
    out.f2 = sqrt(out.f2);
    return out;
}

// The lava's crust plates are cells warped so they are not polygons, slowly
// shifting: where in the cells point `p` (pattern units) falls at time `t`.
fn glow_lava_warp(p: vec3<f32>, t: f32) -> vec3<f32> {
    let warp = vec3<f32>(
        value_noise(p * 0.6 + vec3<f32>(0.0, t * 0.05, 0.0)),
        value_noise(p * 0.6 + vec3<f32>(13.1, 0.0, t * 0.05)),
        value_noise(p * 0.6 + vec3<f32>(t * 0.05, 27.3, 5.7))) - 0.5;
    return p + warp * 0.8;
}

// The two drifting noise fields whose zero-crossings are the energy veins.
fn glow_vein_a(p: vec3<f32>, t: f32) -> f32 {
    return fbm(p + vec3<f32>(0.0, t * 0.25, 0.0)) - 0.5;
}

fn glow_vein_b(p: vec3<f32>, t: f32) -> f32 {
    return fbm(p * 1.9 + vec3<f32>(t * 0.2, 7.7, 3.1)) - 0.5;
}

// A line where v crosses zero, `width` wide and hottest at its centre.
// `fw` is how much v changes across a pixel: the line is widened to at least
// a pixel and dimmed to match, so a thin line fades out as the camera pulls
// back rather than breaking into dashes. The callers work `fw` out from the
// pixel's steps in the object's space (`Surface::p_dx`, `p_dy`), not with
// `fwidth`: a glow can be a painted material, run inside a branch chosen
// pixel by pixel, where screen derivatives mean nothing.
fn glow_line(v: f32, fw: f32, width: f32) -> f32 {
    let w = max(width, fw * 1.5);
    return saturate(1.0 - abs(v) / w) * saturate(width / w);
}

// v[0] the hot colour, v[1] the cool one; v[2] x on, y pattern, z intensity,
// w coverage; v[3] x rim, y pattern size, z speed, w pulse; v[4] x pulse
// rate, y dim. The light given off is the sample's emission, and what is
// under it is multiplied by its `dim`.
fn finish_glow(s: Surface, params: FinishParams) -> MaterialSample {
    let glow_hot = params.v[0];
    let glow_cool = params.v[1];
    let glow_p0 = params.v[2];
    let glow_p1 = params.v[3];
    let glow_p2 = params.v[4];
    let n = s.n;
    let pattern_size = max(glow_p1.y, 1e-5);
    let p = s.p / pattern_size;
    let t = look.light_params.w * glow_p1.z;
    let facing = saturate(dot(n, s.v));
    let rim = pow(1.0 - facing, 3.0);
    let cv = surface_convexity(s.curv, 1.0);
    let coverage = glow_p0.w;
    let pattern = i32(glow_p0.y + 0.5);

    var m = 0.0;
    var heat = 0.0;
    if (pattern == 0) {
        // Neon: all of it, a pale core where it faces the camera.
        m = 1.0;
        heat = saturate(facing * 1.3 - 0.2);
    } else if (pattern == 1) {
        // Aura: the silhouette, wider with coverage, with wisps drifting through.
        let rim_power = mix(5.0, 1.0, saturate(coverage / 1.5));
        m = pow(1.0 - facing, rim_power);
        m *= 0.8 + 0.4 * value_noise(p * 1.5 + vec3<f32>(0.0, -t * 0.4, 0.0));
        heat = m;
    } else if (pattern == 2) {
        // Lava: crust plates, warped so they are not polygons and slowly
        // shifting, split by molten cracks; molten pools in the recesses.
        let q = glow_lava_warp(p, t);
        let cells = glow_cells(q);
        let edge = cells.f2 - cells.f1;
        // How much the crack field changes to the next pixel right and down,
        // as `fwidth(edge)` would measure it, without screen derivatives.
        let right = glow_cells(glow_lava_warp(p + s.p_dx / pattern_size, t));
        let down = glow_cells(glow_lava_warp(p + s.p_dy / pattern_size, t));
        let fw_edge = abs(right.f2 - right.f1 - edge) + abs(down.f2 - down.f1 - edge);
        let crack = glow_line(edge, fw_edge, 0.04 + 0.12 * coverage);
        // Each stretch of crack flares and fades on its own slow cycle.
        let flare = 0.6 + 0.4 * value_noise(q * 1.3 + vec3<f32>(0.0, 0.0, t * 0.3));
        let pool = smoothstep(0.35 - 0.4 * coverage, 0.75 - 0.4 * coverage, -cv);
        m = max(crack * flare, pool * 0.85);
        heat = max(crack * crack * flare, pool * 0.6);
    } else if (pattern == 3) {
        // Veins: the zero-crossings of two drifting noise fields.
        let n1 = glow_vein_a(p, t);
        let n2 = glow_vein_b(p, t);
        // Each field's change to the next pixel right and down, as `fwidth`
        // would measure it, without screen derivatives.
        let dx = s.p_dx / pattern_size;
        let dy = s.p_dy / pattern_size;
        let fw1 = abs(glow_vein_a(p + dx, t) - n1) + abs(glow_vein_a(p + dy, t) - n1);
        let fw2 = abs(glow_vein_b(p + dx, t) - n2) + abs(glow_vein_b(p + dy, t) - n2);
        let width = 0.02 + 0.05 * coverage;
        m = max(glow_line(n1, fw1, width), glow_line(n2, fw2, width * 0.7) * 0.7);
        heat = m * m;
        m *= 0.7 + 0.3 * value_noise(p * 3.0 - t * 1.5);
    } else if (pattern == 4) {
        // Forge: raised forms and thin edges are hottest, the rest a dull red.
        let level = 0.45 - 0.9 * coverage;
        var hot = saturate((cv - level) / 0.6 + 0.25);
        hot = saturate(hot * (0.85 + 0.3 * fbm(p + vec3<f32>(0.0, t * 0.15, 0.0))));
        m = hot;
        heat = hot;
    } else if (pattern == 5) {
        // Hologram: bright rim, scanlines rolling up the world's vertical, a
        // brighter sweep now and then, and a flicker.
        let fres = pow(1.0 - facing, 2.0);
        let y = s.world.y / pattern_size;
        let scan_v = fract(y - t * 0.5) - 0.5;
        // The scanlines climb with the world's height: one pixel's step in
        // it, from the pixel's steps in the object's own space.
        let rise = vec3<f32>(s.mx.y, s.my.y, s.mz.y) / s.scale;
        let scan = glow_line(scan_v, surface_width(s, rise) / pattern_size, 0.04 + 0.12 * coverage);
        let sweep = pow(fract(y * 0.05 - t * 0.12), 12.0);
        let flicker = 0.85 + 0.15 * value_noise(vec3<f32>(t * 9.0, 0.5, 0.5));
        m = saturate(fres * 1.2 + scan * 0.45 + sweep * 0.5 + 0.08) * flicker;
        heat = fres;
    } else if (pattern == 6) {
        // Bioluminescence: some cells get a soft spot, each on its own beat.
        let cells = glow_cells(p);
        let h = hash33(cells.nearest + 71.3);
        let lit = step(h.x, 0.35 + 0.4 * coverage);
        let radius = (0.15 + 0.2 * coverage) * (0.6 + 0.8 * h.y);
        var spot = saturate(1.0 - cells.f1 / radius);
        spot *= spot;
        let beat = 0.55 + 0.45 * sin(t * (1.2 + h.z) * 2.0 + h.y * TAU);
        // Spots smaller than a pixel would only sparkle: settle on their average.
        let lod = saturate(1.5 - s.footprint / pattern_size * 2.0);
        m = mix(0.08 * lit, spot * lit * beat, lod);
        heat = spot;
    } else {
        // Inner light: shining out of every recess, shimmering a little.
        let level = 0.3 - 0.5 * coverage;
        let pool = smoothstep(level, level + 0.45, -cv);
        m = pool * (0.8 + 0.4 * fbm(p + vec3<f32>(0.0, t * 0.2, 0.0)));
        heat = pool;
    }

    let rim_amount = glow_p1.x;
    m += rim * rim_amount;
    heat = max(heat, rim * rim_amount);
    // Breathing: down to half brightness at full depth.
    m *= mix(1.0, 0.5 + 0.5 * sin(t * TAU * glow_p2.x), glow_p1.w);

    let color = mix(glow_cool.rgb, glow_hot.rgb, saturate(heat));
    var out = material(s, vec3<f32>(0.0));
    out.emission = color * (m * glow_p0.z);
    out.dim = 1.0 - glow_p2.y;
    return out;
}

// ---- cast resin -----------------------------------------------------------
//
// Clear or tinted resin, as a print or a casting is: a glossy surface that
// reflects at a glance (the lighting does that, as for any gloss) and lets
// what is behind it through head on, bent by the resin, tinted and darkened
// by as much resin as the view crosses (the plastic's back-face thickness,
// Beer and Lambert's law over it), a little milky haze lit like a surface,
// and light from behind glowing through.

// What the view sees in direction `dir` past the model: the backdrop as it
// is drawn, so a resin in a dark studio looks dark through and one in front
// of an HDRI shows it.
fn backdrop(dir: vec3<f32>) -> vec3<f32> {
    let mode = look.bg_params.x;
    if (mode > 1.5) {
        return textureSampleLevel(hdri_tex, hdri_smp, hdri_uv(dir, look.bg_params.y), 2.0).rgb * look.bg_params.z;
    }
    if (mode > 0.5) {
        let t = pow(clamp(dir.y * 0.5 + 0.5, 0.0, 1.0), 1.0 / max(look.bg_params.w, 0.001));
        return mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
    }
    return g.bg_bottom.rgb;
}

// v[0] the colour light takes on through `w` of resin (world units); v[1]
// x haze, y smoothness, z transmission.
fn finish_resin(s: Surface, params: FinishParams) -> MaterialSample {
    let tint = params.v[0].rgb;
    let depth = max(params.v[0].w, 1e-5);
    let haze = saturate(params.v[1].x);
    let n = s.n;
    // How much resin the view crosses; with nothing behind, a solid body.
    var thickness = depth * 2.0;
    if (s.back_depth > 0.0) {
        let front = dot(s.world - g.cam_pos.xyz, g.cam_forward.xyz);
        if (s.back_depth >= front - depth * 0.02) {
            thickness = max(s.back_depth - front, 0.0);
        }
    }
    let through = pow(max(tint, vec3<f32>(1e-3)), vec3<f32>(thickness / depth));
    // What is behind, along the view bent into the resin, and only what the
    // surface does not reflect (the lighting adds the reflection).
    var bent = refract(-s.v, n, 1.0 / 1.5);
    if (dot(bent, bent) < 0.5) {
        bent = -s.v;
    }
    let nv = saturate(dot(n, s.v));
    let passes = 1.0 - (0.04 + 0.96 * pow5(1.0 - nv));
    let behind = backdrop(bent) * through * passes * (1.0 - haze);
    // A light behind the model seen through it, tinted by the resin: where
    // the bent view points at it, sharply in clear resin and spread wide in
    // milky.
    var glow = vec3<f32>(0.0);
    let spread = mix(48.0, 3.0, sqrt(haze));
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, s);
        glow += light.color * pow(saturate(dot(bent, light.dir)), spread);
    }
    var m = material(s, tint * haze);
    m.smoothness = params.v[1].y;
    m.emission = behind + glow * through * params.v[1].z * passes;
    // A milky resin scatters light past the shadow line, as clay does.
    m.scatter_color = tint * haze * 0.5;
    m.scatter_wrap = 0.6;
    m.scatter_shadow = 0.5;
    return m;
}

// ---- the surface ----------------------------------------------------------

// What this pipeline's finish makes of the pixel.
fn surface_material(s: Surface) -> MaterialSample {
    if (FINISH == FINISH_PLASTIC) {
        return finish_plastic(s, look.finish);
    }
    if (FINISH == FINISH_METAL) {
        return finish_metal(s, look.finish);
    }
    if (FINISH == FINISH_CLAY) {
        return finish_clay(s, look.finish);
    }
    if (FINISH == FINISH_WOOD) {
        return finish_wood(s, look.finish);
    }
    if (FINISH == FINISH_RESIN) {
        return finish_resin(s, look.finish);
    }
    return finish_pbr(s, look.finish);
}

// What the pixel is, before any light: where it is, which way it faces,
// its place in the object's own pattern space, its curvature and the
// surface grain. Shared with the painted pipelines (painted.wgsl), so a
// painted object and a bare one are the same surface.
fn mesh_surface(in: VsOut, front: bool) -> Surface {
    var n = shading_normal(in.normal, in.world, front);
    let smooth_n = select(-normalize(in.normal), normalize(in.normal), front);

    var s: Surface;
    s.world = in.world;
    s.v = normalize(g.cam_pos.xyz - in.world);
    if (g.viewport.z <= 0.0) {
        s.v = -g.cam_forward.xyz;
    }
    s.pixel = vec2<i32>(in.pos.xy);
    s.mx = in.model_x;
    s.my = in.model_y;
    s.mz = in.model_z;
    // The object's scale along each of its own axes, so a squashed object
    // gets patterns of the same size rather than squashed ones.
    s.scale = vec3<f32>(length(s.mx), length(s.my), length(s.mz));
    s.p = in.local * s.scale;
    s.footprint = length(fwidth(s.p));
    s.p_dx = dpdx(s.p);
    s.p_dy = dpdy(s.p);
    s.smooth_n = smooth_n;
    s.curv = in.curv;
    s.base_color = g.base_color.rgb;
    s.shadow = 1.0;
    s.contact = vec4<f32>(1.0);
    s.contact_last = 1.0;
    s.ao = 1.0;
    s.ao_direct = 1.0;
    s.bent = smooth_n;
    s.back_depth = 0.0;

    // Surface grain: a small bump straight from the object-space position,
    // which reads through a matcap exactly as it does under real lights.
    if (look.base_params.z > 0.0001) {
        n = perturb_normal(n, in.local * look.base_params.w, look.base_params.z);
    }
    s.n = n;
    return s;
}

// A crease's own shadow toward the light `dir`, too fine for the shadow
// map: a short march from the surface toward the light through the depth
// the normals prepass drew. A point of the march with something drawn in
// front of it, but not so far in front that it is a different surface
// passing behind, is in shadow; the farther along the march, the less.
fn contact_shadow(s: Surface, dir: vec3<f32>) -> f32 {
    if (look.contact.x <= 0.0 || dot(s.smooth_n, dir) <= 0.0) {
        return 1.0;
    }
    // Short on screen, so it reaches fine detail and no further at any zoom.
    let depth = dot(s.world - g.cam_pos.xyz, g.cam_forward.xyz);
    let view_height = select(2.0 * look.cam_params.x, 2.0 * g.viewport.z * depth, g.viewport.z > 0.0);
    let reach = view_height * look.contact.x;
    let size = vec2<f32>(textureDimensions(gbuf));
    let steps = 8;
    let jitter = pixel_noise(vec2<f32>(s.pixel) + 0.5);
    // Off the surface by a couple of its pixels, so it does not shadow itself.
    let start = s.world + s.smooth_n * (s.footprint * 2.0);
    // Deeper in front than this is taken for another surface passing in
    // front, not one the light has to get past. Generous: a march that
    // starts under an overhang is inside the solid, far behind its front.
    let thickness = reach * 8.0;
    for (var k = 0; k < steps; k++) {
        let t = (f32(k) + jitter) / f32(steps);
        let q = start + dir * (t * reach);
        let clip = g.view_proj * vec4<f32>(q, 1.0);
        if (clip.w <= 0.0) {
            break;
        }
        let ndc = clip.xy / clip.w;
        let at = vec2<f32>(ndc.x * 0.5 + 0.5, 0.5 - ndc.y * 0.5) * size;
        if (any(at < vec2<f32>(0.0)) || any(at >= size)) {
            break;
        }
        let drawn = textureLoad(gbuf, vec2<i32>(at), 0).w;
        let behind = dot(q - g.cam_pos.xyz, g.cam_forward.xyz) - drawn;
        if (drawn > 0.0 && behind > s.footprint && behind < thickness) {
            // Full right at the crease, fading out along the march: the
            // light's own size softens a shadow that far from its caster.
            return sqrt(t);
        }
    }
    return 1.0;
}

// The surface with what reaches it of the lights: the key's shadow, each
// light's contact shadow, the ambient occlusion and its bent normal, and the
// back faces behind it for the plastic.
fn lit_surface(surface: Surface) -> Surface {
    var s = surface;
    s.shadow = key_shadow(s.world, s.smooth_n, vec2<f32>(s.pixel));
    if (look.contact.x > 0.0) {
        let count = light_count();
        for (var i = 0; i < min(count, 4); i++) {
            s.contact[i] = contact_shadow(s, look.light_dir[i].xyz);
        }
        if (count > 4) {
            s.contact_last = contact_shadow(s, look.light_dir[4].xyz);
        }
    }
    if (look.env_params.z > 0.5) {
        let occlusion = textureLoad(ao_tex, s.pixel, 0);
        s.ao = occlusion.a;
        s.ao_direct = mix(1.0, s.ao, look.env_params.w);
        // Back from the occlusion's view space (z forward) to the world.
        let b = occlusion.rgb * 2.0 - 1.0;
        let to_world = transpose(mat3x3<f32>(g.view[0].xyz, g.view[1].xyz, g.view[2].xyz));
        let bent = to_world * vec3<f32>(b.x, b.y, -b.z);
        if (dot(bent, bent) > 0.01) {
            s.bent = normalize(bent);
            // Facing the eye as the surface does.
            s.bent = select(s.bent, -s.bent, dot(s.bent, s.smooth_n) < -0.5);
        }
    }
    if (look.matcap_params.w > 0.5) {
        s.back_depth = textureLoad(back_tex, s.pixel, 0).r;
    }
    return s;
}

// What goes over the shaded surface, the same for every pipeline: cavity,
// the glow, the inside's tint, the mask and the symmetry guides.
fn mesh_finish(shaded: vec3<f32>, s: Surface, in: VsOut, front: bool) -> vec4<f32> {
    var color = shaded;
    // Cavity multiplies the finished colour - over a matcap's baked lighting
    // or the lit result alike - and before the mask tint, so a masked crease
    // still reads as a crease.
    color *= cavity(in.pos.xy);

    // Glow goes on after cavity (a crease cavity darkens is exactly where
    // lava should shine) but before the mask tint.
    if (GLOW) {
        let glow = finish_glow(s, look.glow);
        color = color * glow.dim + glow.emission;
    }

    if (!front) {
        // Inside of the surface: dim and cool, so holes and flipped faces read.
        color = mix(color * 0.35, vec3<f32>(0.05, 0.07, 0.12), 0.5);
    }
    // Mask: darkens the fully lit result rather than the colour fed into
    // lighting, so highlights dim too.
    color = mix(color, color * look.mask_tint.rgb, saturate(in.mask * look.mask_tint.a));

    // Mirror plane guides: a thin tinted line where each plane cuts the surface.
    // The planes are x = 0, y = 0 and z = 0 of the space the object's symmetry
    // sits in: its own, or the world's (g.guide_space takes the surface there).
    let at = (g.guide_space * vec4<f32>(in.local, 1.0)).xyz;
    let w = fwidth(at);
    var axis_color = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.05, 0.05), vec3<f32>(0.1, 1.0, 0.1), vec3<f32>(0.08, 0.26, 1.0));
    for (var i = 0; i < 3; i++) {
        if (g.symmetry[i] > 0.5 && in.is_active > 0.5) {
            let d = abs(at[i]) / max(w[i], 1e-9);
            let guide = 1.0 - smoothstep(0.4, 1.1, d);
            color = mix(color, axis_color[i], guide * 0.35);
        }
    }
    // Radial symmetry guide: the same thin line along the edge of each
    // sector, one spoke per repeat, meeting where the axis comes through
    // the surface.
    let across_x = dpdx(at);
    let across_y = dpdy(at);
    let spokes = g.radial_axis.w;
    if (spokes > 1.5 && in.is_active > 0.5) {
        let u = g.radial_spoke.xyz;
        let v = cross(g.radial_axis.xyz, u);
        let along = vec2<f32>(dot(at, u), dot(at, v));
        // The nearest spoke, as an angle round the axis. On the axis itself
        // every spoke is as near as any other.
        let sector = TAU / spokes;
        let off_axis = abs(along.x) + abs(along.y) > 0.0;
        let turn = select(0.0, round(atan2(along.y, along.x) / sector) * sector, off_axis);
        // How far this point is from that spoke's plane, in pixels.
        let normal = cos(turn) * v - sin(turn) * u;
        let per_pixel = abs(dot(across_x, normal)) + abs(dot(across_y, normal));
        let d = abs(dot(at, normal)) / max(per_pixel, 1e-9);
        let guide = 1.0 - smoothstep(0.4, 1.1, d);
        var spoke_color = array<vec3<f32>, 4>(
            axis_color[0], axis_color[1], axis_color[2], vec3<f32>(1.0, 0.6, 0.05));
        color = mix(color, spoke_color[i32(g.radial_spoke.w)], guide * 0.35);
    }
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    let front = is_front(handedness(in.model_x, in.model_y, in.model_z), facing);
    var s = mesh_surface(in, front);
    var color: vec3<f32>;
    if (FINISH == FINISH_MATCAP) {
        color = matcap_shade(s.n);
    } else {
        s = lit_surface(s);
        color = light_sample(s, surface_material(s));
    }
    return mesh_finish(color, s, in, front);
}

// ---- the floor ------------------------------------------------------------
//
// A shadow catcher (render/floor.rs): a square under the model that is the
// backdrop itself, darkened by the key's shadow and the occlusion where the
// model meets it, with the model's reflection on it, all fading out toward
// its edge. Drawn over the backdrop, premultiplied: alpha is how much of the
// backdrop it takes away, colour what it adds.

struct FloorOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
};

@vertex
fn vs_floor(@builtin(vertex_index) i: u32) -> FloorOut {
    // Two triangles over the square, out to where it has faded away.
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, -1.0), vec2<f32>(1.0, 1.0),
        vec2<f32>(-1.0, -1.0), vec2<f32>(1.0, 1.0), vec2<f32>(-1.0, 1.0));
    let c = corners[i] * look.floor.z;
    var out: FloorOut;
    out.world = vec3<f32>(look.floor_more.x + c.x, look.floor.y, look.floor_more.y + c.y);
    out.pos = g.view_proj * vec4<f32>(out.world, 1.0);
    if (look.floor.x < 0.5) {
        // Off: every corner in one place, so nothing is drawn.
        out.pos = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }
    return out;
}

// How much of the floor there is here: all of it near the model, none at
// its edge.
fn floor_fade(world: vec3<f32>) -> f32 {
    let d = length(world.xz - look.floor_more.xy) / max(look.floor.z, 1e-6);
    return 1.0 - smoothstep(0.35, 1.0, d);
}

// Toward the eye from a point on the floor.
fn floor_view(world: vec3<f32>) -> vec3<f32> {
    if (g.viewport.z <= 0.0) {
        return -g.cam_forward.xyz;
    }
    return normalize(g.cam_pos.xyz - world);
}

// The floor in the normals prepass: facing up, at its eye depth.
@fragment
fn fs_floor_prepass(in: FloorOut) -> @location(0) vec4<f32> {
    if (floor_view(in.world).y <= 0.0) {
        discard;
    }
    let vn = (g.view * vec4<f32>(0.0, 1.0, 0.0, 0.0)).xyz;
    let depth = dot(in.world - g.cam_pos.xyz, g.cam_forward.xyz);
    return vec4<f32>(vn, max(depth, 1e-6));
}

@fragment
fn fs_floor(in: FloorOut) -> @location(0) vec4<f32> {
    let v = floor_view(in.world);
    // Seen from below there is no floor.
    if (v.y <= 0.0) {
        discard;
    }
    let fade = floor_fade(in.world);
    let pixel = in.pos.xy;
    let up = vec3<f32>(0.0, 1.0, 0.0);
    // The key's shadow, as dark as the floor's own setting, and the
    // occlusion where the model comes close above it.
    var lit = 1.0;
    if (look.light_params.y > 0.5) {
        lit = 1.0 - (1.0 - key_light_reaching(in.world, up, pixel)) * look.floor.w;
    }
    var open = 1.0;
    if (look.env_params.z > 0.5) {
        open = textureLoad(ao_tex, vec2<i32>(pixel), 0).a;
    }
    let dark = (1.0 - lit * open) * fade;

    // The model's reflection, blurred by the floor's roughness: taps over a
    // disc as wide as that, turned for each pixel and each sample.
    var reflected = vec4<f32>(0.0);
    let strength = look.floor_more.z;
    if (strength > 0.0) {
        let uv = pixel / g.viewport.xy;
        let blur = look.floor_more.w * look.floor_more.w * 0.05;
        let turn = pixel_noise(pixel) * TAU;
        var way = vec2<f32>(cos(turn), sin(turn));
        let aspect = vec2<f32>(1.0 / max(g.viewport.w, 1e-3), 1.0);
        const TAPS: i32 = 8;
        for (var i = 0; i < TAPS; i++) {
            let r = sqrt((f32(i) + 0.5) / f32(TAPS)) * blur;
            reflected += textureSampleLevel(floor_tex, env_smp, uv + way * aspect * r, 0.0);
            way = GOLDEN_TURN * way;
        }
        reflected /= f32(TAPS);
        // A glossy floor reflects more at a glancing angle.
        let k = strength * (0.3 + 0.7 * pow5(1.0 - saturate(v.y))) * fade;
        reflected *= k;
    }
    // Premultiplied: the reflection is added, and it and the shadow take
    // that much of the backdrop away.
    return vec4<f32>(reflected.rgb, dark + reflected.a * (1.0 - dark));
}

// ---- helper passes --------------------------------------------------------

// The scene from the key light, for its shadow. Casters are pushed a little
// away from the light and shrunk along their normals where they turn from
// it, which keeps a surface from shadowing itself.
@vertex
fn vs_shadow(in: PreIn) -> @builtin(position) vec4<f32> {
    let x = in.model_x.xyz;
    let y = in.model_y.xyz;
    let z = in.model_z.xyz;
    var world = x * in.position.x + y * in.position.y + z * in.position.z + in.model_w.xyz;
    let n = normalize((cross(y, z) * in.normal.x + cross(z, x) * in.normal.y + cross(x, y) * in.normal.z)
        * handedness(x, y, z));
    let l = look.light_dir[0].xyz;
    world -= l * look.shadow_params.y;
    world -= n * ((1.0 - saturate(dot(l, n))) * look.shadow_params.z);
    return look.shadow_view_proj * vec4<f32>(world, 1.0);
}

// Eye depth of back faces, so the lure plastic can tell a thin claw from a
// thick body by how much plastic the view ray crosses.
@fragment
fn fs_back_depth(in: PreOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    // Both faces are rasterised and the front ones dropped here, not culled by
    // the pipeline: for a mirror copy the hardware's front is the far side.
    if (is_front(in.hand, facing)) {
        discard;
    }
    let depth = dot(in.world - g.cam_pos.xyz, g.cam_forward.xyz);
    return vec4<f32>(max(depth, 1e-6), 0.0, 0.0, 1.0);
}
