// What happens to the picture around and after the surface shading: ambient
// occlusion before it; bloom, vignette, the tone curve and colour grading
// after. Ported from the Unity app's render pipeline so the same settings
// give the same picture.

struct PostU {
    // xy viewport px, z tan(half fov) (0 = orthographic), w aspect
    viewport: vec4<f32>,
    // x orthographic half height, y occlusion radius, z occlusion intensity,
    // w which sample of a refined picture (the noise moves on with it)
    ao: vec4<f32>,
    // x intensity (0 = off), y threshold, z scatter
    bloom: vec4<f32>,
    // x intensity * 3 (0 = off), y smoothness * 5
    vignette: vec4<f32>,
    // x tone curve (0 none, 1 neutral, 2 ACES, 3 PBR Neutral, 4 AgX),
    // y grading on, z saturation, w contrast (both 1 = unchanged)
    grade: vec4<f32>,
    // x what the light is multiplied by before the tone curve, y 1 when the
    // backdrop is a picked colour, which is shown as picked, z 1 for a
    // picture with no backdrop: alpha out is how much of a pixel is model
    exposure: vec4<f32>,
    // x 1 while the picture changes every frame (the occlusion takes fewer
    // samples then)
    occlusion: vec4<f32>,
    // x film grain, y sharpening, z the colour table's strength (0 none),
    // w its points a side
    extras: vec4<f32>,
};

@group(0) @binding(0) var<uniform> post: PostU;
@group(0) @binding(1) var tex_a: texture_2d<f32>;
@group(0) @binding(2) var tex_b: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
// The colour table (render/lut.rs), for the composite pass only.
@group(1) @binding(0) var lut_tex: texture_3d<f32>;
@group(1) @binding(1) var lut_smp: sampler;

const TAU: f32 = 6.2831853;

struct PostOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) variant: u32,
};

// One triangle over the whole target. The instance number picks a variant
// of the pass (the blur's direction).
@vertex
fn vs_post(@builtin(vertex_index) i: u32, @builtin(instance_index) instance: u32) -> PostOut {
    var out: PostOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    out.variant = instance;
    return out;
}

// ---- ambient occlusion ----------------------------------------------------
//
// tex_a here is the normals prepass: view-space normal and eye depth.

// A pixel's position relative to the camera, along its right / up / forward.
fn view_pos(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x / post.viewport.x * 2.0 - 1.0, 1.0 - pixel.y / post.viewport.y * 2.0);
    if (post.viewport.z > 0.0) {
        return vec3<f32>(ndc.x * post.viewport.z * post.viewport.w * depth, ndc.y * post.viewport.z * depth, depth);
    }
    return vec3<f32>(ndc.x * post.ao.x * post.viewport.w, ndc.y * post.ao.x, depth);
}

fn to_pixel(p: vec3<f32>) -> vec2<f32> {
    var ndc: vec2<f32>;
    if (post.viewport.z > 0.0) {
        ndc = vec2<f32>(p.x / (p.z * post.viewport.z * post.viewport.w), p.y / (p.z * post.viewport.z));
    } else {
        ndc = vec2<f32>(p.x / (post.ao.x * post.viewport.w), p.y / post.ao.x);
    }
    return vec2<f32>((ndc.x * 0.5 + 0.5) * post.viewport.x, (0.5 - ndc.y * 0.5) * post.viewport.y);
}

