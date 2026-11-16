// GolemSculpt web: the passes ahead of the viewport that the finishes'
// shadows and ambient occlusion read (WebGPU only; shadows.rs). The key
// light's shadow map, a prepass of view normal and eye depth, and the
// desktop's ground-truth ambient occlusion over it (src/render/post.wgsl,
// `fs_ssao` and `fs_ssao_blur`), with its two blur passes.

// The viewport's globals: the same layout as `Globals` in shader.wgsl (and
// `render::Globals`); keep the three alike.
struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    mask_tint: vec4<f32>,
    // x: flat shading, y: the page wants linear colour, z: show backfaces,
    // w: cavity on.
    params: vec4<f32>,
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    eye: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    look: vec4<f32>,
    guides: vec4<f32>,
    grade: vec4<f32>,
    extras: vec4<f32>,
};

// `shadows::Uniform`, as shade.wgsl reads it too.
struct ShadeU {
    shadow_view_proj: mat4x4<f32>,
    // x one shadow texel in uv, y depth bias, z normal bias (world units),
    // w 1 while the picture changes frame after frame (half the taps then)
    shadow_params: vec4<f32>,
    // x the map's width in world units, y world units per unit of map depth,
    // z tangent of the key light's angular radius, w the part of the texture
    // the map fills (less than 1 while it is drawn small)
    shadow_soft: vec4<f32>,
    // x shadows on, y how dark they get, z ambient occlusion on, w its
    // strength on direct light
    flags: vec4<f32>,
    // The key light's direction (toward the light).
    key: vec4<f32>,
    // xy viewport px, z tan(half fov) (0 = orthographic), w aspect
    viewport: vec4<f32>,
    // x orthographic half height, y occlusion radius, z its power
    ao: vec4<f32>,
    // x 1 while the picture changes every frame (fewer samples then)
    occlusion: vec4<f32>,
    // xy the size of the picture being drawn, to find the occlusion's pixel
    picture: vec4<f32>,
};

struct Object {
    model: mat4x4<f32>,
    normal: mat4x4<f32>,
    tint: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var<uniform> shade: ShadeU;
// The prepass (view normal, eye depth) and the occlusion being blurred.
@group(0) @binding(2) var tex_a: texture_2d<f32>;
@group(0) @binding(3) var tex_b: texture_2d<f32>;
@group(0) @binding(4) var smp: sampler;
@group(1) @binding(0) var<uniform> obj: Object;

struct GeoIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

// ---- the key light's shadow map -------------------------------------------------

// The scene from the key light. Casters are pushed a little away from the
// light and shrunk along their normals where they turn from it, which keeps
// a surface from shadowing itself (the desktop's `vs_shadow`).
@vertex
fn vs_shadow(in: GeoIn) -> @builtin(position) vec4<f32> {
    var world = (obj.model * vec4<f32>(in.position, 1.0)).xyz;
    let n = normalize((obj.normal * vec4<f32>(in.normal, 0.0)).xyz);
    let l = shade.key.xyz;
    world -= l * shade.shadow_params.y;
    world -= n * ((1.0 - saturate(dot(l, n))) * shade.shadow_params.z);
    return shade.shadow_view_proj * vec4<f32>(world, 1.0);
}

// ---- the prepass ----------------------------------------------------------------

struct PreOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

@vertex
fn vs_prepass(in: GeoIn) -> PreOut {
    var out: PreOut;
    let world = (obj.model * vec4<f32>(in.position, 1.0)).xyz;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = (obj.normal * vec4<f32>(in.normal, 0.0)).xyz;
    return out;
}

// View-space normal and eye depth (the desktop's `fs_prepass`).
@fragment
fn fs_prepass(in: PreOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    let front = facing != (obj.normal[3][3] < 0.0);
    if (!front && g.params.z < 0.5) {
        discard;
    }
    var n = normalize(in.normal);
    n = select(-n, n, front);
    let vn = (g.view * vec4<f32>(n, 0.0)).xyz;
    let depth = dot(in.world - g.eye.xyz, g.forward.xyz);
    return vec4<f32>(vn, max(depth, 1e-6));
}

// ---- ambient occlusion ----------------------------------------------------------

struct PostOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_post(@builtin(vertex_index) i: u32) -> PostOut {
    var out: PostOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    out.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn view_pos(pixel: vec2<f32>, depth: f32) -> vec3<f32> {
    let ndc = vec2<f32>(pixel.x / shade.viewport.x * 2.0 - 1.0, 1.0 - pixel.y / shade.viewport.y * 2.0);
    if (shade.viewport.z > 0.0) {
        return vec3<f32>(ndc.x * shade.viewport.z * shade.viewport.w * depth, ndc.y * shade.viewport.z * depth, depth);
    }
    return vec3<f32>(ndc.x * shade.ao.x * shade.viewport.w, ndc.y * shade.ao.x, depth);
}

fn gradient_noise(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715))));
}

const PI_HALF: f32 = 1.5707963;

fn fast_acos(x: f32) -> f32 {
    let ax = abs(x);
    let r = (-0.156583 * ax + PI_HALF) * sqrt(1.0 - ax);
    return select(3.1415927 - r, r, x >= 0.0);
}

const OPEN: vec4<f32> = vec4<f32>(0.5, 0.5, 0.5, 1.0);

