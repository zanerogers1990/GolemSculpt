// The surroundings the model reflects: a small cube map drawn from the
// default sky, the background gradient or the loaded HDRI, then blurred into
// its smaller levels so a rougher surface can read a blurrier reflection.

struct EnvU {
    // x source (0 sky, 1 gradient, 2 HDRI), y HDRI turn in radians,
    // z HDRI exposure, w gradient bias
    params: vec4<f32>,
    bottom: vec4<f32>,
    top: vec4<f32>,
    // Toward the sun, and its colour.
    sun_dir: vec4<f32>,
    sun_color: vec4<f32>,
};

@group(0) @binding(0) var<uniform> env: EnvU;
@group(0) @binding(1) var hdri_tex: texture_2d<f32>;
@group(0) @binding(2) var hdri_smp: sampler;
@group(0) @binding(3) var src_cube: texture_cube<f32>;
@group(0) @binding(4) var cube_smp: sampler;

const PI: f32 = 3.14159265;
const TAU: f32 = 6.2831853;
// Side of the cube's largest level, and how many levels are blurred.
const CUBE_SIZE: f32 = 128.0;

struct EnvOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Which face is being drawn, and which level of the cube.
    @location(1) @interpolate(flat) face: u32,
    @location(2) @interpolate(flat) level: u32,
};

// One triangle over the whole target. The instance number carries the face
// (0-5) and the level (times 6).
@vertex
fn vs_face(@builtin(vertex_index) i: u32, @builtin(instance_index) instance: u32) -> EnvOut {
    var out: EnvOut;
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    out.pos = vec4<f32>(p, 0.0, 1.0);
    out.uv = vec2<f32>(p.x, -p.y);
    out.face = instance % 6u;
    out.level = instance / 6u;
    return out;
}

// The direction a point on a cube face looks along (u right, v down).
fn face_dir(face: u32, uv: vec2<f32>) -> vec3<f32> {
    var d: vec3<f32>;
    switch face {
        case 0u: { d = vec3<f32>(1.0, -uv.y, -uv.x); }
        case 1u: { d = vec3<f32>(-1.0, -uv.y, uv.x); }
        case 2u: { d = vec3<f32>(uv.x, 1.0, uv.y); }
        case 3u: { d = vec3<f32>(uv.x, -1.0, -uv.y); }
        case 4u: { d = vec3<f32>(uv.x, -uv.y, 1.0); }
        default: { d = vec3<f32>(-uv.x, -uv.y, -1.0); }
    }
    return normalize(d);
}

// ---- the default sky ------------------------------------------------------
//
// The atmosphere of the Unity app's default skybox (after Sean O'Neil's
// scattering), which is what its models reflect when no HDRI is loaded: blue
// overhead, pale at the horizon, grey-brown ground, and a small hot sun.

const OUTER_RADIUS: f32 = 1.025;
const INNER_RADIUS: f32 = 1.0;
const CAMERA_HEIGHT: f32 = 0.0001;
const MIE: f32 = 0.0010;
const SUN_BRIGHTNESS: f32 = 20.0;
const MAX_SCATTER: f32 = 50.0;
const SCALE_DEPTH: f32 = 0.25;
const MIE_G: f32 = -0.990;
const MIE_G2: f32 = 0.9801;
const SKY_GROUND_THRESHOLD: f32 = 0.02;
const SKY_EXPOSURE: f32 = 1.3;
const SUN_SIZE: f32 = 0.04;
const SUN_CONVERGENCE: f32 = 5.0;

fn scatter_scale(in_cos: f32) -> f32 {
    let x = 1.0 - in_cos;
    return 0.25 * exp(-0.00287 + x * (0.459 + x * (3.83 + x * (-6.80 + x * 5.25))));
}

fn mie_phase(eye_cos: f32, eye_cos2: f32) -> f32 {
    var temp = 1.0 + MIE_G2 - 2.0 * MIE_G * eye_cos;
    temp = pow(temp, pow(SUN_SIZE, 0.65) * 10.0);
    temp = max(temp, 1.0e-4);
    return 1.5 * ((1.0 - MIE_G2) / (2.0 + MIE_G2)) * (1.0 + eye_cos2) / temp;
}

