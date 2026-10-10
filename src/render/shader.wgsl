// Viewport shading, first half: the shared inputs, the mesh vertex stage, the
// normals prepass and the screen-space cavity. The surface shading itself is
// in sculpt_pbr.wgsl, which is compiled together with this file. Colour is
// linear and unclamped here; post.wgsl tone-maps and encodes it.

// The finishes take screen-space derivatives of patterns inside branches, as
// the shader they are ported from does.
diagnostic(off, derivative_uniformity);

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    cam_pos: vec4<f32>,
    cam_right: vec4<f32>,
    cam_up: vec4<f32>,
    cam_forward: vec4<f32>,
    // xyz base colour (linear), w smoothness
    base_color: vec4<f32>,
    // rgb linear; bg_bottom.w = gradient on, bg_top.w = gradient bias
    bg_bottom: vec4<f32>,
    bg_top: vec4<f32>,
    // x matcap on, y flat shading, z cavity ridge, w cavity valley
    shading: vec4<f32>,
    // xyz = 1 where that mirror plane's guide is shown, w cavity offset px
    symmetry: vec4<f32>,
    // xy viewport px, z tan(half fov) (0 = orthographic), w aspect
    viewport: vec4<f32>,
    light_dir: array<vec4<f32>, 3>,
    light_color: array<vec4<f32>, 3>,
    ambient_sky: vec4<f32>,
    ambient_equator: vec4<f32>,
    ambient_ground: vec4<f32>,
    // The radial symmetry guide. xyz the axis in the active object's own
    // space, w how many spokes go round it (0 = no guide)
    radial_axis: vec4<f32>,
    // xyz the way the first spoke points, w its colour (0 1 2 = the X Y Z axis
    // colours, 3 = a custom axis)
    radial_spoke: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var matcap_tex: texture_2d<f32>;
@group(0) @binding(2) var matcap_smp: sampler;
@group(0) @binding(3) var gbuf: texture_2d<f32>;

fn to_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = max(c, vec3<f32>(0.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

// Cheap screen-space noise to break up 8-bit banding in smooth gradients.
fn dither(p: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(p, vec2<f32>(0.06711056, 0.00583715)))) - 0.5;
}

// ---- background -----------------------------------------------------------

struct BgOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) ndc: vec2<f32>,
};

@vertex
fn vs_background(@builtin(vertex_index) i: u32) -> BgOut {
    var out: BgOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    // Depth 0 is the far plane in reverse depth.
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.ndc = p;
    return out;
}

// ---- mesh -----------------------------------------------------------------

struct VsIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    // Recess (above 0.5) or ridge (below), from the mesh's own curvature.
    @location(7) curv: f32,
    // Per object: its matrix by columns. The last column's w is 1 on the
    // active object.
    @location(3) model_x: vec4<f32>,
    @location(4) model_y: vec4<f32>,
    @location(5) model_z: vec4<f32>,
    @location(6) model_w: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    // Position in the object's own space, where its mirror planes are.
    @location(3) local: vec3<f32>,
    @location(4) is_active: f32,
    @location(5) curv: f32,
    // The object's matrix, for patterns that live in its own space.
    @location(6) @interpolate(flat) model_x: vec3<f32>,
    @location(7) @interpolate(flat) model_y: vec3<f32>,
    @location(8) @interpolate(flat) model_z: vec3<f32>,
};

@vertex
fn vs_mesh(in: VsIn) -> VsOut {
    var out: VsOut;
    let x = in.model_x.xyz;
    let y = in.model_y.xyz;
    let z = in.model_z.xyz;
    let world = x * in.position.x + y * in.position.y + z * in.position.z + in.model_w.xyz;
    out.pos = g.view_proj * vec4<f32>(world, 1.0);
    out.world = world;
    out.local = in.position;
    out.is_active = in.model_w.w;
    // Normals go through the cofactors, so an unevenly scaled object still
    // shades as the shape it now is.
    let n = in.normal;
    out.normal = cross(y, z) * n.x + cross(z, x) * n.y + cross(x, y) * n.z;
    out.mask = in.mask;
    out.curv = in.curv;
    out.model_x = x;
    out.model_y = y;
    out.model_z = z;
    return out;
}

fn shading_normal(in: VsOut, front: bool) -> vec3<f32> {
    var n = normalize(in.normal);
    if (g.shading.y > 0.5) {
        // Flat shading: the true facet normal, to judge mesh density.
        let f = normalize(cross(dpdy(in.world), dpdx(in.world)));
        n = select(-f, f, dot(f, n) >= 0.0);
    }
    return select(-n, n, front);
}

// View-space normal + eye depth, read back by the cavity term.
@fragment
fn fs_prepass(in: VsOut, @builtin(front_facing) front: bool) -> @location(0) vec4<f32> {
    let n = shading_normal(in, front);
    let vn = (g.view * vec4<f32>(n, 0.0)).xyz;
    let depth = dot(in.world - g.cam_pos.xyz, g.cam_forward.xyz);
    return vec4<f32>(vn, max(depth, 1e-6));
}

fn soft_clamp(c: f32, k: f32) -> f32 {
    if (c < 0.5 / k) {
        return c * (1.0 - c * k);
    }
    return 0.25 / k;
}

// Screen-space curvature (after Blender's Workbench): ridges catch a little
// light and valleys darken, which makes sculpted detail readable.
fn cavity(frag: vec2<f32>) -> f32 {
    let ridge = g.shading.z;
    let valley = g.shading.w;
    if (ridge <= 0.0 && valley <= 0.0) {
        return 1.0;
    }
    let size = vec2<i32>(textureDimensions(gbuf));
    let p = vec2<i32>(frag);
    let o = i32(g.symmetry.w);
    let lim = size - vec2<i32>(1, 1);
    let centre = textureLoad(gbuf, clamp(p, vec2<i32>(0, 0), lim), 0);
    if (centre.w <= 0.0) {
        return 1.0;
    }
    let up = textureLoad(gbuf, clamp(p + vec2<i32>(0, -o), vec2<i32>(0, 0), lim), 0);
    let down = textureLoad(gbuf, clamp(p + vec2<i32>(0, o), vec2<i32>(0, 0), lim), 0);
    let left = textureLoad(gbuf, clamp(p + vec2<i32>(-o, 0), vec2<i32>(0, 0), lim), 0);
    let right = textureLoad(gbuf, clamp(p + vec2<i32>(o, 0), vec2<i32>(0, 0), lim), 0);
    // Across a silhouette or a depth step the normals are unrelated.
    let tol = centre.w * 0.05;
    if (abs(up.w - centre.w) > tol || abs(down.w - centre.w) > tol
        || abs(left.w - centre.w) > tol || abs(right.w - centre.w) > tol) {
        return 1.0;
    }
    let d = (up.y - down.y) + (right.x - left.x);
    var f = 1.0;
    if (d < 0.0) {
        if (valley > 0.0) {
            f = 1.0 - 2.0 * soft_clamp(-d, 0.7 / (valley * valley));
        }
    } else if (ridge > 0.0) {
        f = 1.0 + 2.0 * soft_clamp(d, 0.5 / (ridge * ridge));
    }
    return clamp(f, 0.0, 4.0);
}
