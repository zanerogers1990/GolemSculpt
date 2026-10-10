// Viewport shading, second half: the surface itself. A port of the Unity
// app's SculptPBR shader - physically based lighting under the preset rig,
// matcaps, the lure plastic / aged metal / clay / wood finishes and the glow
// layer - on top of the inputs and helpers in shader.wgsl.

const MAX_LIGHTS: i32 = 5;
const TAU: f32 = 6.2831853;

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
    // x metallic, z normal detail strength, w normal detail scale
    base_params: vec4<f32>,
    // x intensity, y tint by base colour, z finish (0 none, 1 plastic,
    // 2 metal, 3 clay, 4 wood), w back-face depth buffer is valid
    matcap_params: vec4<f32>,
    mask_tint: vec4<f32>,

    plastic_thin: vec4<f32>,
    plastic_thick: vec4<f32>,
    // x absorption depth, y transmission, z gloss
    plastic_params: vec4<f32>,
    flake_a: vec4<f32>,
    flake_b: vec4<f32>,
    flake_c: vec4<f32>,
    micro_color: vec4<f32>,
    // x cell size, y density, z shape, w tilt
    flake_p0: vec4<f32>,
    // x sparkle, y micro cell size, z micro density
    flake_p1: vec4<f32>,

    metal_color: vec4<f32>,
    coat_a: vec4<f32>,
    coat_b: vec4<f32>,
    wash_color: vec4<f32>,
    metal_offset: vec4<f32>,
    // x metal smoothness, y metal metallic, z coat metallic, w coat smoothness
    metal_p0: vec4<f32>,
    // x wash, y coverage, z edge wear, w patch size
    metal_p1: vec4<f32>,
    // x patch contrast, y patch amount, z coat follows recesses, w grain size
    metal_p2: vec4<f32>,
    // x grain, y detail, z patch breakup, w sparkle
    metal_p3: vec4<f32>,
    // x undulation, y undulation size
    metal_p4: vec4<f32>,

    clay_color: vec4<f32>,
    clay_recess: vec4<f32>,
    clay_scatter: vec4<f32>,
    // x smoothness, y wetness, z wet power, w subsurface
    clay_p0: vec4<f32>,
    // x recess, y ridge, z mottle, w mottle size
    clay_p1: vec4<f32>,
    // x grain, y grain size, z detail
    clay_p2: vec4<f32>,

    wood_early: vec4<f32>,
    wood_late: vec4<f32>,
    wood_worn: vec4<f32>,
    wood_recess: vec4<f32>,
    wood_scatter: vec4<f32>,
    wood_axis: vec4<f32>,
    wood_pith: vec4<f32>,
    // x ring size, y ring contrast, z latewood fraction, w wobble
    wood_p0: vec4<f32>,
    // x figure size, y streaks, z streak size, w pores
    wood_p1: vec4<f32>,
    // x end grain, y patina, z wear, w smoothness
    wood_p2: vec4<f32>,
    // x wax, y wax power, z subsurface, w checks
    wood_p3: vec4<f32>,
    // x detail
    wood_p4: vec4<f32>,

    glow_hot: vec4<f32>,
    glow_cool: vec4<f32>,
    // x on, y pattern, z intensity, w coverage
    glow_p0: vec4<f32>,
    // x rim, y pattern size, z speed, w pulse
    glow_p1: vec4<f32>,
    // x pulse rate, y dim
    glow_p2: vec4<f32>,
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

// Everything the shading of one pixel shares.
struct Px {
    world: vec3<f32>,
    // Toward the eye.
    v: vec3<f32>,
    pixel: vec2<i32>,
    // Position in the object's own space at world size, and how much of it
    // one pixel covers: patterns live here so they stay on the model.
    p: vec3<f32>,
    footprint: f32,
    scale: vec3<f32>,
    mx: vec3<f32>,
    my: vec3<f32>,
    mz: vec3<f32>,
    // Smooth normal (never the faceted one).
    smooth_n: vec3<f32>,
    curv: f32,
    // Light reaching the surface from the key: 1 lit, 0 in its shadow.
    shadow: f32,
    // Ambient occlusion (1 = open), and what direct light keeps of it.
    ao: f32,
    ao_direct: f32,
};