fn gradient_noise(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

fn hash1(x: f32) -> f32 {
    return fract(sin(x * 12.9898) * 43758.5453);
}

// Ground-truth ambient occlusion (Jimenez, Wu, Pesce and Jarabo 2016, as
// Intel's XeGTAO lays it out). In a few slices through the view at this
// pixel, the highest the depth buffer rises on either side within the
// radius bounds the sky the surface can see, and the cosine-weighted arc of
// sky between those two horizons has a closed form: that is how open the
// pixel is, as a matte surface feels it. The same arcs give the bent normal,
// the way the open sky lies, which the surface shader takes its ambient
// light from and measures its reflections' occlusion against.
//
// Out: the bent normal (view space, z forward) in rgb as 0.5 + 0.5 n, and
// how open the pixel is in alpha (1 = all).

const PI_HALF: f32 = 1.5707963;

// XeGTAO's fast arc cosine: close enough for the horizons, and cheap.
fn fast_acos(x: f32) -> f32 {
    let ax = abs(x);
    let r = (-0.156583 * ax + PI_HALF) * sqrt(1.0 - ax);
    return select(3.1415927 - r, r, x >= 0.0);
}

// What the occlusion writes where there is no surface, or none it can
// measure: open, with no bent normal (0.5 is zero), so the surface shader
// keeps the surface's own.
const OPEN: vec4<f32> = vec4<f32>(0.5, 0.5, 0.5, 1.0);

@fragment
fn fs_ssao(in: PostOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(tex_a));
    let pixel = vec2<i32>(in.pos.xy);
    let centre = textureLoad(tex_a, pixel, 0);
    if (centre.w <= 0.0) {
        return OPEN;
    }
    let depth = centre.w;
    // The prepass normal looks back along the view's z; here z runs forward.
    let n = normalize(vec3<f32>(centre.x, centre.y, -centre.z));
    let p = view_pos(in.pos.xy, depth);
    let v = select(normalize(-p), vec3<f32>(0.0, 0.0, -1.0), post.viewport.z <= 0.0);
    let radius = post.ao.y;
    // The radius in pixels at this depth.
    let per_unit = select(post.viewport.y / (2.0 * post.ao.x), post.viewport.y / (2.0 * post.viewport.z * depth), post.viewport.z > 0.0);
    let reach = min(radius * per_unit, post.viewport.y * 0.25);
    if (reach < 1.5) {
        return OPEN;
    }
    // Every slice and step turned and shifted per pixel and per sample of
    // a refined picture; fewer of them while the picture changes every frame.
    let moving = post.occlusion.x > 0.5;
    let slices = select(2, 1, moving);
    let steps = select(8, 4, moving);
    let turn = gradient_noise(in.pos.xy + 5.588238 * post.ao.w);
    let shift = gradient_noise(in.pos.yx * 1.31 + 17.17 + 3.7 * post.ao.w);
    var visibility = 0.0;
    var bent = vec3<f32>(0.0);
    for (var slice = 0; slice < slices; slice++) {
        let phi = (f32(slice) + turn) * 3.1415927 / f32(slices);
        // On screen y runs down; in view space up.
        let omega = vec2<f32>(cos(phi), sin(phi));
        let dir = vec3<f32>(omega.x, -omega.y, 0.0);
        let ortho = dir - v * dot(dir, v);
        let axis = normalize(cross(ortho, v));
        let projected = n - axis * dot(n, axis);
        let projected_len = length(projected);
        let cos_n = saturate(dot(projected, v) / max(projected_len, 1e-6));
        let angle_n = sign(dot(ortho, projected)) * fast_acos(cos_n);
        // Horizons start at the surface's own tangent plane.
        let low0 = cos(angle_n + PI_HALF);
        let low1 = cos(angle_n - PI_HALF);
        var horizon0 = low0;
        var horizon1 = low1;
        for (var step = 0; step < steps; step++) {
            let s = (f32(step) + shift) / f32(steps);
            let along = omega * (s * s * reach + 1.0);
            let at0 = clamp(in.pos.xy + along, vec2<f32>(0.5), size - 0.5);
            let at1 = clamp(in.pos.xy - along, vec2<f32>(0.5), size - 0.5);
            let depth0 = textureLoad(tex_a, vec2<i32>(at0), 0).w;
            let depth1 = textureLoad(tex_a, vec2<i32>(at1), 0).w;
            if (depth0 > 0.0) {
                let d = view_pos(at0, depth0) - p;
                let len = length(d);
                // Farther than the radius counts for less, then nothing.
                let near = saturate((radius - len) / (radius * 0.4));
                horizon0 = max(horizon0, mix(low0, dot(d / len, v), near));
            }
            if (depth1 > 0.0) {
                let d = view_pos(at1, depth1) - p;
                let len = length(d);
                let near = saturate((radius - len) / (radius * 0.4));
                horizon1 = max(horizon1, mix(low1, dot(d / len, v), near));
            }
        }
        // The two horizons as angles from the view, within the hemisphere.
        let h0 = angle_n + max(-fast_acos(horizon1) - angle_n, -PI_HALF);
        let h1 = angle_n + min(fast_acos(horizon0) - angle_n, PI_HALF);
        let sin_n = sin(angle_n);
        let arc0 = (cos_n + 2.0 * h0 * sin_n - cos(2.0 * h0 - angle_n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin_n - cos(2.0 * h1 - angle_n)) * 0.25;
        visibility += projected_len * (arc0 + arc1);
        // The middle of the open arc, weighted as the visibility is.
        let t0 = (6.0 * sin(h0 - angle_n) - sin(3.0 * h0 - angle_n) + 6.0 * sin(h1 - angle_n) - sin(3.0 * h1 - angle_n)
            + 16.0 * sin_n - 3.0 * (sin(h0 + angle_n) + sin(h1 + angle_n))) / 12.0;
        let t1 = (-cos(3.0 * h0 - angle_n) - cos(3.0 * h1 - angle_n) + 8.0 * cos(angle_n)
            - 3.0 * (cos(h0 + angle_n) + cos(h1 + angle_n))) / 12.0;
        bent += projected_len * (normalize(ortho) * t0 + v * t1);
    }
    visibility /= f32(slices);
    // As strong as the occlusion it replaces looked.
    let open = pow(saturate(visibility), post.ao.z);
    let b = select(n, normalize(bent), dot(bent, bent) > 1e-10);
    return vec4<f32>(b * 0.5 + 0.5, open);
}

fn prepass_normal(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(tex_a));
    let at = clamp(vec2<i32>(uv * size), vec2<i32>(0), vec2<i32>(size) - 1);
    return textureLoad(tex_a, at, 0).xyz;
}