// Ground-truth ambient occlusion, as the desktop's `fs_ssao`: in a slice or
// two through the view at this pixel, the highest the depth rises on either
// side within the radius bounds the sky the surface sees. Out: the bent
// normal (view space, z forward) as 0.5 + 0.5 n, and how open in alpha.
@fragment
fn fs_ssao(in: PostOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(tex_a));
    let pixel = vec2<i32>(in.pos.xy);
    let centre = textureLoad(tex_a, pixel, 0);
    if (centre.w <= 0.0) {
        return OPEN;
    }
    let depth = centre.w;
    let n = normalize(vec3<f32>(centre.x, centre.y, -centre.z));
    let p = view_pos(in.pos.xy, depth);
    let v = select(normalize(-p), vec3<f32>(0.0, 0.0, -1.0), shade.viewport.z <= 0.0);
    let radius = shade.ao.y;
    let per_unit = select(shade.viewport.y / (2.0 * shade.ao.x), shade.viewport.y / (2.0 * shade.viewport.z * depth), shade.viewport.z > 0.0);
    let reach = min(radius * per_unit, shade.viewport.y * 0.25);
    if (reach < 1.5) {
        return OPEN;
    }
    let moving = shade.occlusion.x > 0.5;
    let slices = select(2, 1, moving);
    let steps = select(8, 4, moving);
    let turn = gradient_noise(in.pos.xy);
    let shift = gradient_noise(in.pos.yx * 1.31 + 17.17);
    var visibility = 0.0;
    var bent = vec3<f32>(0.0);
    for (var slice = 0; slice < slices; slice++) {
        let phi = (f32(slice) + turn) * 3.1415927 / f32(slices);
        let omega = vec2<f32>(cos(phi), sin(phi));
        let dir = vec3<f32>(omega.x, -omega.y, 0.0);
        let ortho = dir - v * dot(dir, v);
        let axis = normalize(cross(ortho, v));
        let projected = n - axis * dot(n, axis);
        let projected_len = length(projected);
        let cos_n = saturate(dot(projected, v) / max(projected_len, 1e-6));
        let angle_n = sign(dot(ortho, projected)) * fast_acos(cos_n);
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
        let h0 = angle_n + max(-fast_acos(horizon1) - angle_n, -PI_HALF);
        let h1 = angle_n + min(fast_acos(horizon0) - angle_n, PI_HALF);
        let sin_n = sin(angle_n);
        let arc0 = (cos_n + 2.0 * h0 * sin_n - cos(2.0 * h0 - angle_n)) * 0.25;
        let arc1 = (cos_n + 2.0 * h1 * sin_n - cos(2.0 * h1 - angle_n)) * 0.25;
        visibility += projected_len * (arc0 + arc1);
        let t0 = (6.0 * sin(h0 - angle_n) - sin(3.0 * h0 - angle_n) + 6.0 * sin(h1 - angle_n) - sin(3.0 * h1 - angle_n)
            + 16.0 * sin_n - 3.0 * (sin(h0 + angle_n) + sin(h1 + angle_n))) / 12.0;
        let t1 = (-cos(3.0 * h0 - angle_n) - cos(3.0 * h1 - angle_n) + 8.0 * cos(angle_n)
            - 3.0 * (cos(h0 + angle_n) + cos(h1 + angle_n))) / 12.0;
        bent += projected_len * (normalize(ortho) * t0 + v * t1);
    }
    visibility /= f32(slices);
    let open = pow(saturate(visibility), shade.ao.z);
    let b = select(n, normalize(bent), dot(bent, bent) > 1e-10);
    return vec4<f32>(b * 0.5 + 0.5, open);
}

fn prepass_normal(uv: vec2<f32>) -> vec3<f32> {
    let size = vec2<f32>(textureDimensions(tex_a));
    let at = clamp(vec2<i32>(uv * size), vec2<i32>(0), vec2<i32>(size) - 1);
    return textureLoad(tex_a, at, 0).xyz;
}

// Smooths the occlusion along `delta` without bleeding across edges: taps
// whose surface faces another way count for less (the desktop's blur).
fn blur(uv: vec2<f32>, delta: vec2<f32>) -> vec4<f32> {
    let n0 = prepass_normal(uv);
    var offsets = array<f32, 5>(0.0, -1.3846153846, 1.3846153846, -3.2307692308, 3.2307692308);
    var weights = array<f32, 5>(0.2270270270, 0.3162162162, 0.3162162162, 0.0702702703, 0.0702702703);
    var sum = vec4<f32>(0.0);
    var total = 0.0;
    for (var i = 0; i < 5; i++) {
        let at = uv + delta * offsets[i];
        var w = weights[i];
        if (i > 0) {
            w *= smoothstep(0.8, 1.0, dot(n0, prepass_normal(at)));
        }
        sum += textureSampleLevel(tex_b, smp, at, 0.0) * w;
        total += w;
    }
    return sum / total;
}

@fragment
fn fs_blur_across(in: PostOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(tex_b));
    return blur(in.uv, vec2<f32>(texel.x, 0.0));
}

@fragment
fn fs_blur_down(in: PostOut) -> @location(0) vec4<f32> {
    let texel = 1.0 / vec2<f32>(textureDimensions(tex_b));
    return blur(in.uv, vec2<f32>(0.0, texel.y));
}
