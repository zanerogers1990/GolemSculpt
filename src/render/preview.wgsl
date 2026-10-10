// Preview meshes of the hosted tools: a tube being drawn, the solid a lathe
// profile makes, a peg. Drawn after the scene, inside the overlay pass, so
// they are hidden by whatever is in front of them (unless a draw asks to be
// on top). One mesh is drawn once per instance; each instance carries its own
// placement and colour. All lighting maths is in linear colour; see `encode`
// for how it is written out.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // w = 1 when the view is orthographic
    cam_pos: vec4<f32>,
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    cam_forward: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// The one place colour leaves this shader. The viewport target is a plain
// 8-bit format, so linear colour is sRGB-encoded on the way out.
fn encode(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

fn dither(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715)))) - 0.5;
}

fn view_dir(world: vec3<f32>) -> vec3<f32> {
    if (u.cam_pos.w > 0.5) {
        return -u.cam_forward.xyz;
    }
    return normalize(u.cam_pos.xyz - world);
}

// A direction given relative to the camera (x right, y up, z toward the
// viewer), in world space. Lights are fixed to the camera so the lit side is
// always the one being looked at.
fn from_view(v: vec3<f32>) -> vec3<f32> {
    return normalize(u.cam_right.xyz * v.x + u.cam_up.xyz * v.y - u.cam_forward.xyz * v.z);
}

struct In {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    // Per instance: the mesh's own space into world space, by columns.
    @location(2) model0: vec4<f32>,
    @location(3) model1: vec4<f32>,
    @location(4) model2: vec4<f32>,
    @location(5) model3: vec4<f32>,
    // Linear colour, straight alpha.
    @location(6) color: vec4<f32>,
    // x = 1: shade by face.
    @location(7) params: vec4<f32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
    @location(3) params: vec4<f32>,
};

@vertex
fn vs_preview(in: In) -> Out {
    let model = mat4x4<f32>(in.model0, in.model1, in.model2, in.model3);
    let world = model * vec4<f32>(in.position, 1.0);
    var out: Out;
    out.pos = u.view_proj * world;
    out.world = world.xyz;
    out.normal = (model * vec4<f32>(in.normal, 0.0)).xyz;
    out.color = in.color;
    out.params = in.params;
    return out;
}

// The shading normal, turned to face the viewer: the mesh's own, or the
// face's when the draw asked for flat shading.
fn surface_normal(in: Out, front: bool) -> vec3<f32> {
    let v = view_dir(in.world);
    if (in.params.x > 0.5) {
        let n = normalize(cross(dpdx(in.world), dpdy(in.world)));
        return select(-n, n, dot(n, v) >= 0.0);
    }
    let n = normalize(in.normal);
    return select(-n, n, front);
}

@fragment
fn fs_solid(in: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let n = surface_normal(in, front);
    let v = view_dir(in.world);
    let key = from_view(vec3<f32>(-0.49, 0.64, 0.59));
    let fill = from_view(vec3<f32>(0.75, 0.17, 0.63));
    let albedo = in.color.rgb;
    var color = albedo * (0.26 + 0.10 * n.y);
    color += albedo * 0.95 * max(dot(n, key), 0.0);
    color += albedo * 0.28 * max(dot(n, fill), 0.0);
    let h = normalize(key + v);
    color += vec3<f32>(0.105) * pow(max(dot(n, h), 0.0), 39.0);
    return vec4<f32>(encode(color) + dither(in.pos.xy) / 255.0, 1.0);
}

// A see-through shell: soft wrapped light and a rim. Edge-on areas go more
// opaque, which makes the shell read as having a silhouette instead of a
// vague haze.
@fragment
fn fs_shell(in: Out, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let n = surface_normal(in, front);
    let v = view_dir(in.world);
    let wrapped = clamp(dot(n, from_view(vec3<f32>(-0.4, 0.8, 0.5))) * 0.5 + 0.5, 0.0, 1.0);
    let fresnel = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 2.0);
    let body = in.color.rgb * mix(0.55, 1.0, wrapped);
    let rim = mix(in.color.rgb, vec3<f32>(1.0), 0.6);
    let out = mix(body, rim, fresnel * 0.8);
    let alpha = clamp(in.color.a + fresnel * (1.0 - in.color.a), 0.0, 1.0);
    return vec4<f32>(encode(out), alpha);
}

@fragment
fn fs_unlit(in: Out) -> @location(0) vec4<f32> {
    return vec4<f32>(encode(in.color.rgb), in.color.a);
}
