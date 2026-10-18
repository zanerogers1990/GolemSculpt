// Depth of field: what sits at the focus distance stays sharp, and the rest
// blurs the farther it is from there, as through a camera lens.
//
// The Unity app used its render pipeline's "Bokeh" depth of field, which has
// no shader of ours to port. This follows that effect's steps and numbers
// (the same blur size from distance, the same disc of taps, the same
// margins), so the same Focus Distance and Aperture give the same picture:
//
//   1. half size: the scene, with how blurred each pixel should be beside it
//   2. half size: each pixel gathers a disc of its neighbours
//   3. half size: a small smoothing of the gather's dot pattern
//   4. full size: sharp and blurred mixed by how blurred each pixel should be
//
// Where it departs from that effect is at the outlines between sharp and
// blurred, which the original leaves with a dark line; each such place says
// so below.

struct DofU {
    // xy the picture's size in px, zw one of its pixels in uv
    size: vec4<f32>,
    // Distance from the eye of a depth-buffer value d: (y - d * w) / (d * z - x)
    depth: vec4<f32>,
    // x focus distance, y blur per (1 - focus / distance), z the widest blur
    // radius as a fraction of the picture's height, w how far behind its
    // perspective position the camera is parked (orthographic views)
    lens: vec4<f32>,
    // The disc: xy a tap's offset in half-size pixels, z how far out that is
    // as a fraction of the picture's height
    kernel: array<vec4<f32>, 42>,
};

@group(0) @binding(0) var<uniform> dof: DofU;
@group(0) @binding(1) var tex_a: texture_2d<f32>;
@group(0) @binding(2) var tex_b: texture_2d<f32>;
@group(0) @binding(3) var smp: sampler;
// The scene's own depth buffer, four samples a pixel.
@group(0) @binding(4) var scene_depth: texture_depth_multisampled_2d;

const TAPS: i32 = 42;
const PI: f32 = 3.14159265;

struct DofOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