fn to_world_dir(px: Px, v: vec3<f32>) -> vec3<f32> {
    return normalize(px.mx * v.x + px.my * v.y + px.mz * v.z);
}

// A world direction in the object's space (not renormalised).
fn to_object_dir(px: Px, v: vec3<f32>) -> vec3<f32> {
    let cx = cross(px.my, px.mz);
    let cy = cross(px.mz, px.mx);
    let cz = cross(px.mx, px.my);
    let det = dot(px.mx, cx);
    return vec3<f32>(dot(cx, v), dot(cy, v), dot(cz, v)) / det;
}

fn to_object_normal(px: Px, n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(px.mx, n), dot(px.my, n), dot(px.mz, n));
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

fn get_light(i: i32, px: Px) -> Light {
    var l: Light;
    l.dir = look.light_dir[i].xyz;
    l.color = look.light_color[i].rgb;
    // Only the key casts shadows.
    l.shadow = select(1.0, px.shadow, i == 0);
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
fn physically_shade(px: Px, n: vec3<f32>, albedo: vec3<f32>, metallic: f32, smoothness: f32, occlusion: f32) -> vec3<f32> {
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
    let nv = saturate(dot(n, px.v));
    let fresnel = pow(1.0 - nv, 4.0);
    let reflection = environment(reflect(-px.v, n), perceptual_roughness);
    var color = ambient(n) * diffuse
        + reflection * (1.0 / (roughness2 + 1.0)) * mix(specular, vec3<f32>(grazing), fresnel);
    color *= min(px.ao, occlusion);

    // Direct.
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, px);
        let nl = saturate(dot(n, light.dir));
        let h = normalize(light.dir + px.v);
        let nh = saturate(dot(n, h));
        let lh = saturate(dot(light.dir, h));
        let d = nh * nh * (roughness2 - 1.0) + 1.00001;
        let spec = roughness2 / (d * d * max(0.1, lh * lh) * normalization);
        color += (diffuse + specular * spec) * light.color * (px.ao_direct * light.shadow * nl);
    }
    return color;
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
fn trace_flakes(px: Px, origin: vec3<f32>, dir: vec3<f32>, normal: vec3<f32>,
                cell_size: f32, density: f32, seed: f32, palette: bool) -> FlakeHit {
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
    let fp = max(px.footprint / cell_size, 1e-4);
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
        let nf = normalize(n + (h2 * 2.0 - 1.0) * look.flake_p0.w);

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
        let s = sin(angle);
        let c = cos(angle);
        let uv = vec2<f32>(dot(q, ta), dot(q, tb));
        let a = abs(vec2<f32>(uv.x * c - uv.y * s, uv.x * s + uv.y * c));
        let hex_dist = max(a.x * 0.8660254 + a.y * 0.5, a.y);
        let square_dist = max(a.x, a.y);
        let dist = mix(hex_dist, square_dist, look.flake_p0.z);
        // Antialiased over one pixel, and a flake smaller than a pixel faded
        // to its share of it, or zooming out turns glitter into crawling noise.
        let coverage = saturate((radius - dist) / fp + 0.5) * saturate(2.0 * radius / fp) * lod_fade;
        if (coverage <= 0.01) {
            continue;
        }
        best_t = t;
        hit.depth = t * cell_size;
        hit.coverage = coverage;
        hit.normal = to_world_dir(px, flake_unrot(nf) / px.scale);
        if (palette) {
            // Colour weights ride in the alpha channels.
            let total = max(look.flake_a.a + look.flake_b.a + look.flake_c.a, 1e-4);
            let pick = h0.z * total;
            if (pick < look.flake_a.a) {
                hit.color = look.flake_a.rgb;
            } else if (pick < look.flake_a.a + look.flake_b.a) {
                hit.color = look.flake_b.rgb;
            } else {
                hit.color = look.flake_c.rgb;
            }
        } else {
            hit.color = look.micro_color.rgb;
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

fn flake_radiance(px: Px, color: vec3<f32>, n: vec3<f32>, diffuse: vec3<f32>, glint: vec3<f32>) -> vec3<f32> {
    let env = environment(reflect(-px.v, n), 0.3);
    return color * (diffuse + ambient(n)) * 0.7
        + color * env * 0.6
        + mix(color, vec3<f32>(1.0), 0.3) * glint * look.flake_p1.x;
}

// Colour left after light crosses `distance` of plastic twice (down to a
// flake and back up).
fn plastic_transmittance(distance: f32, depth_scale: f32) -> vec3<f32> {
    return pow(max(look.plastic_thick.rgb, vec3<f32>(1e-3)), vec3<f32>(2.0 * distance / depth_scale));
}

fn plastic_shade(px: Px, n: vec3<f32>) -> vec3<f32> {
    let depth_scale = max(look.plastic_params.x, 1e-5);

    // How much plastic the view ray crosses here. With no buffer (or nothing
    // behind): treat it as solid body.
    var thickness = depth_scale * 4.0;
    if (look.matcap_params.w > 0.5) {
        let back = textureLoad(back_tex, px.pixel, 0).r;
        let front = dot(px.world - g.cam_pos.xyz, g.cam_forward.xyz);
        // A back face in front of this surface means an open or intersecting
        // mesh: the difference means nothing there.
        if (back > 0.0 && back >= front - depth_scale * 0.02) {
            thickness = max(back - front, 0.0);
        }
    }
    let through = exp(-thickness / depth_scale);
    var body = mix(look.plastic_thick.rgb, look.plastic_thin.rgb, through);

    // The smooth normal steers the flakes even under flat shading, so a
    // flake does not break along triangle edges.
    let dir_os = normalize(to_object_dir(px, refract(-px.v, px.smooth_n, 1.0 / 1.5)) * px.scale);
    let surface_os = normalize(to_object_normal(px, px.smooth_n) / px.scale);
    let big = trace_flakes(px, px.p, dir_os, surface_os, look.flake_p0.x, look.flake_p0.y, 0.0, true);
    let micro = trace_flakes(px, px.p, dir_os, surface_os, look.flake_p1.y, look.flake_p1.z, 131.0, false);
    let weights = max(look.flake_a.a + look.flake_b.a + look.flake_c.a, 1e-4);
    let palette_mean = (look.flake_a.rgb * look.flake_a.a + look.flake_b.rgb * look.flake_b.a
        + look.flake_c.rgb * look.flake_c.a) / weights;
    body = faded_flake_tint(body, palette_mean, px.footprint, look.flake_p0.x, look.flake_p0.y);
    body = faded_flake_tint(body, look.micro_color.rgb, px.footprint, look.flake_p1.y, look.flake_p1.z);
    let n_big = face_viewer(big.normal, px.v);
    let n_micro = face_viewer(micro.normal, px.v);

    var diffuse_a = vec3<f32>(0.0);
    var glint_a = vec3<f32>(0.0);
    var diffuse_b = vec3<f32>(0.0);
    var glint_b = vec3<f32>(0.0);
    var back_light = vec3<f32>(0.0);
    var scatter_light = vec3<f32>(0.0);
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, px);
        let shadowed = light.color * light.shadow;
        diffuse_a += shadowed * saturate(dot(n_big, light.dir));
        glint_a += shadowed * flake_glint(n_big, light.dir, px.v);
        diffuse_b += shadowed * saturate(dot(n_micro, light.dir));
        // Pepper flakes are specks, not mirrors.
        glint_b += shadowed * flake_glint(n_micro, light.dir, px.v) * 0.35;
        // Light entering the far side and leaving toward the eye. Unshadowed
        // on purpose: this is exactly the light passing through the model.
        let through_dir = normalize(light.dir + n * 0.35);
        back_light += light.color * pow(saturate(dot(px.v, -through_dir)), 3.0);
        // Light scattered inside the plastic reaches past the shadow line:
        // the extra a wrapped Lambert gets over the plain one.
        let ndl = dot(n, light.dir);
        let wrapped = saturate((ndl + 0.6) / 1.6);
        scatter_light += light.color * mix(1.0, light.shadow, 0.5) * max(wrapped - saturate(ndl), 0.0);
    }

    // Front to back: whichever flake is shallower covers the other.
    var w_big = big.coverage;
    var w_micro = micro.coverage;
    if (micro.depth < big.depth) {
        w_big *= 1.0 - w_micro;
    } else {
        w_micro *= 1.0 - w_big;
    }
    let t_big = plastic_transmittance(big.depth, depth_scale);
    let t_micro = plastic_transmittance(micro.depth, depth_scale);
    let flakes = w_big * t_big * flake_radiance(px, big.color, n_big, diffuse_a, glint_a)
        + w_micro * t_micro * flake_radiance(px, micro.color, n_micro, diffuse_b, glint_b);
    // A flake blocks the plastic behind it, by as much of it as shows through.
    let occluded = saturate(w_big * luminance(t_big) + w_micro * luminance(t_micro));

    var color = physically_shade(px, n, body * (1.0 - occluded), 0.0, look.plastic_params.z, 1.0);
    let transmission = look.plastic_params.y;
    let glow = look.plastic_thin.rgb * through * (back_light + ambient(-n) * 0.6) * transmission;
    // Scattered light leaves tinted by the plastic it wandered through.
    let scatter = scatter_light * mix(body, look.plastic_thin.rgb, 0.3) * transmission * 0.5;
    color += flakes + (glow + scatter) * (1.0 - occluded);
    return color;
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

fn metal_shade(px: Px, n_in: vec3<f32>) -> vec3<f32> {
    var n = n_in;
    let p = px.p;
    let cv = surface_convexity(px.curv, look.metal_p3.y);
    // The offset slides the model through the noise, moving every blotch.
    let patch_p = p / max(look.metal_p1.w, 1e-5) + look.metal_offset.xyz;
    var patches = fbm(patch_p);
    let tint = fbm(patch_p * 2.7 + 41.7);
    // Grain finer than a pixel is only shimmer: fade it out as it gets there.
    let grain_size = max(look.metal_p2.w, 1e-6);
    let grain_lod = saturate(1.5 - px.footprint / grain_size);
    let grain = value_noise(p / grain_size + 7.3);
    let breakup = look.metal_p3.z;
    if (breakup > 0.001) {
        // Etched steel goes dull in lacy clusters, not soft blobs.
        patches += ((fbm(patch_p * 3.7 + 11.3) - 0.5) + (grain - 0.5) * 0.4 * grain_lod) * breakup;
    }

    // Worn to bare metal along the ridges, broken up by the patches.
    let wear = saturate(smoothstep(0.1, 0.55, cv) * look.metal_p1.z * (0.55 + 0.9 * patches));
    // The coat covers wherever the patch noise sits under the coverage
    // level, and recesses hold more of it than raised areas.
    let coat_level = look.metal_p1.y - cv * look.metal_p2.z;
    let patch_level = mix(0.5, patches, look.metal_p2.y);
    let coat = saturate((coat_level - patch_level) * look.metal_p2.x + 0.5) * (1.0 - wear);
    let wash = smoothstep(0.03, 0.6, -cv) * look.metal_p1.x;

    var coat_color = mix(look.coat_a.rgb, look.coat_b.rgb, saturate((tint - 0.5) * 2.5 + 0.5 - cv * 0.5));
    coat_color *= mix(1.0, mix(0.7, 1.2, grain), look.metal_p3.x * grain_lod);
    var metal_color = look.metal_color.rgb * mix(0.88, 1.08, tint);
    let sparkle = look.metal_p3.w;
    if (sparkle > 0.001) {
        // Fine bright specks: the tops of the grain left between pits.
        let sparkle_lod = saturate(1.5 - px.footprint * 2.0 / grain_size);
        let speck = smoothstep(0.4, 0.9, value_noise(p / grain_size * 2.0 + 19.7));
        metal_color *= mix(1.0, 0.82 + 0.6 * speck, sparkle * sparkle_lod);
    }

    var albedo = mix(metal_color, coat_color, coat);
    var metallic = mix(look.metal_p0.y, look.metal_p0.z, coat);
    var smoothness = mix(look.metal_p0.x, look.metal_p0.w, coat);
    albedo = mix(albedo, look.wash_color.rgb, wash);
    metallic *= 1.0 - wash;
    smoothness *= 1.0 - 0.35 * wash;

    // Hand-worked metal is never optically flat: a slow ripple that bends
    // the reflections the way hammering and filing leave a surface.
    let undulation = look.metal_p4.x;
    if (undulation > 0.001) {
        let ripple_p = p / max(look.metal_p4.y, 1e-5) + look.metal_offset.xyz * 0.37;
        n = perturb_normal(n, ripple_p, undulation * 0.35);
        n = perturb_normal(n, ripple_p * 2.3 + 5.1, undulation * 0.2);
    }
    // Pitted where coated, much less on the polished metal.
    let bump = look.metal_p3.x * grain_lod * mix(0.25, 1.0, coat);
    if (bump > 0.001) {
        n = perturb_normal(n, p / grain_size, bump * 0.6);
    }
    return physically_shade(px, n, albedo, metallic, smoothness, 1.0 - 0.5 * wash);
}

// ---- sculptor's clay ------------------------------------------------------
//
// Oil clay reads as clay rather than grey plastic through a satin sheen, a
// thin wet highlight riding on it, light bleeding a little past the shadow
// line, and recesses darker and richer than the flats.

fn clay_shade(px: Px, n_in: vec3<f32>) -> vec3<f32> {
    var n = n_in;
    let p = px.p;
    let cv = surface_convexity(px.curv, look.clay_p2.z);

    // Clay is never one flat colour: a broad, faint drift in tone.
    let mottle = fbm(p / max(look.clay_p1.w, 1e-5));
    var albedo = look.clay_color.rgb * (1.0 + (mottle - 0.5) * 2.0 * look.clay_p1.z);

    let recess = saturate(smoothstep(0.0, 0.45, -cv) * look.clay_p1.x);
    let ridge = smoothstep(0.1, 0.6, cv) * look.clay_p1.y;
    albedo = mix(albedo, look.clay_recess.rgb, recess);
    albedo *= 1.0 + 0.25 * ridge;
    let smoothness = saturate(look.clay_p0.x + 0.15 * ridge - 0.2 * recess);

    // Fine grit in the clay body, faded out before it is smaller than a pixel.
    let grain_size = max(look.clay_p2.y, 1e-6);
    let grain = look.clay_p2.x * saturate(1.5 - px.footprint / grain_size);
    if (grain > 0.001) {
        n = perturb_normal(n, p / grain_size, grain * 0.12);
        let speck = value_noise(p / grain_size * 1.7 + 11.3);
        albedo *= mix(1.0, mix(0.9, 1.06, speck), saturate(grain));
    }

    var color = physically_shade(px, n, albedo, 0.0, smoothness, 1.0 - 0.6 * recess);

    var wet = vec3<f32>(0.0);
    var scatter = vec3<f32>(0.0);
    let wet_power = look.clay_p0.z;
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, px);
        let ndl = dot(n, light.dir);
        // A sharp lobe that keeps its energy, with a dielectric's fresnel:
        // a film of wet, lit on the grain-bumped normal so it breaks up.
        let h = normalize(light.dir + px.v);
        let spec = pow(saturate(dot(n, h)), wet_power) * (wet_power + 8.0) / 8.0;
        let fresnel = 0.04 + 0.96 * pow(1.0 - saturate(dot(px.v, h)), 5.0);
        wet += light.color * light.shadow * (spec * fresnel * saturate(ndl));
        // Scattered light gets a little past the terminator.
        let wrapped = saturate((ndl + 0.5) / 1.5);
        scatter += light.color * mix(1.0, light.shadow, 0.6) * max(wrapped - saturate(ndl), 0.0);
    }
    // The wet film thins out in the recesses, where the tools did not drag it.
    color += wet * look.clay_p0.y * (1.0 - 0.7 * recess)
        + scatter * look.clay_scatter.rgb * look.clay_p0.w;
    // Recesses see less of the lights as well as less sky.
    color *= 1.0 - 0.35 * recess;
    return color;
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