// Smooths the occlusion and its bent normal along one axis (variant 0
// across, 1 down) without bleeding across edges: taps whose surface faces
// another way count for less. tex_b is the occlusion.
@fragment
fn fs_ssao_blur(in: PostOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(tex_b));
    let delta = select(vec2<f32>(texel.x, 0.0), vec2<f32>(0.0, texel.y), in.variant == 1u);
    let n0 = prepass_normal(in.uv);
    var offsets = array<f32, 5>(0.0, -1.3846153846, 1.3846153846, -3.2307692308, 3.2307692308);
    var weights = array<f32, 5>(0.2270270270, 0.3162162162, 0.3162162162, 0.0702702703, 0.0702702703);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = 0; i < 5; i++) {
        let uv = in.uv + delta * offsets[i];
        var w = weights[i];
        if (i > 0) {
            w *= smoothstep(0.8, 1.0, dot(n0, prepass_normal(uv)));
        }
        sum += textureSampleLevel(tex_b, smp, uv, 0.0) * w;
        total += w;
    }
    return sum / total;
}

// ---- bloom ----------------------------------------------------------------

// Keeps only what is brighter than the threshold (with a soft knee), at half
// size, through a wide filter so a bright pixel does not flicker.
@fragment
fn fs_bloom_prefilter(in: PostOut) -> @location(0) vec4<f32> {
    let t = 1.0 / vec2<f32>(textureDimensions(tex_a));
    let uv = in.uv;
    let a = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(-1.0, -1.0), 0.0).rgb;
    let b = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(0.0, -1.0), 0.0).rgb;
    let c = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(1.0, -1.0), 0.0).rgb;
    let d = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(-0.5, -0.5), 0.0).rgb;
    let e = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(0.5, -0.5), 0.0).rgb;
    let f = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(-1.0, 0.0), 0.0).rgb;
    let gg = textureSampleLevel(tex_a, smp, uv, 0.0).rgb;
    let h = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(1.0, 0.0), 0.0).rgb;
    let i = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(-0.5, 0.5), 0.0).rgb;
    let j = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(0.5, 0.5), 0.0).rgb;
    let k = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(-1.0, 1.0), 0.0).rgb;
    let l = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(0.0, 1.0), 0.0).rgb;
    let m = textureSampleLevel(tex_a, smp, uv + t * vec2<f32>(1.0, 1.0), 0.0).rgb;
    var color = (d + e + i + j) * 0.125
        + ((a + b + gg + f) + (b + c + h + gg) + (f + gg + l + k) + (gg + h + m + l)) * 0.03125;
    color = min(color, vec3<f32>(65472.0));

    let threshold = post.bloom.y;
    let knee = threshold * 0.5;
    let brightness = max(color.r, max(color.g, color.b));
    var softness = clamp(brightness - threshold + knee, 0.0, 2.0 * knee);
    softness = softness * softness / (4.0 * knee + 1e-4);
    let multiplier = max(brightness - threshold, softness) / max(brightness, 1e-4);
    return vec4<f32>(max(color * multiplier, vec3<f32>(0.0)), 1.0);
}

