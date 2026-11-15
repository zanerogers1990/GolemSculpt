
// ==== material finishes ===========================================================
//
// Clay, Metal and Wood: the desktop's own finish functions (sculpt_pbr.wgsl),
// copied as they are below, lit by a lighter rig than the desktop's so they
// run on WebGL2 too: three lights turned with the camera, the sky, equator
// and ground light all round, and a reflected sky worked out here rather
// than read from an HDRI. No shadow map and no ambient occlusion: the mesh's
// own curvature puts the wash in the recesses and the wear on the ridges.

struct FinishU {
    // x: the finish (0 matcap, 1 clay, 2 metal, 3 wood), y: how strongly the
    // sky is reflected.
    kind: vec4<f32>,
    light_dir: array<vec4<f32>, 3>,
    // Linear colour times intensity (pi is in it, as on the desktop).
    light_color: array<vec4<f32>, 3>,
    sky: vec4<f32>,
    equator: vec4<f32>,
    ground: vec4<f32>,
    // The finish's own settings, laid out as its function reads them.
    v: array<vec4<f32>, 12>,
};
@group(0) @binding(4) var<uniform> fin: FinishU;

const TAU: f32 = 6.2831853;

fn luminance(c: vec3<f32>) -> f32 {
    return dot(c, vec3<f32>(0.2126, 0.7152, 0.0722));
}

// ---- noise (the desktop's) ---------------------------------------------------------

fn hash33(p_in: vec3<f32>) -> vec3<f32> {
    var p = fract(p_in * vec3<f32>(0.1031, 0.1030, 0.0973));
    p += dot(p, p.yxz + 33.33);
    return fract((p.xxy + p.yxx) * p.zyx);
}

// A lattice corner's value, 0..1, in whole-number arithmetic, so the cells
// on either side of a grid plane get the same bits for the corners they share.
fn hash_corner(c: vec3<i32>) -> f32 {
    let u = bitcast<vec3<u32>>(c);
    var x = (u.x * 0x8da6b343u) ^ (u.y * 0xd8163841u) ^ (u.z * 0xcb1ab31fu);
    x ^= x >> 16u;
    x *= 0x7feb352du;
    x ^= x >> 15u;
    x *= 0x846ca68bu;
    x ^= x >> 16u;
    return f32(x >> 8u) * (1.0 / 16777216.0);
}

