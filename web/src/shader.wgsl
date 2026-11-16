// GolemSculpt web: the model under a matcap, drawn off screen (with its
// wireframe over it when that is on), then copied into the page over the
// backdrop, or into a picture to save.

struct Globals {
    view_proj: mat4x4<f32>,
    view: mat4x4<f32>,
    // rgb: what masked parts are multiplied by; a: how strongly.
    mask_tint: vec4<f32>,
    // x: flat shading, y: the page wants linear colour (an sRGB target),
    // z: show backfaces, w: cavity on.
    params: vec4<f32>,
    // rgb: the backdrop; a: cavity's ridge (top) and valley (bottom).
    bg_top: vec4<f32>,
    bg_bottom: vec4<f32>,
    // The camera: where it is (w: 1 for an orthographic view), the way it
    // looks, and its right and up (w: how far across and up the view
    // reaches at one unit in front of the eye, for the backdrop's gradient).
    eye: vec4<f32>,
    forward: vec4<f32>,
    right: vec4<f32>,
    up: vec4<f32>,
    // x: the backdrop is a gradient, y: the gradient's bias, z: leave the
    // backdrop out (a picture with a clear background), w: the wireframe's
    // opacity.
    look: vec4<f32>,
    // xyz: 1 where that mirror plane's guide is shown.
    guides: vec4<f32>,
    // The Effects card: saturation, contrast (1 unchanged), the vignette's
    // strength (0 none) and smoothness.
    grade: vec4<f32>,
    // x: film grain (0 none).
    extras: vec4<f32>,
};

@group(0) @binding(0) var<uniform> g: Globals;
@group(0) @binding(1) var matcap_tex: texture_2d<f32>;
@group(0) @binding(2) var matcap_smp: sampler;

// One drawn thing: the model, or a tool's preview at one of its places.
struct Object {
    model: mat4x4<f32>,
    // The model's normals into world space: the inverse transpose of
    // `model`, so an object stretched unevenly still shades true.
    normal: mat4x4<f32>,
    // rgb: the colour the matcap is tinted toward; a: how far.
    tint: vec4<f32>,
};
@group(1) @binding(0) var<uniform> obj: Object;

struct MeshIn {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) mask: f32,
    // Recess (above 0.5) or ridge (below), from the mesh's own curvature.
    @location(3) curv: f32,
    // The paint: sRGB colour and how much of the surface it covers.
    @location(4) paint: vec4<f32>,
    // The face group's colour, and 1 where Show Group Colours tints it.
    @location(5) group: vec4<f32>,
};

struct MeshOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) normal: vec3<f32>,
    @location(1) world: vec3<f32>,
    @location(2) mask: f32,
    @location(3) curv: f32,
    @location(4) paint: vec4<f32>,
    @location(5) group: vec4<f32>,
};

@vertex
fn vs_mesh(in: MeshIn) -> MeshOut {
    var out: MeshOut;
    let world = (obj.model * vec4<f32>(in.position, 1.0)).xyz;
    out.clip = g.view_proj * vec4<f32>(world, 1.0);
    out.normal = (obj.normal * vec4<f32>(in.normal, 0.0)).xyz;
    out.world = world;
    out.mask = in.mask;
    out.curv = in.curv;
    out.paint = in.paint;
    out.group = in.group;
    return out;
}