fn wood_shade(px: Px, n_in: vec3<f32>) -> vec3<f32> {
    var n = n_in;
    let p = px.p;
    let cv = surface_convexity(px.curv, look.wood_p4.x);

    // The log's frame: `a` along the grain, (u, w) across it.
    let a = normalize(look.wood_axis.xyz);
    let u = normalize(cross(a, select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 1.0, 0.0), abs(a.y) < 0.9)));
    let w = cross(a, u);
    let q = p - look.wood_pith.xyz;
    let along = dot(q, a);
    let radial = vec2<f32>(dot(q, u), dot(q, w));
    let radius = length(radial);

    // Growth rings. A trunk is never a true cylinder: low-frequency noise
    // pushes the rings in and out, stretched along the axis so they stay
    // coherent along the grain - which draws the long flames.
    let ring_size = max(look.wood_p0.x, 1e-6);
    let fig_size = max(look.wood_p1.x, 1e-5);
    let sharp = look.wood_p0.z;
    let wob = fbm(vec3<f32>(radial.x, along * 0.3, radial.y) / fig_size) * 2.0 - 1.0;
    var phase = (radius + wob * look.wood_p0.w * ring_size * 4.0) / ring_size;
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
    var wood = mix(look.wood_early.rgb, look.wood_late.rgb, saturate(late * look.wood_p0.y));

    // Fibres and pores, faded before they are smaller than a pixel.
    let streaks = look.wood_p1.y;
    let streak_size = max(look.wood_p1.z, 1e-6);
    let streak_lod = saturate(1.5 - px.footprint / streak_size);
    let streak = wood_streak_height(q, a, u, w, streak_size);
    wood *= 1.0 + (streak - 0.5) * 0.7 * streaks * streak_lod;
    let pores = look.wood_p1.w;
    if (pores > 0.001) {
        // Ring-porous: big vessels in the earlywood, cut open as dark dashes.
        let ps = vec3<f32>(radial.x, radial.y, along / 6.0) / (streak_size * 0.6);
        let pore = smoothstep(0.7, 0.82, value_noise(ps + 13.7)) * (1.0 - late)
            * saturate(1.5 - px.footprint / (streak_size * 0.6));
        wood *= 1.0 - 0.55 * saturate(pore * pores);
    }

    // End grain drinks the finish and goes darker.
    let surface_s = normalize(to_object_normal(px, px.smooth_n) / px.scale);
    let end_grain = smoothstep(0.55, 0.95, abs(dot(surface_s, a))) * look.wood_p2.x;
    wood *= 1.0 - 0.35 * end_grain;

    // Age: grime in the recesses, rubbed-back edges on the high points.
    let wear_noise = fbm(p / fig_size * 3.0 + 23.1);
    let wear = saturate(smoothstep(0.12, 0.55, cv) * look.wood_p2.z * (0.55 + 0.9 * wear_noise));
    // The worn colour keeps the figure: scaled by how light this bit of wood is.
    let wood_lum = luminance(wood) / max(luminance(look.wood_early.rgb), 1e-3);
    var albedo = mix(wood, look.wood_worn.rgb * wood_lum, wear);
    let recess = saturate(smoothstep(0.0, 0.4, -cv) * look.wood_p2.y);
    albedo = mix(albedo, look.wood_recess.rgb, recess);

    // Checks: seasoning cracks, radial from the pith, running along the grain.
    var crack = 0.0;
    let checks = look.wood_p3.w;
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
            let fp = max(px.footprint, 1e-6);
            crack = saturate((width - dist) / fp + 0.5) * saturate(width * 2.0 / fp) * runs;
        }
    }
    albedo = mix(albedo, look.wood_recess.rgb * 0.5, crack);

    let smoothness = saturate(look.wood_p2.w + 0.12 * wear - 0.3 * recess) * (1.0 - crack);

    // Fibres raise the surface a hair, which breaks the sheen along the grain.
    let bump = streaks * streak_lod;
    if (bump > 0.001) {
        let e = streak_size * 0.35;
        let grad = vec3<f32>(
            wood_streak_height(q + vec3<f32>(e, 0.0, 0.0), a, u, w, streak_size) - streak,
            wood_streak_height(q + vec3<f32>(0.0, e, 0.0), a, u, w, streak_size) - streak,
            wood_streak_height(q + vec3<f32>(0.0, 0.0, e), a, u, w, streak_size) - streak) / e;
        let grad_ws = px.mx * (grad.x / px.scale.x) + px.my * (grad.y / px.scale.y) + px.mz * (grad.z / px.scale.z);
        n = normalize(n - (grad_ws - n * dot(grad_ws, n)) * (bump * 0.05 * streak_size));
    }

    var color = physically_shade(px, n, albedo, 0.0, smoothness, 1.0 - 0.6 * recess);

    var wax = vec3<f32>(0.0);
    var scatter = vec3<f32>(0.0);
    let wax_power = look.wood_p3.y;
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let light = get_light(i, px);
        let ndl = dot(n, light.dir);
        // As the clay's wet film, but broad: rubbed wax is a soft glow.
        let h = normalize(light.dir + px.v);
        let spec = pow(saturate(dot(n, h)), wax_power) * (wax_power + 8.0) / 8.0;
        let fresnel = 0.04 + 0.96 * pow(1.0 - saturate(dot(px.v, h)), 5.0);
        wax += light.color * light.shadow * (spec * fresnel * saturate(ndl));
        let wrapped = saturate((ndl + 0.4) / 1.4);
        scatter += light.color * mix(1.0, light.shadow, 0.6) * max(wrapped - saturate(ndl), 0.0);
    }
    // Wax builds up where hands polish it and is dull with grime in the recesses.
    let wax_amount = look.wood_p3.x * (1.0 - 0.8 * recess) * (0.7 + 0.6 * wear) * (1.0 - crack);
    color += wax * wax_amount + scatter * look.wood_scatter.rgb * look.wood_p3.z * (1.0 - recess);
    color *= 1.0 - 0.35 * recess;
    return color;
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