fn value_noise(p: vec3<f32>) -> f32 {
    let cell = floor(p);
    let i = vec3<i32>(cell);
    var f = p - cell;
    f = f * f * (3.0 - 2.0 * f);
    let n000 = hash_corner(i + vec3<i32>(0, 0, 0));
    let n100 = hash_corner(i + vec3<i32>(1, 0, 0));
    let n010 = hash_corner(i + vec3<i32>(0, 1, 0));
    let n110 = hash_corner(i + vec3<i32>(1, 1, 0));
    let n001 = hash_corner(i + vec3<i32>(0, 0, 1));
    let n101 = hash_corner(i + vec3<i32>(1, 0, 1));
    let n011 = hash_corner(i + vec3<i32>(0, 1, 1));
    let n111 = hash_corner(i + vec3<i32>(1, 1, 1));
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

// ---- what a finish reads and makes (the desktop's, cut down) ------------------

struct Surface {
    world: vec3<f32>,
    // Toward the eye.
    v: vec3<f32>,
    // Position in the model's own space, how much of it one pixel covers,
    // and how much it changes to the next pixel right and down.
    p: vec3<f32>,
    footprint: f32,
    p_dx: vec3<f32>,
    p_dy: vec3<f32>,
    // The model is never stretched or turned here: these are 1 and its axes.
    scale: vec3<f32>,
    mx: vec3<f32>,
    my: vec3<f32>,
    mz: vec3<f32>,
    smooth_n: vec3<f32>,
    n: vec3<f32>,
    // Recess (above 0.5) or ridge (below), from the mesh's own curvature.
    curv: f32,
};

struct MaterialSample {
    albedo: vec3<f32>,
    metallic: f32,
    smoothness: f32,
    occlusion: f32,
    normal: vec3<f32>,
    emission: vec3<f32>,
    sheen_amount: f32,
    sheen_power: f32,
    scatter_color: vec3<f32>,
    scatter_wrap: f32,
    scatter_shadow: f32,
    dim: f32,
};

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

fn surface_width(s: Surface, grad: vec3<f32>) -> f32 {
    return abs(dot(grad, s.p_dx)) + abs(dot(grad, s.p_dy));
}

fn to_object_normal(s: Surface, n: vec3<f32>) -> vec3<f32> {
    return vec3<f32>(dot(s.mx, n), dot(s.my, n), dot(s.mz, n));
}

// ---- the light ------------------------------------------------------------------------

// Diffuse light arriving from everywhere but the lights: the desktop's sky,
// equator and ground lobes.
fn finish_ambient(n: vec3<f32>) -> vec3<f32> {
    let up = (0.09375 + 0.5 * n.y + 0.46875 * n.y * n.y) * (16.0 / 17.0);
    let down = (0.09375 - 0.5 * n.y + 0.46875 * n.y * n.y) * (16.0 / 17.0);
    let eq = fin.equator.rgb;
    return max(eq + (fin.sky.rgb - eq) * up + (fin.ground.rgb - eq) * down, vec3<f32>(0.0));
}

// The surroundings a surface mirrors, in place of the desktop's sky cube:
// ground below, sky above, a brighter band where they meet that a shiny
// surface shows crisply and a rough one spreads into the all-round light.
fn finish_environment(r: vec3<f32>, perceptual_roughness: f32) -> vec3<f32> {
    let edge = 0.04 + 0.5 * perceptual_roughness;
    let above = smoothstep(-edge, edge, r.y);
    let sky = mix(fin.equator.rgb * 1.6, fin.sky.rgb * 1.3, smoothstep(0.0, 0.8, r.y));
    let ground = mix(fin.equator.rgb, fin.ground.rgb * 0.7, smoothstep(0.0, 0.5, -r.y));
    let sharp = mix(ground, sky, above);
    let spread = finish_ambient(r);
    return mix(sharp, spread, perceptual_roughness * perceptual_roughness) * fin.kind.y;
}

fn pow5(x: f32) -> f32 {
    let x2 = x * x;
    return x2 * x2 * x;
}

// The split-sum table the desktop reads from a texture, as Karis's fit for
// mobile: (what it reflects of a surface with f0 = 0, and with f0 = 1).
fn dfg_fit(nv: f32, perceptual_roughness: f32) -> vec2<f32> {
    let c0 = vec4<f32>(-1.0, -0.0275, -0.572, 0.022);
    let c1 = vec4<f32>(1.0, 0.0425, 1.04, -0.04);
    let r = perceptual_roughness * c0 + c1;
    let a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    let ab = vec2<f32>(-1.04, 1.04) * a004 + r.zw;
    return vec2<f32>(ab.y, ab.x + ab.y);
}

fn d_ggx(nh: f32, a: f32) -> f32 {
    let a2 = a * a;
    let d = (nh * a2 - nh) * nh + 1.0;
    return a2 / (3.14159265 * d * d);
}

fn v_smith(nv: f32, nl: f32, a: f32) -> f32 {
    let a2 = a * a;
    let gv = nl * sqrt(nv * nv * (1.0 - a2) + a2);
    let gl = nv * sqrt(nl * nl * (1.0 - a2) + a2);
    return 0.5 / max(gv + gl, 1e-6);
}

fn specular_occlusion(nv: f32, ao: f32, a: f32) -> f32 {
    return saturate(pow(nv + ao, exp2(-16.0 * a - 1.0)) - 1.0 + ao);
}

// The desktop's physically based shading, under the light rig here.
fn physically_shade(s: Surface, n: vec3<f32>, albedo: vec3<f32>, metallic: f32, smoothness: f32, occlusion: f32) -> vec3<f32> {
    let diffuse = albedo * (1.0 - metallic);
    let f0 = mix(vec3<f32>(0.04), albedo, metallic);
    let perceptual_roughness = saturate(1.0 - smoothness);
    let a = max(perceptual_roughness * perceptual_roughness, 0.0078125);
    let nv = max(dot(n, s.v), 1e-4);
    let dfg = dfg_fit(nv, perceptual_roughness);
    let reflected = mix(vec3<f32>(dfg.x), vec3<f32>(dfg.y), f0);
    let energy = 1.0 + f0 * (1.0 / max(dfg.y, 1e-3) - 1.0);
    let r = reflect(-s.v, n);
    let horizon = min(1.0 + dot(r, s.smooth_n), 1.0);
    let hidden = specular_occlusion(nv, occlusion, a);
    var color = finish_ambient(n) * diffuse * (1.0 - reflected) * occlusion
        + finish_environment(r, perceptual_roughness) * reflected * energy * (hidden * horizon * horizon);
    let direct_diffuse = diffuse * (1.0 - 0.04);
    for (var i = 0; i < 3; i++) {
        let l = fin.light_dir[i].xyz;
        let nl = saturate(dot(n, l));
        let h = normalize(l + s.v);
        let nh = saturate(dot(n, h));
        let lh = saturate(dot(l, h));
        let fresnel = f0 + (vec3<f32>(1.0) - f0) * pow5(1.0 - lh);
        let spec = fresnel * (d_ggx(nh, a) * v_smith(nv, nl, a) * 3.14159265) * energy;
        color += (direct_diffuse + spec) * fin.light_color[i].rgb * nl;
    }
    return color;
}

// Lights a material sample: the physically based model, then the sheen and
// the scattered light where the sample has them (the desktop's `light_sample`).
fn light_sample(s: Surface, m: MaterialSample) -> vec3<f32> {
    var color = physically_shade(s, m.normal, m.albedo, m.metallic, m.smoothness, m.occlusion);
    if (m.sheen_amount != 0.0 || any(m.scatter_color != vec3<f32>(0.0))) {
        var sheen = vec3<f32>(0.0);
        var scatter = vec3<f32>(0.0);
        for (var i = 0; i < 3; i++) {
            let l = fin.light_dir[i].xyz;
            let c = fin.light_color[i].rgb;
            let ndl = dot(m.normal, l);
            let h = normalize(l + s.v);
            let spec = pow(saturate(dot(m.normal, h)), m.sheen_power) * (m.sheen_power + 8.0) / 8.0;
            let fresnel = 0.04 + 0.96 * pow(1.0 - saturate(dot(s.v, h)), 5.0);
            sheen += c * (spec * fresnel * saturate(ndl));
            let wrapped = saturate((ndl + m.scatter_wrap) / (1.0 + m.scatter_wrap));
            scatter += c * max(wrapped - saturate(ndl), 0.0);
        }
        color += sheen * m.sheen_amount + scatter * m.scatter_color;
    }
    return color * m.dim + m.emission;
}

// Khronos PBR Neutral, the desktop's default tone curve.
fn pbr_neutral_tonemap(c_in: vec3<f32>) -> vec3<f32> {
    let start = 0.8 - 0.04;
    let desaturation = 0.15;
    var c = c_in;
    let x = min(c.r, min(c.g, c.b));
    let offset = select(0.04, x - 6.25 * x * x, x < 0.08);
    c -= offset;
    let peak = max(c.r, max(c.g, c.b));
    if (peak < start) {
        return c;
    }
    let d = 1.0 - start;
    let new_peak = 1.0 - d * d / (peak + d - start);
    c *= new_peak / peak;
    let g = 1.0 - 1.0 / (desaturation * (peak - new_peak) + 1.0);
    return mix(c, vec3<f32>(new_peak), g);
}

fn to_display(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

// ---- the finishes (the desktop's, as they are) ---------------------------------------

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
fn finish_metal(s: Surface) -> MaterialSample {
    let metal_color_in = fin.v[0];
    let coat_a = fin.v[1];
    let coat_b = fin.v[2];
    let wash_color = fin.v[3];
    let metal_offset = fin.v[4];
    let metal_p0 = fin.v[5];
    let metal_p1 = fin.v[6];
    let metal_p2 = fin.v[7];
    let metal_p3 = fin.v[8];
    let metal_p4 = fin.v[9];
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
fn finish_clay(s: Surface) -> MaterialSample {
    let clay_color = fin.v[0];
    let clay_recess = fin.v[1];
    let clay_scatter = fin.v[2];
    let clay_p0 = fin.v[3];
    let clay_p1 = fin.v[4];
    let clay_p2 = fin.v[5];
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
fn finish_wood(s: Surface) -> MaterialSample {
    let wood_early = fin.v[0];
    let wood_late = fin.v[1];
    let wood_worn = fin.v[2];
    let wood_recess = fin.v[3];
    let wood_scatter = fin.v[4];
    let wood_axis = fin.v[5];
    let wood_pith = fin.v[6];
    let wood_p0 = fin.v[7];
    let wood_p1 = fin.v[8];
    let wood_p2 = fin.v[9];
    let wood_p3 = fin.v[10];
    let wood_p4 = fin.v[11];
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

// The finish in use, lit, tone mapped and encoded for the display.
fn finish_shade(s: Surface) -> vec3<f32> {
    var m: MaterialSample;
    let kind = fin.kind.x;
    if (kind < 1.5) {
        m = finish_clay(s);
    } else if (kind < 2.5) {
        m = finish_metal(s);
    } else {
        m = finish_wood(s);
    }
    return to_display(pbr_neutral_tonemap(light_sample(s, m)));
}