fn default_sky(eye_ray: vec3<f32>) -> vec3<f32> {
    let sun = env.sun_dir.xyz;
    // Sky tint (0.5 grey) and atmosphere thickness 1.
    let wavelength = vec3<f32>(0.65, 0.57, 0.475);
    let inv_wavelength = 1.0 / pow(wavelength, vec3<f32>(4.0));
    let rayleigh = 0.0025;
    let kr_e_sun = rayleigh * SUN_BRIGHTNESS;
    let kr_4pi = rayleigh * 4.0 * PI;
    let km_e_sun = MIE * SUN_BRIGHTNESS;
    let km_4pi = MIE * 4.0 * PI;
    let scale = 1.0 / (OUTER_RADIUS - 1.0);
    let scale_over_depth = scale / SCALE_DEPTH;
    let camera_pos = vec3<f32>(0.0, INNER_RADIUS + CAMERA_HEIGHT, 0.0);
    let ground_color = vec3<f32>(0.1119, 0.0999, 0.0953);

    var c_in: vec3<f32>;
    var c_out: vec3<f32>;
    if (eye_ray.y >= 0.0) {
        let far = sqrt(OUTER_RADIUS * OUTER_RADIUS + INNER_RADIUS * INNER_RADIUS * eye_ray.y * eye_ray.y
            - INNER_RADIUS * INNER_RADIUS) - INNER_RADIUS * eye_ray.y;
        let height = INNER_RADIUS + CAMERA_HEIGHT;
        let depth = exp(scale_over_depth * (-CAMERA_HEIGHT));
        let start_angle = dot(eye_ray, camera_pos) / height;
        let start_offset = depth * scatter_scale(start_angle);
        let sample_length = far / 2.0;
        let scaled_length = sample_length * scale;
        let sample_ray = eye_ray * sample_length;
        var sample_point = camera_pos + sample_ray * 0.5;
        var front = vec3<f32>(0.0);
        for (var i = 0; i < 2; i++) {
            let h = length(sample_point);
            let d = exp(scale_over_depth * (INNER_RADIUS - h));
            let light_angle = dot(sun, sample_point) / h;
            let camera_angle = dot(eye_ray, sample_point) / h;
            let scatter = start_offset + d * (scatter_scale(light_angle) - scatter_scale(camera_angle));
            let attenuate = exp(-clamp(scatter, 0.0, MAX_SCATTER) * (inv_wavelength * kr_4pi + km_4pi));
            front += attenuate * (d * scaled_length);
            sample_point += sample_ray;
        }
        c_in = front * (inv_wavelength * kr_e_sun);
        c_out = front * km_e_sun;
    } else {
        let far = (-CAMERA_HEIGHT) / min(-0.001, eye_ray.y);
        let pos = camera_pos + far * eye_ray;
        let depth = exp((-CAMERA_HEIGHT) * (1.0 / SCALE_DEPTH));
        let camera_angle = dot(-eye_ray, pos);
        let light_angle = dot(sun, pos);
        let camera_scale = scatter_scale(camera_angle);
        let light_scale = scatter_scale(light_angle);
        let camera_offset = depth * camera_scale;
        let temp = light_scale + camera_scale;
        let sample_length = far / 2.0;
        let scaled_length = sample_length * scale;
        let sample_ray = eye_ray * sample_length;
        var sample_point = camera_pos + sample_ray * 0.5;
        var front = vec3<f32>(0.0);
        var attenuate = vec3<f32>(0.0);
        for (var i = 0; i < 2; i++) {
            let h = length(sample_point);
            let d = exp(scale_over_depth * (INNER_RADIUS - h));
            let scatter = d * temp - camera_offset;
            attenuate = exp(-clamp(scatter, 0.0, MAX_SCATTER) * (inv_wavelength * kr_4pi + km_4pi));
            front += attenuate * (d * scaled_length);
            sample_point += sample_ray;
        }
        c_in = front * (inv_wavelength * kr_e_sun + km_e_sun);
        c_out = clamp(attenuate, vec3<f32>(0.0), vec3<f32>(1.0));
    }

    let eye_cos = dot(sun, eye_ray);
    let ground = SKY_EXPOSURE * (c_in + ground_color * c_out);
    let sky = SKY_EXPOSURE * (c_in * (0.75 + 0.75 * eye_cos * eye_cos));
    let y = -eye_ray.y / SKY_GROUND_THRESHOLD;
    var color = mix(sky, ground, saturate(y));
    if (y < 0.0) {
        let light = env.sun_color.rgb;
        let sun_color = 15.0 * saturate(c_out) * light / clamp(length(light), 0.25, 1.0);
        let focused = pow(saturate(eye_cos), SUN_CONVERGENCE);
        color += sun_color * mie_phase(-focused, focused * focused);
    }
    return color;
}

