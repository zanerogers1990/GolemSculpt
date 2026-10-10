// The SSphere tool's layer: the armature (spheres and banded links), the
// placement ghost, the mirror plane, and the skin as a see-through shell or
// a solid preview. Drawn after the scene, inside the same pass, so it is
// hidden by whatever is in front of it. All lighting maths is in linear
// colour; see `encode` for how it is written out.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // The rig's own space into the world: a move, a turn and an even resize.
    // Only the skin needs it; the armature arrives in world space.
    model: mat4x4<f32>,
    // w = 1 when the view is orthographic
    cam_pos: vec4<f32>,
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    cam_forward: vec4<f32>,
    // x: how far the skin's shell is lifted toward the eye
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// Linear colours of the skin: the shell, its rim, and the solid preview.
const SHELL: vec4<f32> = vec4<f32>(0.913, 0.604, 0.420, 0.28);
const SHELL_RIM: vec3<f32> = vec3<f32>(1.0, 0.868, 0.748);
const SOLID: vec3<f32> = vec3<f32>(0.604, 0.214, 0.147);

// The one place colour leaves this shader: every fragment output goes
// through here. The viewport target is a plain 8-bit format, so linear colour
// is sRGB-encoded on the way out.
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

fn lit(n: vec3<f32>, world: vec3<f32>, albedo: vec3<f32>, smoothness: f32) -> vec3<f32> {
    let v = view_dir(world);
    let key = from_view(vec3<f32>(-0.49, 0.64, 0.59));
    let fill = from_view(vec3<f32>(0.75, 0.17, 0.63));
    var color = albedo * (0.26 + 0.10 * n.y);
    color += albedo * 0.95 * max(dot(n, key), 0.0);
    color += albedo * 0.28 * max(dot(n, fill), 0.0);
    let h = normalize(key + v);
    color += vec3<f32>(0.35 * smoothness) * pow(max(dot(n, h), 0.0), 12.0 + 90.0 * smoothness);
    return color;
}

// A see-through shell: soft wrapped light and a rim. Edge-on areas go more
// opaque, which makes the shell read as having a silhouette instead of a
// vague haze.
fn shell(n: vec3<f32>, world: vec3<f32>, color: vec4<f32>, rim: vec3<f32>) -> vec4<f32> {
    let v = view_dir(world);
    let wrapped = clamp(dot(n, from_view(vec3<f32>(-0.4, 0.8, 0.5))) * 0.5 + 0.5, 0.0, 1.0);
    let fresnel = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 2.0);
    let body = color.rgb * mix(0.55, 1.0, wrapped);
    let out = mix(body, rim, fresnel * 0.8);
    let alpha = clamp(color.a + fresnel * (1.0 - color.a), 0.0, 1.0);
    return vec4<f32>(encode(out), alpha);
}

// ---- armature, ghost and mirror plane ---------------------------------------

struct ArmIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

struct ArmOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) color: vec4<f32>,
};

@vertex
fn vs_armature(in: ArmIn) -> ArmOut {
    var out: ArmOut;
    out.pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.world = in.position;
    out.normal = in.normal;
    out.color = in.color;
    return out;
}

@fragment
fn fs_armature(in: ArmOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    // A zero normal marks flat, unlit geometry (the mirror plane).
    if (dot(in.normal, in.normal) < 0.25) {
        return vec4<f32>(encode(in.color.rgb), in.color.a);
    }
    let n = normalize(in.normal);
    if (in.color.a < 0.999) {
        return shell(select(-n, n, front), in.world, in.color, mix(in.color.rgb, vec3<f32>(1.0), 0.6));
    }
    return vec4<f32>(encode(lit(n, in.world, in.color.rgb, 0.35)) + dither(in.pos.xy) / 255.0, 1.0);
}

// ---- skin -------------------------------------------------------------------

struct SkinIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

struct SkinOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
};

// The skin's normal in world space. The resize in `model` is even, so the
// matrix turns a normal correctly and only its length is off; the fragment
// stage normalises it.
fn skin_normal(n: vec3<f32>) -> vec3<f32> {
    return (u.model * vec4<f32>(n, 0.0)).xyz;
}

@vertex
fn vs_skin(in: SkinIn) -> SkinOut {
    var out: SkinOut;
    let world = (u.model * vec4<f32>(in.position, 1.0)).xyz;
    out.pos = u.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.normal = skin_normal(in.normal);
    return out;
}

// The shell, lifted a sliver toward the eye so it wins cleanly against the
// armature spheres it lies exactly on.
@vertex
fn vs_skin_shell(in: SkinIn) -> SkinOut {
    var out: SkinOut;
    let world = (u.model * vec4<f32>(in.position, 1.0)).xyz;
    out.pos = u.view_proj * vec4<f32>(world + view_dir(world) * u.params.x, 1.0);
    out.world = world;
    out.normal = skin_normal(in.normal);
    return out;
}

@fragment
fn fs_skin_shell(in: SkinOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    return shell(select(-n, n, front), in.world, SHELL, SHELL_RIM);
}

@fragment
fn fs_skin_solid(in: SkinOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let n = normalize(in.normal);
    let color = lit(select(-n, n, front), in.world, SOLID, 0.3);
    return vec4<f32>(encode(color) + dither(in.pos.xy) / 255.0, 1.0);
}