fn glow_emission(px: Px, n: vec3<f32>) -> vec3<f32> {
    let pattern_size = max(look.glow_p1.y, 1e-5);
    let p = px.p / pattern_size;
    let t = look.light_params.w * look.glow_p1.z;
    let facing = saturate(dot(n, px.v));
    let rim = pow(1.0 - facing, 3.0);
    let cv = surface_convexity(px.curv, 1.0);
    let coverage = look.glow_p0.w;
    let pattern = i32(look.glow_p0.y + 0.5);

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
        let y = px.world.y / pattern_size;
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
        let lod = saturate(1.5 - px.footprint / pattern_size * 2.0);
        m = mix(0.08 * lit, spot * lit * beat, lod);
        heat = spot;
    } else {
        // Inner light: shining out of every recess, shimmering a little.
        let level = 0.3 - 0.5 * coverage;
        let pool = smoothstep(level, level + 0.45, -cv);
        m = pool * (0.8 + 0.4 * fbm(p + vec3<f32>(0.0, t * 0.2, 0.0)));
        heat = pool;
    }

    let rim_amount = look.glow_p1.x;
    m += rim * rim_amount;
    heat = max(heat, rim * rim_amount);
    // Breathing: down to half brightness at full depth.
    m *= mix(1.0, 0.5 + 0.5 * sin(t * TAU * look.glow_p2.x), look.glow_p1.w);

    let color = mix(look.glow_cool.rgb, look.glow_hot.rgb, saturate(heat));
    return color * (m * look.glow_p0.z);
}