// Shrinks to half size while blurring across.
@fragment
fn fs_bloom_blur_h(in: PostOut) -> @location(0) vec4<f32> {
    let t = 2.0 / f32(textureDimensions(tex_a).x);
    var weights = array<f32, 5>(0.22702703, 0.19459459, 0.12162162, 0.05405405, 0.01621622);
    var color = textureSampleLevel(tex_a, smp, in.uv, 0.0).rgb * weights[0];
    for (var i = 1; i < 5; i++) {
        let o = vec2<f32>(t * f32(i), 0.0);
        color += (textureSampleLevel(tex_a, smp, in.uv + o, 0.0).rgb
            + textureSampleLevel(tex_a, smp, in.uv - o, 0.0).rgb) * weights[i];
    }
    return vec4<f32>(color, 1.0);
}

@fragment
fn fs_bloom_blur_v(in: PostOut) -> @location(0) vec4<f32> {
    let t = 1.0 / f32(textureDimensions(tex_a).y);
    let o1 = vec2<f32>(0.0, t * 1.38461538);
    let o2 = vec2<f32>(0.0, t * 3.23076923);
    let color = textureSampleLevel(tex_a, smp, in.uv, 0.0).rgb * 0.22702703
        + (textureSampleLevel(tex_a, smp, in.uv + o1, 0.0).rgb + textureSampleLevel(tex_a, smp, in.uv - o1, 0.0).rgb) * 0.31621622
        + (textureSampleLevel(tex_a, smp, in.uv + o2, 0.0).rgb + textureSampleLevel(tex_a, smp, in.uv - o2, 0.0).rgb) * 0.07027027;
    return vec4<f32>(color, 1.0);
}

// A smooth (cubic) enlargement of tex_b in four taps.
fn bicubic_b(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(tex_b));
    let xy = uv * size + 0.5;
    let ic = floor(xy);
    let f = fract(xy);
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 / 6.0) * (-f3 + 3.0 * f2 - 3.0 * f + 1.0);
    let w1 = (1.0 / 6.0) * (3.0 * f3 - 6.0 * f2 + 4.0);
    let w2 = (1.0 / 6.0) * (-3.0 * f3 + 3.0 * f2 + 3.0 * f + 1.0);
    let w3 = (1.0 / 6.0) * f3;
    let s0 = w0 + w1;
    let s1 = w2 + w3;
    let p0 = (ic - 1.0 + w1 / s0 - 0.5) / size;
    let p1 = (ic + 1.0 + w3 / s1 - 0.5) / size;
    return s0.y * (s0.x * textureSampleLevel(tex_b, smp, vec2<f32>(p0.x, p0.y), 0.0).rgb
            + s1.x * textureSampleLevel(tex_b, smp, vec2<f32>(p1.x, p0.y), 0.0).rgb)
        + s1.y * (s0.x * textureSampleLevel(tex_b, smp, vec2<f32>(p0.x, p1.y), 0.0).rgb
            + s1.x * textureSampleLevel(tex_b, smp, vec2<f32>(p1.x, p1.y), 0.0).rgb);
}

// On the way back up: this level (tex_a) mixed with the blurrier one below
// it (tex_b). Scatter sets how far the glow spreads.
@fragment
fn fs_bloom_up(in: PostOut) -> @location(0) vec4<f32> {
    let high = textureSampleLevel(tex_a, smp, in.uv, 0.0).rgb;
    return vec4<f32>(mix(high, bicubic_b(in.uv), post.bloom.z), 1.0);
}