@fragment
fn fs_mesh(in: MeshOut, @builtin(front_facing) facing: bool) -> @location(0) vec4<f32> {
    // A place that mirrors (a live mirror copy) turns the triangles' winding
    // round, which the object's normal matrix says in its unused corner.
    let front = facing != (obj.normal[3][3] < 0.0);
    // How the position changes from pixel to pixel, worked out before any
    // branch, for the finishes' patterns and the mirror guides.
    let world_dx = dpdx(in.world);
    let world_dy = dpdy(in.world);
    let world_width = abs(world_dx) + abs(world_dy);
    var n = normalize(in.normal);
    if (g.params.x > 0.5) {
        // Flat shading: the facet's own normal, to see the mesh's density.
        let f = normalize(cross(world_dy, world_dx));
        n = select(-f, f, dot(f, n) >= 0.0);
    }
    if (!front && g.params.z < 0.5) {
        // Show Backfaces off: an open shell is seen straight through, as on the desktop.
        discard;
    }
    n = select(-n, n, front);
    // A photo of a sphere shaded as the surface should look, indexed by the
    // normal in view space, as the desktop app does.
    let vn = normalize((g.view * vec4<f32>(n, 0.0)).xyz);
    let uv = vec2<f32>(vn.x * 0.5 + 0.5, 0.5 - vn.y * 0.5);
    var color = textureSampleGrad(matcap_tex, matcap_smp, uv, dpdx(uv), dpdy(uv)).rgb;
    // A tint keeps the matcap's light and shade: its grey is taken toward the colour.
    let grey = dot(color, vec3<f32>(0.3, 0.59, 0.11));
    color = mix(color, grey * obj.tint.rgb * 1.6, obj.tint.a);
    if (fin.kind.x > 0.5 && obj.tint.a == 0.0) {
        // A material finish (finish.wgsl) on the model, lit instead of a matcap.
        var s: Surface;
        s.world = in.world;
        s.v = normalize(g.eye.xyz - in.world);
        if (g.eye.w > 0.5) {
            s.v = -g.forward.xyz;
        }
        s.p = in.world;
        s.footprint = length(world_width);
        s.p_dx = world_dx;
        s.p_dy = world_dy;
        s.scale = vec3<f32>(1.0);
        s.mx = vec3<f32>(1.0, 0.0, 0.0);
        s.my = vec3<f32>(0.0, 1.0, 0.0);
        s.mz = vec3<f32>(0.0, 0.0, 1.0);
        s.smooth_n = select(-normalize(in.normal), normalize(in.normal), front);
        s.n = n;
        s.curv = in.curv;
        s.pixel = in.clip.xy;
        color = finish_shade(s);
    }
    if (g.params.w > 0.5 && obj.tint.a == 0.0) {
        // Cavity: valleys darken and ridges catch a little light, which
        // makes sculpted detail readable. Only the model has curvature.
        let c = clamp((in.curv - 0.5) * 2.0, -1.0, 1.0);
        var f = 1.0;
        if (c > 0.0) {
            f = 1.0 - min(g.bg_bottom.a * c * 1.4, 0.85);
        } else {
            f = 1.0 + g.bg_top.a * (-c) * 0.9;
        }
        color *= f;
    }
    if (!front) {
        // The inside of an open shell, a little darker so it reads as such.
        color *= 0.6;
    }
    // The paint multiplies the shading, as the desktop's does under a matcap
    // (painted.wgsl): uncovered surface keeps its own colour.
    color *= mix(vec3<f32>(1.0), in.paint.rgb, clamp(in.paint.a, 0.0, 1.0));
    // Face group colours over the picture, as the desktop's overlay blends
    // them (groups.rs): the picture tinted by the group's colour, lifted a
    // little so the colour still reads on a dark material.
    color = mix(color, in.group.rgb * (0.22 + color), clamp(in.group.a, 0.0, 1.0));
    color = mix(color, color * g.mask_tint.rgb, clamp(in.mask * g.mask_tint.a, 0.0, 1.0));
    if (obj.tint.a == 0.0) {
        // Mirror plane guides, as on the desktop: a thin tinted line where
        // each plane symmetry mirrors across cuts the model. The model sits
        // where its own space is, so the planes are x = 0, y = 0 and z = 0.
        let w = world_width;
        var axis_color = array<vec3<f32>, 3>(
            vec3<f32>(1.0, 0.05, 0.05), vec3<f32>(0.1, 1.0, 0.1), vec3<f32>(0.08, 0.26, 1.0));
        for (var i = 0; i < 3; i++) {
            if (g.guides[i] > 0.5) {
                let d = abs(in.world[i]) / max(w[i], 1e-9);
                let guide = 1.0 - smoothstep(0.4, 1.1, d);
                color = mix(color, axis_color[i], guide * 0.35);
            }
        }
    }
    return vec4<f32>(color, 1.0);
}

// ---- wireframe ------------------------------------------------------------------

// How far toward the eye the lines sit, as a share of their distance, or
// they would fight the triangles they lie on for depth (as the desktop's).
const WIRE_LIFT: f32 = 0.0015;

@vertex
fn vs_wire(@location(0) position: vec3<f32>) -> @builtin(position) vec4<f32> {
    let world = (obj.model * vec4<f32>(position, 1.0)).xyz;
    let to_eye = g.eye.xyz - world;
    let dist = length(to_eye);
    var toward = to_eye / max(dist, 1e-6);
    if (g.eye.w > 0.5) {
        toward = -g.forward.xyz;
    }
    return g.view_proj * vec4<f32>(world + toward * dist * WIRE_LIFT, 1.0);
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    // Near black, see-through; premultiplied, as the model's picture is.
    let a = g.look.w;
    return vec4<f32>(vec3<f32>(0.02, 0.025, 0.035) * a, a);
}

// ---- copy into the page -------------------------------------------------------

@group(0) @binding(3) var shot: texture_2d<f32>;

struct BlitOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_blit(@builtin(vertex_index) i: u32) -> BlitOut {
    // One triangle that covers the whole rectangle.
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: BlitOut;
    out.pos = vec4<f32>(p * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, 1.0 - p.y);
    return out;
}