// ---- the surface ----------------------------------------------------------

@fragment
fn fs_mesh(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    var n = shading_normal(in, front);
    let smooth_n = select(-normalize(in.normal), normalize(in.normal), front);

    var px: Px;
    px.world = in.world;
    px.v = normalize(g.cam_pos.xyz - in.world);
    if (g.viewport.z <= 0.0) {
        px.v = -g.cam_forward.xyz;
    }
    px.pixel = vec2<i32>(in.pos.xy);
    px.mx = in.model_x;
    px.my = in.model_y;
    px.mz = in.model_z;
    // The object's scale along each of its own axes, so a squashed object
    // gets patterns of the same size rather than squashed ones.
    px.scale = vec3<f32>(length(px.mx), length(px.my), length(px.mz));
    px.p = in.local * px.scale;
    px.footprint = length(fwidth(px.p));
    px.smooth_n = smooth_n;
    px.curv = in.curv;

    let matcap = g.shading.x > 0.5;
    let finish = i32(look.matcap_params.z + 0.5);

    // Surface grain: a small bump straight from the object-space position,
    // which reads through a matcap exactly as it does under real lights.
    if (look.base_params.z > 0.0001) {
        n = perturb_normal(n, in.local * look.base_params.w, look.base_params.z);
    }

    var color: vec3<f32>;
    if (matcap) {
        color = matcap_shade(n);
    } else {
        px.shadow = key_shadow(in.world);
        px.ao = 1.0;
        px.ao_direct = 1.0;
        if (look.env_params.z > 0.5) {
            px.ao = textureLoad(ao_tex, px.pixel, 0).r;
            px.ao_direct = mix(1.0, px.ao, look.env_params.w);
        }
        if (finish == 1) {
            color = plastic_shade(px, n);
        } else if (finish == 2) {
            color = metal_shade(px, n);
        } else if (finish == 3) {
            color = clay_shade(px, n);
        } else if (finish == 4) {
            color = wood_shade(px, n);
        } else {
            color = physically_shade(px, n, g.base_color.rgb, look.base_params.x, g.base_color.w, 1.0);
        }
    }

    // Cavity multiplies the finished colour - over a matcap's baked lighting
    // or the lit result alike - and before the mask tint, so a masked crease
    // still reads as a crease.
    color *= cavity(in.pos.xy);

    // Glow goes on after cavity (a crease cavity darkens is exactly where
    // lava should shine) but before the mask tint.
    if (look.glow_p0.x > 0.5) {
        color = color * (1.0 - look.glow_p2.y) + glow_emission(px, n);
    }

    if (!front) {
        // Inside of the surface: dim and cool, so holes and flipped faces read.
        color = mix(color * 0.35, vec3<f32>(0.05, 0.07, 0.12), 0.5);
    }
    // Mask: darkens the fully lit result rather than the colour fed into
    // lighting, so highlights dim too.
    color = mix(color, color * look.mask_tint.rgb, saturate(in.mask * look.mask_tint.a));

    // Mirror plane guides: a thin tinted line where each plane cuts the surface.
    let w = fwidth(in.local);
    var axis_color = array<vec3<f32>, 3>(
        vec3<f32>(1.0, 0.05, 0.05), vec3<f32>(0.1, 1.0, 0.1), vec3<f32>(0.08, 0.26, 1.0));
    for (var i = 0; i < 3; i++) {
        if (g.symmetry[i] > 0.5 && in.is_active > 0.5) {
            let d = abs(in.local[i]) / max(w[i], 1e-9);
            let guide = 1.0 - smoothstep(0.4, 1.1, d);
            color = mix(color, axis_color[i], guide * 0.35);
        }
    }
    return vec4<f32>(color, 1.0);
}