// One triangle over the whole target.
@vertex
fn vs_dof(@builtin(vertex_index) i: u32) -> DofOut {
    var out: DofOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

// How blurred the surface at a depth-buffer value is: 0 in focus, down to -1
// nearer than the focus and up to 1 beyond it. Empty background counts as
// very far away.
fn blur_of(d: f32) -> f32 {
    let away = (dof.depth.y - d * dof.depth.w) / max(d * dof.depth.z - dof.depth.x, 1e-9) - dof.lens.w;
    return clamp((1.0 - dof.lens.x / max(away, 1e-6)) * dof.lens.y, -1.0, 1.0);
}

// How much of a pixel blurred by `radius` (a fraction of the picture's
// height) belongs to the blur at all: none in focus, all of it from two
// pixels of blur up.
fn blurred_part(radius: f32) -> f32 {
    return smoothstep(0.0, dof.size.w * 2.0, abs(radius));
}

// Step 1. tex_a is the scene. Out: its colour at half size, and in alpha the
// blur radius there (a fraction of the picture's height; negative nearer
// than the focus).
@fragment
fn fs_dof_prefilter(in: DofOut) -> @location(0) vec4<f32> {
    let full = vec2<i32>(textureDimensions(tex_a));
    let samples = i32(textureNumSamples(scene_depth));
    // The four full-size pixels under this one.
    let corner = vec2<i32>(floor(in.uv * vec2<f32>(full) - 0.5));
    var sum = vec3<f32>(0.0);
    var total = 0.0;
    var plain = vec3<f32>(0.0);
    var nearest = 1.0;
    var farthest = -1.0;
    for (var i = 0; i < 4; i++) {
        let at = clamp(corner + vec2<i32>(i & 1, i >> 1), vec2<i32>(0), full - 1);
        let color = textureLoad(tex_a, at, 0).rgb;
        // A pixel on an outline holds two surfaces at once. It counts as the
        // sharper of them, so the edge of a sharp object stays out of the
        // blur behind it. (The original reads one depth a pixel.)
        var blur = 1.0;
        for (var s = 0; s < samples; s++) {
            let b = blur_of(textureLoad(scene_depth, at, s));
            blur = select(blur, b, abs(b) < abs(blur) || s == 0);
        }
        // Sharp pixels stay out of the average, so they cannot leak into
        // the blur; very bright ones count for less, so they do not flicker.
        let weight = abs(blur) / (max(color.r, max(color.g, color.b)) + 1.0);
        sum += color * weight;
        total += weight;
        plain += color;
        nearest = min(nearest, blur);
        farthest = max(farthest, blur);
    }
    // The most blurred of the four speaks for all of them.
    let radius = select(farthest, nearest, -nearest > farthest) * dof.lens.z;
    // All four in focus: their plain average, which step 2 leaves out of
    // every blur but its own pixel can fall back on.
    let color = select(plain * 0.25, sum / max(total, 1e-6), total > 1e-6);
    return vec4<f32>(color, radius);
}

// Step 2. tex_a is step 1. Each pixel gathers the disc twice over: once for
// what lies beyond the focus and once for what lies nearer. Out: the blurred
// colour, and in alpha how much of it is the nearer blur (which spreads over
// whatever is behind it, sharp or not).
@fragment
fn fs_dof_blur(in: DofOut) -> @location(0) vec4<f32> {
    // One half-size pixel as a fraction of the picture's height, and the two
    // over which a tap comes in gradually as the blur grows past it.
    let pixel = dof.size.w * 2.0;
    let margin = pixel * 2.0;
    let here = vec2<i32>(in.pos.xy);
    let last = vec2<i32>(textureDimensions(tex_a)) - 1;
    let centre = textureLoad(tex_a, here, 0);
    // A pixel in focus never shows its own far blur, but the smoothing and
    // enlarging in steps 3 and 4 let its neighbours see a little of it. So
    // it stands for the background that would show if the sharp surface were
    // not there: for it, a tap counts by the tap's blur alone. (The original
    // leaves it black, which is the dark line around sharp objects there.)
    let in_focus = 1.0 - smoothstep(pixel, margin, abs(centre.a));
    let reach = max(centre.a, in_focus * dof.lens.z);
    var far = vec4<f32>(0.0);
    var near = vec4<f32>(0.0);
    // -1 is the centre itself, which is not in the list of taps.
    for (var i = -1; i < TAPS; i++) {
        var tap = vec4<f32>(0.0);
        var s = centre;
        if (i >= 0) {
            tap = dof.kernel[i];
            // Whole pixels, not blends of them: a blend of a sharp pixel and
            // a blurred one is neither, and would be weighed as if it were.
            s = textureLoad(tex_a, clamp(here + vec2<i32>(tap.xy), vec2<i32>(0), last), 0);
        }
        // Beyond the focus a tap counts only as far as BOTH it and this
        // pixel are blurred, so a sharp surface in front never smears onto
        // the blurred background around it. What is in focus stays out.
        let far_blur = max(min(reach, s.a), 0.0);
        let far_weight = saturate((far_blur - tap.z + margin) / margin) * blurred_part(s.a);
        // Nearer than the focus a tap counts as far as its own blur reaches.
        let near_weight = saturate((-s.a - tap.z + margin) / margin) * step(pixel, -s.a);
        far += vec4<f32>(s.rgb, 1.0) * far_weight;
        near += vec4<f32>(s.rgb, 1.0) * near_weight;
    }
    // A gather that found nothing to blur keeps the pixel's own colour.
    let far_color = mix(centre.rgb, far.rgb / max(far.a, 1e-5), saturate(far.a));
    let near_color = near.rgb / max(near.a, 1e-5);
    let cover = saturate(near.a * PI / f32(TAPS + 1));
    return vec4<f32>(mix(far_color, near_color, cover), cover);
}

// Step 3. tex_a is step 2: four taps, each between four pixels.
@fragment
fn fs_dof_smooth(in: DofOut) -> @location(0) vec4<f32> {
    let t = 0.5 / vec2<f32>(textureDimensions(tex_a));
    return (textureSampleLevel(tex_a, smp, in.uv - t, 0.0)
        + textureSampleLevel(tex_a, smp, in.uv + vec2<f32>(t.x, -t.y), 0.0)
        + textureSampleLevel(tex_a, smp, in.uv + vec2<f32>(-t.x, t.y), 0.0)
        + textureSampleLevel(tex_a, smp, in.uv + t, 0.0)) * 0.25;
}

// Step 4. tex_a is the sharp scene and tex_b step 3.
@fragment
fn fs_dof_composite(in: DofOut) -> @location(0) vec4<f32> {
    let at = vec2<i32>(in.pos.xy);
    let color = textureLoad(tex_a, at, 0).rgb;
    let blurred = textureSampleLevel(tex_b, smp, in.uv, 0.0);
    // How much of this pixel lies far enough beyond the focus to take the
    // blur: none under two pixels of it, all of it over four. Each of the
    // pixel's depth samples has its say, so the smoothed outline of a sharp
    // object against a blurred background stays smooth.
    let samples = i32(textureNumSamples(scene_depth));
    var far = 0.0;
    for (var s = 0; s < samples; s++) {
        let radius = blur_of(textureLoad(scene_depth, at, s)) * dof.lens.z;
        far += smoothstep(dof.size.w * 2.0, dof.size.w * 4.0, radius);
    }
    far /= f32(samples);
    // Alpha is the sharp scene's: how much of the pixel is model, not backdrop.
    let model = textureLoad(tex_a, at, 0).a;
    return vec4<f32>(mix(color, blurred.rgb, far + blurred.a - far * blurred.a), model);
}