// ---- the finished picture -------------------------------------------------

fn neutral_curve(x: vec3<f32>) -> vec3<f32> {
    let a = 0.2;
    let b = 0.29;
    let c = 0.24;
    let d = 0.272;
    let e = 0.02;
    let f = 0.3;
    return (x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f) - e / f;
}

// The pipeline's "neutral" tone curve: a gentle shoulder that keeps hue.
fn neutral_tonemap(x: vec3<f32>) -> vec3<f32> {
    let white_scale = vec3<f32>(1.0) / neutral_curve(vec3<f32>(5.3));
    return neutral_curve(x * white_scale) * white_scale;
}

// A close fit of the ACES film curve.
fn aces_tonemap(x: vec3<f32>) -> vec3<f32> {
    let m1 = mat3x3<f32>(
        vec3<f32>(0.59719, 0.07600, 0.02840),
        vec3<f32>(0.35458, 0.90834, 0.13383),
        vec3<f32>(0.04823, 0.01566, 0.83777));
    let m2 = mat3x3<f32>(
        vec3<f32>(1.60475, -0.10208, -0.00327),
        vec3<f32>(-0.53108, 1.10813, -0.07276),
        vec3<f32>(-0.07367, -0.00605, 1.07602));
    let v = m1 * x;
    let a = v * (v + 0.0245786) - 0.000090537;
    let b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return clamp(m2 * (a / b), vec3<f32>(0.0), vec3<f32>(1.0));
}

// Khronos PBR Neutral: the colour as it is up to near white (less a small
// offset that keeps the darkest tones from going muddy), then the brightest
// part rolled off toward white with as little shift of hue as it can.
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

// AgX (Troy Sobotka's, in Benjamin Wrensch's fit): into a log space with
// the primaries pulled in a little, an S curve there, and back out. Its
// result is display-encoded; it is returned linear, for the encoding below.
fn agx_tonemap(c: vec3<f32>) -> vec3<f32> {
    let inset = mat3x3<f32>(
        vec3<f32>(0.842479062253094, 0.0423282422610123, 0.0423756549057051),
        vec3<f32>(0.0784335999999992, 0.878468636469772, 0.0784336),
        vec3<f32>(0.0792237451477643, 0.0791661274605434, 0.879142973793104));
    let outset = mat3x3<f32>(
        vec3<f32>(1.19687900512017, -0.0528968517574562, -0.0529716355144438),
        vec3<f32>(-0.0980208811401368, 1.15190312990417, -0.0980434501171241),
        vec3<f32>(-0.0990297440797205, -0.0989611768448433, 1.15107367264116));
    let min_ev = -12.47393;
    let max_ev = 4.026069;
    var v = inset * max(c, vec3<f32>(1e-10));
    v = clamp(log2(v), vec3<f32>(min_ev), vec3<f32>(max_ev));
    v = (v - min_ev) / (max_ev - min_ev);
    let v2 = v * v;
    let v4 = v2 * v2;
    v = 15.5 * v4 * v2 - 40.14 * v4 * v + 31.96 * v4 - 6.868 * v2 * v + 0.4298 * v2 + 0.1191 * v - 0.00232;
    v = outset * v;
    return pow(max(v, vec3<f32>(0.0)), vec3<f32>(2.2));
}

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

// A number in -0.5..0.5 with no pattern in it, for film grain.
fn grain_noise(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(0.1031, 0.1030));
    let r = q + dot(q, q.yx + 33.33);
    return fract((r.x + r.y) * r.x) - 0.5;
}