// ---- helper passes --------------------------------------------------------

// The scene from the key light, for its shadow. Casters are pushed a little
// away from the light and shrunk along their normals where they turn from
// it, which keeps a surface from shadowing itself.
@vertex
fn vs_shadow(in: VsIn) -> @builtin(position) vec4<f32> {
    let x = in.model_x.xyz;
    let y = in.model_y.xyz;
    let z = in.model_z.xyz;
    var world = x * in.position.x + y * in.position.y + z * in.position.z + in.model_w.xyz;
    let n = normalize(cross(y, z) * in.normal.x + cross(z, x) * in.normal.y + cross(x, y) * in.normal.z);
    let l = look.light_dir[0].xyz;
    world -= l * look.shadow_params.y;
    world -= n * ((1.0 - saturate(dot(l, n))) * look.shadow_params.z);
    return look.shadow_view_proj * vec4<f32>(world, 1.0);
}

// Eye depth of back faces, so the lure plastic can tell a thin claw from a
// thick body by how much plastic the view ray crosses.
@fragment
fn fs_back_depth(in: VsOut) -> @location(0) vec4<f32> {
    let depth = dot(in.world - g.cam_pos.xyz, g.cam_forward.xyz);
    return vec4<f32>(max(depth, 1e-6), 0.0, 0.0, 1.0);
}