fn hdri_uv(dir: vec3<f32>, turn: f32) -> vec2<f32> {
    let v = dir;
    let s = sin(turn);
    let c = cos(turn);
    let d = vec3<f32>(c * v.x - s * v.z, v.y, s * v.x + c * v.z);
    return vec2<f32>(atan2(d.z, d.x) / TAU + 0.5, 0.5 - asin(clamp(d.y, -1.0, 1.0)) / PI);
}

// The largest level: the source itself.
@fragment
fn fs_source(in: EnvOut) -> @location(0) vec4<f32> {
    let dir = face_dir(in.face, in.uv);
    var color: vec3<f32>;
    if (env.params.x > 1.5) {
        // Read a level of the picture about as fine as this cube's texels.
        let width = f32(textureDimensions(hdri_tex).x);
        let lod = max(log2(width / (4.0 * CUBE_SIZE)), 0.0);
        color = textureSampleLevel(hdri_tex, hdri_smp, hdri_uv(dir, env.params.y), lod).rgb * env.params.z;
    } else if (env.params.x > 0.5) {
        let t = pow(saturate(dir.y * 0.5 + 0.5), 1.0 / max(env.params.w, 0.001));
        color = mix(env.bottom.rgb, env.top.rgb, t);
    } else {
        color = default_sky(dir);
    }
    return vec4<f32>(min(color, vec3<f32>(60000.0)), 1.0);
}

// Each smaller level of the source: the level above, averaged.
@fragment
fn fs_shrink(in: EnvOut) -> @location(0) vec4<f32> {
    return textureSampleLevel(src_cube, cube_smp, face_dir(in.face, in.uv), 0.0);
}

fn radical_inverse(bits_in: u32) -> f32 {
    var bits = bits_in;
    bits = (bits << 16u) | (bits >> 16u);
    bits = ((bits & 0x55555555u) << 1u) | ((bits & 0xAAAAAAAAu) >> 1u);
    bits = ((bits & 0x33333333u) << 2u) | ((bits & 0xCCCCCCCCu) >> 2u);
    bits = ((bits & 0x0F0F0F0Fu) << 4u) | ((bits & 0xF0F0F0F0u) >> 4u);
    bits = ((bits & 0x00FF00FFu) << 8u) | ((bits & 0xFF00FF00u) >> 8u);
    return f32(bits) * 2.3283064365386963e-10;
}

// The reflection a surface of this level's roughness sees: the source
// averaged over the highlight's lobe.
@fragment
fn fs_blur(in: EnvOut) -> @location(0) vec4<f32> {
    let n = face_dir(in.face, in.uv);
    if (in.level == 0u) {
        return textureSampleLevel(src_cube, cube_smp, n, 0.0);
    }
    // The roughness whose reflections are read from this level (the inverse
    // of the level a roughness looks up).
    let m = f32(in.level) / 6.0;
    let rough = (1.7 - sqrt(2.89 - 2.8 * m)) / 1.4;
    let a = rough * rough;
    let a2 = a * a;
    let up = select(vec3<f32>(1.0, 0.0, 0.0), vec3<f32>(0.0, 0.0, 1.0), abs(n.z) < 0.999);
    let tx = normalize(cross(up, n));
    let ty = cross(n, tx);
    let texel_angle = 4.0 * PI / (6.0 * CUBE_SIZE * CUBE_SIZE);
    const SAMPLES: u32 = 256u;
    var color = vec3<f32>(0.0);
    var total = 0.0;
    for (var i = 0u; i < SAMPLES; i++) {
        let xi = vec2<f32>(f32(i) / f32(SAMPLES), radical_inverse(i));
        let phi = TAU * xi.x;
        let cos_theta = sqrt((1.0 - xi.y) / (1.0 + (a2 - 1.0) * xi.y));
        let sin_theta = sqrt(1.0 - cos_theta * cos_theta);
        let h = tx * (sin_theta * cos(phi)) + ty * (sin_theta * sin(phi)) + n * cos_theta;
        let l = 2.0 * dot(n, h) * h - n;
        let nl = dot(n, l);
        if (nl > 0.0) {
            // Samples the lobe rarely visits stand for a wide patch of sky:
            // read a blurrier level of the source for them, so a small
            // bright sun does not turn into sparkle.
            let nh = cos_theta;
            let dd = nh * nh * (a2 - 1.0) + 1.0;
            let pdf = a2 / (PI * dd * dd) * 0.25;
            let sample_angle = 1.0 / (f32(SAMPLES) * pdf + 1e-4);
            let lod = max(0.5 * log2(sample_angle / texel_angle) + 1.0, 0.0);
            color += textureSampleLevel(src_cube, cube_smp, l, lod).rgb * nl;
            total += nl;
        }
    }
    return vec4<f32>(color / max(total, 1e-4), 1.0);
}
