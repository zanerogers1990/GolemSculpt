// ---- ray tracing (render/ray.rs) -------------------------------------------
//
// One pass over the picture before it is shaded, tracing against the card's
// own acceleration structures what the shading cannot see from the screen:
// whether each light reaches each pixel, the sky's light and the light
// bounced off the model, what the reflection meets, and what is seen through
// resin. Each sample of a render traces one ray of each kind per pixel, a
// different one each sample, and the samples' average is the picture.
//
// Compiled after shader.wgsl and sculpt_pbr.wgsl (whose lights, sky and
// surroundings it reads), with the ray query extensions enabled ahead of
// both. The surface shading reads what this writes (`look.rays`).

// What a ray that meets the model sees it made of, per object: its colour on
// the whole and how metallic and smooth it is (ray.rs, `hit_material`).
struct HitMaterial {
    // rgb colour (linear), a metallic
    color: vec4<f32>,
    // x smoothness
    params: vec4<f32>,
};

struct TraceParams {
    // Clip space back to the world, for the view as this sample draws it.
    unproject: mat4x4<f32>,
    // x the sample, y 1 when the view through resin is traced, z how far a
    // ray reaches, w the roughness reflections are traced with (GGX alpha)
    params: vec4<f32>,
};

@group(1) @binding(0) var scene: acceleration_structure<vertex_return>;
@group(1) @binding(1) var<storage, read> hit_materials: array<HitMaterial>;
@group(1) @binding(2) var<uniform> trace: TraceParams;
// Whether lights 0 to 3 reach the pixel (1) or not (0).
@group(1) @binding(3) var out_lights: texture_storage_2d<rgba8unorm, write>;
// rgb the light arriving from one direction over the surface (the sky's, or
// what the ray met of the model), a whether light 4 reaches it.
@group(1) @binding(4) var out_diffuse: texture_storage_2d<rgba16float, write>;
// rgb the light of what the reflection met, a 1 when it met nothing (and the
// surroundings show), -1 where there is no surface.
@group(1) @binding(5) var out_specular: texture_storage_2d<rgba16float, write>;
// rgb what is seen through the resin, a how much resin the view crosses
// (-1 when it was not traced).
@group(1) @binding(6) var out_through: texture_storage_2d<rgba16float, write>;

// ---- random numbers ---------------------------------------------------------

// Two numbers in 0..1 from three integers (a PCG hash).
fn ray_hash(p: vec3<u32>) -> vec2<f32> {
    var v = p * 1664525u + 1013904223u;
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    v = v ^ (v >> vec3<u32>(16u));
    v.x += v.y * v.z;
    v.y += v.z * v.x;
    v.z += v.x * v.y;
    return vec2<f32>(v.xy >> vec2<u32>(8u)) / 16777216.0;
}

// Two numbers in 0..1 for one of the rays a pixel starts (`dim`): spread
// evenly over the samples (an R2 sequence), the sequence shifted by a
// different amount for each pixel and each ray. Every ray's sequence is the
// same one shifted, so two rays' choices go together: each ray's own
// average comes out right, but a choice made along a ray (where it meets,
// which light lights that) takes `ray_noise` instead.
fn ray_random(pixel: vec2<u32>, dim: u32) -> vec2<f32> {
    return ray_random_at(pixel, dim, trace.params.x);
}

// The same, for the `k`th of `count` rays of one kind a sample traces: the
// sequence carries on from ray to ray and sample to sample.
fn ray_random_of(pixel: vec2<u32>, dim: u32, k: u32, count: u32) -> vec2<f32> {
    return ray_random_at(pixel, dim, trace.params.x * f32(count) + f32(k));
}

fn ray_random_at(pixel: vec2<u32>, dim: u32, n: f32) -> vec2<f32> {
    let even = fract(vec2<f32>(0.5) + n * vec2<f32>(0.7548776662, 0.5698402910));
    return fract(even + ray_hash(vec3<u32>(pixel, dim)));
}