// tex_a is the shaded scene, tex_b the bloom. The scene's alpha is how much
// of the pixel is model (1) rather than backdrop (0).
@fragment
fn fs_composite(in: PostOut) -> @location(0) vec4<f32> {
    var scene = textureSampleLevel(tex_a, smp, in.uv, 0.0);
    if (post.extras.y > 0.0) {
        // Sharpening: the pixel pushed away from its neighbours' average,
        // never past the brightest or darkest of them, so edges get crisper
        // without a halo (after AMD's contrast-adaptive sharpening).
        let t = 1.0 / vec2<f32>(textureDimensions(tex_a));
        let n = textureSampleLevel(tex_a, smp, in.uv + vec2<f32>(0.0, -t.y), 0.0);
        let s = textureSampleLevel(tex_a, smp, in.uv + vec2<f32>(0.0, t.y), 0.0);
        let e = textureSampleLevel(tex_a, smp, in.uv + vec2<f32>(t.x, 0.0), 0.0);
        let w = textureSampleLevel(tex_a, smp, in.uv + vec2<f32>(-t.x, 0.0), 0.0);
        let lo = min(min(min(n, s), min(e, w)), scene);
        let hi = max(max(max(n, s), max(e, w)), scene);
        scene = clamp(scene + (scene - (n + s + e + w) * 0.25) * post.extras.y * 2.0, lo, hi);
    }
    let transparent = post.exposure.z > 0.5;
    let coverage = saturate(scene.a);
    // With no backdrop (black there), an outline pixel is the model's colour
    // times its share of the pixel: undone, so the alpha alone fades it.
    var color = select(scene.rgb, scene.rgb / max(coverage, 1e-4), transparent);
    if (post.bloom.x > 0.0) {
        color += bicubic_b(in.uv) * post.bloom.x;
    }
    if (post.vignette.x > 0.0) {
        let dist = abs(in.uv - 0.5) * post.vignette.x;
        color *= pow(saturate(1.0 - dot(dist, dist)), post.vignette.y);
    }
    // A backdrop colour the user picked is shown as picked: a white
    // background for a product shot stays white whatever the curve does.
    let as_picked = color;
    let backdrop = select(0.0, 1.0 - coverage, post.exposure.y > 0.5 && !transparent);
    color *= post.exposure.x;
    if (post.grade.x > 3.5) {
        color = agx_tonemap(color);
    } else if (post.grade.x > 2.5) {
        color = pbr_neutral_tonemap(color);
    } else if (post.grade.x > 1.5) {
        color = aces_tonemap(color);
    } else if (post.grade.x > 0.5) {
        color = neutral_tonemap(color);
    }
    if (post.grade.y > 0.5) {
        // Contrast pivots on mid grey in a log encoding, as film does.
        let lg = 0.244161 * log(5.555556 * max(color, vec3<f32>(0.0)) + 0.047996) / log(10.0) + 0.386036;
        let contrasted = (lg - 0.4135884) * post.grade.w + 0.4135884;
        color = (pow(vec3<f32>(10.0), (contrasted - 0.386036) / 0.244161) - 0.047996) / 5.555556;
        let luma = dot(color, vec3<f32>(0.2126729, 0.7151522, 0.0721750));
        color = max(vec3<f32>(luma) + post.grade.z * (color - luma), vec3<f32>(0.0));
    }
    color = mix(color, as_picked, backdrop);
    if (post.extras.x > 0.0) {
        // Film grain: strongest in the mid tones, as on film.
        let luma = dot(color, vec3<f32>(0.2126729, 0.7151522, 0.0721750));
        let amount = post.extras.x * 0.12 * (1.0 - 0.7 * abs(saturate(luma) * 2.0 - 1.0));
        color = max(color * (1.0 + grain_noise(in.pos.xy) * amount * 4.0) + grain_noise(in.pos.yx + 7.7) * amount * 0.25, vec3<f32>(0.0));
    }
    // The target is a plain 8-bit format: encode here, with a little noise
    // to break up banding in smooth gradients.
    var shown = to_srgb(color);
    if (post.extras.z > 0.0) {
        // The colour table works on the colour as shown: last.
        let size = max(post.extras.w, 2.0);
        let at = saturate(shown) * ((size - 1.0) / size) + 0.5 / size;
        shown = mix(shown, textureSampleLevel(lut_tex, lut_smp, at, 0.0).rgb, post.extras.z);
    }
    let dither = fract(52.9829189 * fract(dot(in.pos.xy, vec2<f32>(0.06711056, 0.00583715)))) - 0.5;
    return vec4<f32>(shown + dither / 255.0, select(1.0, coverage, transparent));
}
