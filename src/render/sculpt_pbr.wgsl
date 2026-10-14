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
    // x one shadow texel in uv, y depth bias, z normal bias (world units)
    shadow_params: vec4<f32>,
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
};

@group(0) @binding(4) var<uniform> look: Look;
// The surroundings, blurrier in each smaller level, for reflections.
@group(0) @binding(5) var env_cube: texture_cube<f32>;
@group(0) @binding(6) var env_smp: sampler;
@group(0) @binding(7) var shadow_tex: texture_depth_2d;
@group(0) @binding(8) var shadow_smp: sampler_comparison;
// 1 = open, 0 = fully occluded.
@group(0) @binding(9) var ao_tex: texture_2d<f32>;
// Eye depth of the nearest back face under each pixel, 0 = none.
@group(0) @binding(10) var back_tex: texture_2d<f32>;
@group(0) @binding(11) var hdri_tex: texture_2d<f32>;
@group(0) @binding(12) var hdri_smp: sampler;

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
    return vec4<f32>(color, 1.0);
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
    // Ambient occlusion (1 = open), and what direct light keeps of it.
    ao: f32,
    ao_direct: f32,
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

// Soft shadow of the key light: a 7x7 tent over the shadow map.
fn key_shadow(world: vec3<f32>) -> f32 {
    if (look.light_params.y < 0.5) {
        return 1.0;
    }
    let clip = look.shadow_view_proj * vec4<f32>(world, 1.0);
    let pos = vec3<f32>(clip.x * 0.5 + 0.5, 0.5 - clip.y * 0.5, clip.z);
    if (pos.z <= 0.0 || pos.z >= 1.0 || any(pos.xy <= vec2<f32>(0.0)) || any(pos.xy >= vec2<f32>(1.0))) {
        return 1.0;
    }
    let texel = look.shadow_params.x;
    let uv = pos.xy / texel;
    var base = floor(uv + 0.5);
    let s = uv.x + 0.5 - base.x;
    let t = uv.y + 0.5 - base.y;
    base = (base - 0.5) * texel;

    let uw = vec4<f32>(5.0 * s - 6.0, 11.0 * s - 28.0, -(11.0 * s + 17.0), -(5.0 * s + 1.0));
    let u = vec4<f32>(
        (4.0 * s - 5.0) / uw.x - 3.0,
        (4.0 * s - 16.0) / uw.y - 1.0,
        -(7.0 * s + 5.0) / uw.z + 1.0,
        -s / uw.w + 3.0);
    let vw = vec4<f32>(5.0 * t - 6.0, 11.0 * t - 28.0, -(11.0 * t + 17.0), -(5.0 * t + 1.0));
    let v = vec4<f32>(
        (4.0 * t - 5.0) / vw.x - 3.0,
        (4.0 * t - 16.0) / vw.y - 1.0,
        -(7.0 * t + 5.0) / vw.z + 1.0,
        -t / vw.w + 3.0);
    var sum = 0.0;
    for (var j = 0; j < 4; j++) {
        for (var i = 0; i < 4; i++) {
            let at = base + vec2<f32>(u[i], v[j]) * texel;
            sum += uw[i] * vw[j] * textureSampleCompareLevel(shadow_tex, shadow_smp, at, pos.z);
        }
    }
    // Shadow strength: how dark the preset lets the key's shadow get.
    return mix(1.0, sum / 2704.0, look.light_params.z);
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
    // Only the key casts shadows.
    l.shadow = select(1.0, s.shadow, i == 0);
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

// The surroundings mirrored in a surface of the given roughness.
fn environment(r: vec3<f32>, perceptual_roughness: f32) -> vec3<f32> {
    let level = perceptual_roughness * (1.7 - 0.7 * perceptual_roughness) * 6.0;
    return textureSampleLevel(env_cube, env_smp, r, level).rgb * look.env_params.y;
}

// Physically based shading under the whole rig: the Unity pipeline's lit
// model (a GGX highlight in its cheap normalised form, ambient from the sky
// colours or the HDRI, a blurred environment reflection).
fn physically_shade(s: Surface, n: vec3<f32>, albedo: vec3<f32>, metallic: f32, smoothness: f32, occlusion: f32) -> vec3<f32> {
    let one_minus_reflectivity = 0.96 * (1.0 - metallic);
    let reflectivity = 1.0 - one_minus_reflectivity;
    let diffuse = albedo * one_minus_reflectivity;
    let specular = mix(vec3<f32>(0.04), albedo, metallic);
    let perceptual_roughness = 1.0 - smoothness;
    let roughness = max(perceptual_roughness * perceptual_roughness, 0.0078125);
    let roughness2 = max(roughness * roughness, 6.103515625e-5);
    let grazing = saturate(smoothness + reflectivity);
    let normalization = roughness * 4.0 + 2.0;

    // Indirect.
    let nv = saturate(dot(n, s.v));
    let fresnel = pow(1.0 - nv, 4.0);
    let reflection = environment(reflect(-s.v, n), perceptual_roughness);
    var color = ambient(n) * diffuse
        + reflection * (1.0 / (roughness2 + 1.0)) * mix(specular, vec3<f32>(grazing), fresnel);
    color *= min(s.ao, occlusion);

    // Direct.
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, s);
        let nl = saturate(dot(n, light.dir));
        let h = normalize(light.dir + s.v);
        let nh = saturate(dot(n, h));
        let lh = saturate(dot(light.dir, h));
        let d = nh * nh * (roughness2 - 1.0) + 1.00001;
        let spec = roughness2 / (d * d * max(0.1, lh * lh) * normalization);
        color += (diffuse + specular * spec) * light.color * (s.ao_direct * light.shadow * nl);
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
    let fw = fwidth(phase);
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

// A line where v crosses zero, `width` wide and hottest at its centre.
// `fw` is how much v changes across a pixel: the line is widened to at least
// a pixel and dimmed to match, so a thin line fades out as the camera pulls
// back rather than breaking into dashes.
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
        let warp = vec3<f32>(
            value_noise(p * 0.6 + vec3<f32>(0.0, t * 0.05, 0.0)),
            value_noise(p * 0.6 + vec3<f32>(13.1, 0.0, t * 0.05)),
            value_noise(p * 0.6 + vec3<f32>(t * 0.05, 27.3, 5.7))) - 0.5;
        let q = p + warp * 0.8;
        let cells = glow_cells(q);
        let edge = cells.f2 - cells.f1;
        let crack = glow_line(edge, fwidth(edge), 0.04 + 0.12 * coverage);
        // Each stretch of crack flares and fades on its own slow cycle.
        let flare = 0.6 + 0.4 * value_noise(q * 1.3 + vec3<f32>(0.0, 0.0, t * 0.3));
        let pool = smoothstep(0.35 - 0.4 * coverage, 0.75 - 0.4 * coverage, -cv);
        m = max(crack * flare, pool * 0.85);
        heat = max(crack * crack * flare, pool * 0.6);
    } else if (pattern == 3) {
        // Veins: the zero-crossings of two drifting noise fields.
        let n1 = fbm(p + vec3<f32>(0.0, t * 0.25, 0.0)) - 0.5;
        let n2 = fbm(p * 1.9 + vec3<f32>(t * 0.2, 7.7, 3.1)) - 0.5;
        let width = 0.02 + 0.05 * coverage;
        m = max(glow_line(n1, fwidth(n1), width), glow_line(n2, fwidth(n2), width * 0.7) * 0.7);
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
        let scan = glow_line(scan_v, fwidth(scan_v), 0.04 + 0.12 * coverage);
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
    return finish_pbr(s, look.finish);
}

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    let front = is_front(handedness(in.model_x, in.model_y, in.model_z), facing);
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
    s.smooth_n = smooth_n;
    s.curv = in.curv;
    s.base_color = g.base_color.rgb;
    s.shadow = 1.0;
    s.ao = 1.0;
    s.ao_direct = 1.0;
    s.back_depth = 0.0;

    // Surface grain: a small bump straight from the object-space position,
    // which reads through a matcap exactly as it does under real lights.
    if (look.base_params.z > 0.0001) {
        n = perturb_normal(n, in.local * look.base_params.w, look.base_params.z);
    }
    s.n = n;

    var color: vec3<f32>;
    if (FINISH == FINISH_MATCAP) {
        color = matcap_shade(n);
    } else {
        s.shadow = key_shadow(in.world);
        if (look.env_params.z > 0.5) {
            s.ao = textureLoad(ao_tex, s.pixel, 0).r;
            s.ao_direct = mix(1.0, s.ao, look.env_params.w);
        }
        if (look.matcap_params.w > 0.5) {
            s.back_depth = textureLoad(back_tex, s.pixel, 0).r;
        }
        color = light_sample(s, surface_material(s));
    }

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