// How many rays of a kind each pixel traces each sample: the soft shadows
// and the light from around want the most to come out clean.
const LIGHT_RAYS: u32 = 2u;
const AROUND_RAYS: u32 = 2u;

// Two numbers in 0..1, unrelated to any other choice, for each pixel, each
// `dim` and each sample.
fn ray_noise(pixel: vec2<u32>, dim: u32) -> vec2<f32> {
    return ray_hash(vec3<u32>(pixel, dim | (u32(trace.params.x) << 8u)));
}

// ---- directions -------------------------------------------------------------

// Two axes across `n`, and `n` (Duff and others, 2017).
fn ray_frame(n: vec3<f32>) -> mat3x3<f32> {
    let s = select(-1.0, 1.0, n.z >= 0.0);
    let a = -1.0 / (s + n.z);
    let b = n.x * n.y * a;
    let t = vec3<f32>(1.0 + s * n.x * n.x * a, s * b, -s * n.x);
    let u = vec3<f32>(b, s + n.y * n.y * a, -n.y);
    return mat3x3<f32>(t, u, n);
}

// A direction over `n`, as likely as the light a matte surface takes from it.
fn cosine_toward(n: vec3<f32>, u: vec2<f32>) -> vec3<f32> {
    let r = sqrt(u.x);
    let turn = TAU * u.y;
    let local = vec3<f32>(r * cos(turn), r * sin(turn), sqrt(max(1.0 - u.x, 0.0)));
    return ray_frame(n) * local;
}

// A direction the reflection of `v` off a surface of roughness `a` takes, as
// likely as the microfacets that face it are seen from `v` (Heitz, "Sampling
// the GGX distribution of visible normals", 2018).
fn ggx_reflection(n: vec3<f32>, v: vec3<f32>, a: f32, u: vec2<f32>) -> vec3<f32> {
    let f = ray_frame(n);
    let ve = transpose(f) * v;
    let vh = normalize(vec3<f32>(a * ve.x, a * ve.y, max(ve.z, 1e-4)));
    let across = vh.x * vh.x + vh.y * vh.y;
    let t1 = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(-vh.y, vh.x, 0.0) * inverseSqrt(across), across > 1e-7);
    let t2 = cross(vh, t1);
    let r = sqrt(u.x);
    let turn = TAU * u.y;
    let p1 = r * cos(turn);
    let blend = 0.5 * (1.0 + vh.z);
    let p2 = (1.0 - blend) * sqrt(max(1.0 - p1 * p1, 0.0)) + blend * r * sin(turn);
    let nh = p1 * t1 + p2 * t2 + sqrt(max(1.0 - p1 * p1 - p2 * p2, 0.0)) * vh;
    let h = normalize(vec3<f32>(a * nh.x, a * nh.y, max(nh.z, 0.0)));
    return f * reflect(-ve, h);
}

// How far off a surface at `at` a ray starts, so it does not meet the
// surface it leaves: a little more for points far from the middle, where
// numbers are coarser.
fn ray_offset(at: vec3<f32>) -> f32 {
    return 2e-5 * (1.0 + max(abs(at.x), max(abs(at.y), abs(at.z))));
}

// ---- tracing ----------------------------------------------------------------

// Whether nothing of the model is between `start` and `far` along `dir`.
fn ray_clear(start: vec3<f32>, dir: vec3<f32>, far: f32) -> bool {
    var q: ray_query<vertex_return>;
    rayQueryInitialize(&q, scene, RayDesc(RAY_FLAG_TERMINATE_ON_FIRST_HIT, 0xFFu, 0.0, far, start, dir));
    while (rayQueryProceed(&q)) {}
    return rayQueryGetCommittedIntersection(&q).kind == RAY_QUERY_INTERSECTION_NONE;
}

struct Hit {
    found: bool,
    // The floor, not the model.
    floor: bool,
    t: f32,
    at: vec3<f32>,
    // The face's own normal, turned back toward where the ray came from.
    normal: vec3<f32>,
    material: HitMaterial,
};