fn to_linear(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + 0.055) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

@fragment
fn fs_blit(in: BlitOut) -> @location(0) vec4<f32> {
    let size = vec2<f32>(textureDimensions(shot));
    let texel = vec2<i32>(clamp(in.uv * size, vec2<f32>(0.0), size - 1.0));
    let model = textureLoad(shot, texel, 0);
    if (g.look.z > 0.5) {
        // A picture with a clear background: the model alone, its colour
        // no longer premultiplied, as a PNG keeps it.
        let rgb = select(model.rgb / max(model.a, 1e-6), vec3<f32>(0.0), model.a <= 0.0);
        return vec4<f32>(rgb, model.a);
    }
    var bg = g.bg_bottom.rgb;
    if (g.look.x > 0.5) {
        // A sky-dome gradient by the way each pixel looks, as the desktop's,
        // so it stays put as the view turns; straight up the screen when
        // the view is orthographic.
        let ndc = vec2<f32>(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
        var t = ndc.y * 0.5 + 0.5;
        if (g.eye.w < 0.5) {
            let dir = normalize(g.forward.xyz + g.right.xyz * (ndc.x * g.right.w) + g.up.xyz * (ndc.y * g.up.w));
            t = clamp(dir.y * 0.5 + 0.5, 0.0, 1.0);
        }
        t = pow(t, 1.0 / max(g.look.y, 0.001));
        bg = mix(g.bg_bottom.rgb, g.bg_top.rgb, t);
    }
    // The model was drawn over nothing (alpha 0), so its edges are
    // premultiplied by how much of each pixel it covers.
    var color = model.rgb + bg * (1.0 - model.a);
    color = effects(color, model.a, in.uv, in.pos.xy);
    if (g.params.y > 0.5) {
        color = to_linear(color);
    }
    return vec4<f32>(color, 1.0);
}

// A number in -0.5..0.5 with no pattern in it, for film grain.
fn grain_noise(p: vec2<f32>) -> f32 {
    let q = fract(p * vec2<f32>(0.1031, 0.1030));
    let r = q + dot(q, q.yx + 33.33);
    return fract((r.x + r.y) * r.x) - 0.5;
}

fn to_display_srgb(c: vec3<f32>) -> vec3<f32> {
    let x = clamp(c, vec3<f32>(0.0), vec3<f32>(1.0));
    let lo = x * 12.92;
    let hi = 1.055 * pow(x, vec3<f32>(1.0 / 2.4)) - 0.055;
    return select(hi, lo, x <= vec3<f32>(0.0031308));
}

// The Effects card on a display colour (the desktop's post shader, in its
// order): the vignette over everything, saturation and contrast on the model
// only (a backdrop colour stays as picked), then film grain over everything.
fn effects(display: vec3<f32>, coverage: f32, uv: vec2<f32>, pos: vec2<f32>) -> vec3<f32> {
    let grading = g.grade.x != 1.0 || g.grade.y != 1.0;
    if (g.grade.z <= 0.0 && !grading && g.extras.x <= 0.0) {
        return display;
    }
    var color = to_linear(display);
    if (g.grade.z > 0.0) {
        let dist = abs(uv - 0.5) * g.grade.z;
        color *= pow(saturate(1.0 - dot(dist, dist)), g.grade.w);
    }
    if (grading && coverage > 0.0) {
        let as_picked = color;
        // Contrast pivots on mid grey in a log encoding, as film does.
        let lg = 0.244161 * log(5.555556 * max(color, vec3<f32>(0.0)) + 0.047996) / log(10.0) + 0.386036;
        let contrasted = (lg - 0.4135884) * g.grade.y + 0.4135884;
        var graded = (pow(vec3<f32>(10.0), (contrasted - 0.386036) / 0.244161) - 0.047996) / 5.555556;
        let luma = dot(graded, vec3<f32>(0.2126729, 0.7151522, 0.0721750));
        graded = max(vec3<f32>(luma) + g.grade.x * (graded - luma), vec3<f32>(0.0));
        color = mix(as_picked, graded, coverage);
    }
    if (g.extras.x > 0.0) {
        // Film grain: strongest in the mid tones, as on film.
        let luma = dot(color, vec3<f32>(0.2126729, 0.7151522, 0.0721750));
        let amount = g.extras.x * 0.12 * (1.0 - 0.7 * abs(saturate(luma) * 2.0 - 1.0));
        color = max(color * (1.0 + grain_noise(pos) * amount * 4.0) + grain_noise(pos.yx + 7.7) * amount * 0.25, vec3<f32>(0.0));
    }
    return to_display_srgb(color);
}