// The nearest point of the model along `dir`, within `far`.
fn ray_closest(start: vec3<f32>, dir: vec3<f32>, far: f32) -> Hit {
    var q: ray_query<vertex_return>;
    rayQueryInitialize(&q, scene, RayDesc(RAY_FLAG_NONE, 0xFFu, 0.0, far, start, dir));
    while (rayQueryProceed(&q)) {}
    let hit = rayQueryGetCommittedIntersection(&q);
    var h: Hit;
    if (hit.kind != RAY_QUERY_INTERSECTION_TRIANGLE) {
        return h;
    }
    h.found = true;
    h.t = hit.t;
    h.at = start + dir * hit.t;
    // The triangle's corners in the object's own space, its normal from
    // them, and that into the world as shader.wgsl turns normals.
    let c = getCommittedHitVertexPositions(&q);
    let local = cross(c[1] - c[0], c[2] - c[0]);
    let m = hit.object_to_world;
    let n = normalize(cross(m[1], m[2]) * local.x + cross(m[2], m[0]) * local.y + cross(m[0], m[1]) * local.z);
    h.normal = select(n, -n, dot(n, dir) > 0.0);
    h.material = hit_materials[hit.instance_custom_data];
    return h;
}

// The nearest of the model or the floor (where it shows: its fading edge is
// met as often as it is opaque there).
fn ray_meet(start: vec3<f32>, dir: vec3<f32>, far: f32, pixel: vec2<u32>, dim: u32) -> Hit {
    var h = ray_closest(start, dir, far);
    if (look.floor.x > 0.5 && dir.y < -1e-6) {
        let t = (look.floor.y - start.y) / dir.y;
        if (t > 0.0 && (!h.found || t < h.t)) {
            let at = start + dir * t;
            if (floor_fade(at) > ray_noise(pixel, dim).x) {
                h.found = true;
                h.floor = true;
                h.t = t;
                h.at = at;
                h.normal = vec3<f32>(0.0, 1.0, 0.0);
            }
        }
    }
    return h;
}

// ---- light ------------------------------------------------------------------

// Toward light `i` from `at`, through a point of it picked by `u`: over a
// softbox's face, or over the disc a light far away covers (as wide as the
// key's own size). The direction, and how far the ray may go.
fn light_toward(i: i32, at: vec3<f32>, u: vec2<f32>) -> vec4<f32> {
    if (look.light_pos[i].w > 0.5) {
        let q = look.light_pos[i].xyz + look.light_side[i].xyz * (u.x * 2.0 - 1.0)
            + look.light_up[i].xyz * (u.y * 2.0 - 1.0);
        let to = q - at;
        let d = length(to);
        return vec4<f32>(to / max(d, 1e-9), d * 0.999);
    }
    let l = look.light_dir[i].xyz;
    let r = sqrt(u.x) * look.shadow_soft.z;
    let turn = TAU * u.y;
    let f = ray_frame(l);
    return vec4<f32>(normalize(l + f[0] * (r * cos(turn)) + f[1] * (r * sin(turn))), trace.params.z);
}

// The sky's light from one direction: the ambient light (sky_sun or the
// HDRI's harmonics, which `ambient` gives already spread over a hemisphere,
// as a matte surface sees them) narrowed back, so a matte surface averaging
// it over an open sky gets exactly `ambient` again. Each band of harmonics is
// spread by 1, 2/3 and 1/4: narrowing undoes that.
fn sky_light(d: vec3<f32>) -> vec3<f32> {
    if (look.env_params.x > 0.5) {
        return look.sh[0].rgb
            + (look.sh[1].rgb * d.y + look.sh[2].rgb * d.z + look.sh[3].rgb * d.x) * 1.5
            + (look.sh[4].rgb * (d.x * d.y) + look.sh[5].rgb * (d.y * d.z)
                + look.sh[6].rgb * (3.0 * d.z * d.z - 1.0) + look.sh[7].rgb * (d.x * d.z)
                + look.sh[8].rgb * (d.x * d.x - d.y * d.y)) * 4.0;
    }
    // `ambient`'s lobes are A + B y + C y^2: a constant A + C / 3, a first
    // band B y and a second C (y^2 - 1/3).
    let y = d.y;
    let second = 1.875 * (y * y - 1.0 / 3.0);
    let up = (0.25 + 0.75 * y + second) * (16.0 / 17.0);
    let down = (0.25 - 0.75 * y + second) * (16.0 / 17.0);
    let eq = look.ambient_equator.rgb;
    return eq + (look.ambient_sky.rgb - eq) * up + (look.ambient_ground.rgb - eq) * down;
}

// The light leaving a point a ray met, back along the ray: the surface's
// colour lit by each light that reaches it and by the sky, plus its own
// reflection of the surroundings. The floor gives the backdrop's colour,
// darker where the key's shadow falls on it.
fn hit_light(h: Hit, dir: vec3<f32>, pixel: vec2<u32>, dim: u32) -> vec3<f32> {
    let up = vec3<f32>(0.0, 1.0, 0.0);
    if (h.floor) {
        var color = g.bg_bottom.rgb;
        if (look.bg_params.x > 1.5) {
            color = ambient(up) * 0.5;
        }
        let l = light_toward(0, h.at, ray_noise(pixel, dim));
        let lit = select(0.0, 1.0, l.y <= 0.0 || ray_clear(h.at + up * ray_offset(h.at), l.xyz, l.w));
        return color * mix(1.0, lit, look.floor.w);
    }
    let m = h.material;
    let albedo = m.color.rgb * (1.0 - m.color.a);
    let f0 = mix(vec3<f32>(0.04), m.color.rgb, m.color.a);
    let start = h.at + h.normal * ray_offset(h.at);
    var light = ambient(h.normal);
    let count = light_count();
    for (var i = 0; i < count; i++) {
        let l = light_toward(i, h.at, ray_noise(pixel, dim + u32(i)));
        let nl = dot(h.normal, l.xyz);
        if (nl > 0.0 && ray_clear(start, l.xyz, l.w)) {
            var strength = look.light_color[i].rgb;
            if (look.light_pos[i].w > 0.5) {
                // A softbox's light falls off with distance as the shading has it.
                let c = look.light_pos[i].xyz - h.at;
                strength *= look.light_up[i].w / max(dot(c, c), 1e-12);
            }
            light += strength * nl;
        }
    }
    return albedo * light + f0 * environment(reflect(dir, h.normal), 1.0 - m.params.x);
}

// ---- the pass ---------------------------------------------------------------

@compute @workgroup_size(8, 8)
fn cs_trace(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(gbuf);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    let pixel = id.xy;
    let drawn = textureLoad(gbuf, vec2<i32>(pixel), 0);
    if (drawn.w <= 0.0) {
        // No surface: the shading keeps its own light here.
        textureStore(out_lights, pixel, vec4<f32>(1.0));
        textureStore(out_diffuse, pixel, vec4<f32>(0.0, 0.0, 0.0, 1.0));
        textureStore(out_specular, pixel, vec4<f32>(0.0, 0.0, 0.0, -1.0));
        textureStore(out_through, pixel, vec4<f32>(0.0, 0.0, 0.0, -1.0));
        return;
    }

    // Where the pixel's surface is: along the pixel's line of sight, as far
    // as its eye depth.
    let ndc = vec2<f32>((f32(pixel.x) + 0.5) / f32(size.x) * 2.0 - 1.0, 1.0 - (f32(pixel.y) + 0.5) / f32(size.y) * 2.0);
    let near = trace.unproject * vec4<f32>(ndc, 1.0, 1.0);
    let mid = trace.unproject * vec4<f32>(ndc, 0.5, 1.0);
    let a = near.xyz / near.w;
    let along = mid.xyz / mid.w - a;
    let forward = g.cam_forward.xyz;
    let world = a + along * ((drawn.w - dot(a - g.cam_pos.xyz, forward)) / dot(along, forward));
    var v = normalize(g.cam_pos.xyz - world);
    if (g.viewport.z <= 0.0) {
        v = -forward;
    }
    // The prepass normal is in view space: back to the world, facing the eye.
    let to_world = transpose(mat3x3<f32>(g.view[0].xyz, g.view[1].xyz, g.view[2].xyz));
    var n = normalize(to_world * drawn.xyz);
    n = select(n, -n, dot(n, v) < 0.0);
    // Off the surface by about a pixel.
    let pixel_size = select(2.0 * look.cam_params.x, 2.0 * g.viewport.z * drawn.w, g.viewport.z > 0.0) / f32(size.y);
    let lift = max(pixel_size, ray_offset(world));
    let origin = world + n * lift;
    let reach = trace.params.z;

    // Each light: rays to points of it. One that the surface faces away
    // from gives it no light either way.
    var reaches = array<f32, 5>(1.0, 1.0, 1.0, 1.0, 1.0);
    let count = light_count();
    for (var i = 0; i < count; i++) {
        var clear = 0.0;
        for (var k = 0u; k < LIGHT_RAYS; k++) {
            let l = light_toward(i, origin, ray_random_of(pixel, u32(i), k, LIGHT_RAYS));
            if (dot(l.xyz, n) <= 0.0 || ray_clear(origin, l.xyz, l.w)) {
                clear += 1.0;
            }
        }
        reaches[i] = clear / f32(LIGHT_RAYS);
    }
    textureStore(out_lights, pixel, vec4<f32>(reaches[0], reaches[1], reaches[2], reaches[3]));

    // The sky, or what hides it.
    var diffuse = vec3<f32>(0.0);
    for (var k = 0u; k < AROUND_RAYS; k++) {
        let d = cosine_toward(n, ray_random_of(pixel, 8u, k, AROUND_RAYS));
        let met = ray_meet(origin, d, reach, pixel, 40u + k);
        if (met.found) {
            diffuse += hit_light(met, d, pixel, 48u + 8u * k);
        } else {
            diffuse += sky_light(d);
        }
    }
    diffuse /= f32(AROUND_RAYS);
    textureStore(out_diffuse, pixel, vec4<f32>(diffuse, reaches[4]));

    // The reflection: the surroundings show where it meets nothing.
    var specular = vec4<f32>(0.0, 0.0, 0.0, 1.0);
    let r = ggx_reflection(n, v, max(trace.params.w, 0.002), ray_random(pixel, 12u));
    if (dot(r, n) > 0.0) {
        let mirrored = ray_meet(origin, r, reach, pixel, 13u);
        if (mirrored.found) {
            specular = vec4<f32>(hit_light(mirrored, r, pixel, 24u), 0.0);
        }
    }
    textureStore(out_specular, pixel, specular);

    // Through the resin: into it, bent, across to its far side, out again
    // bent back (reflected inside where it meets the far side too steeply to
    // leave), and on to whatever is there.
    var through = vec4<f32>(0.0, 0.0, 0.0, -1.0);
    if (trace.params.y > 0.5) {
        let bent = refract(-v, n, 1.0 / 1.5);
        if (dot(bent, bent) > 0.5) {
            let exit = ray_closest(world - n * lift, bent, reach);
            if (exit.found) {
                var leaving = refract(bent, exit.normal, 1.5);
                if (dot(leaving, leaving) < 0.5) {
                    leaving = reflect(bent, exit.normal);
                }
                let beyond = ray_meet(exit.at - exit.normal * ray_offset(exit.at), leaving, reach, pixel, 14u);
                var seen = backdrop(leaving);
                if (beyond.found) {
                    seen = hit_light(beyond, leaving, pixel, 32u);
                }
                through = vec4<f32>(seen, exit.t);
            }
        }
    }
    textureStore(out_through, pixel, through);
}
