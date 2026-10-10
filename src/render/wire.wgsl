// Wireframe overlay: the edges of each object's faces, as lines over the
// finished picture.

struct Uniforms {
    view_proj: mat4x4<f32>,
    // w = 1 for an orthographic view
    cam_pos: vec4<f32>,
    cam_forward: vec4<f32>,
    color: vec4<f32>,
    // x: how far toward the eye the lines are drawn, as a share of their distance
    params: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

@vertex
fn vs_wire(
    @location(0) position: vec3<f32>,
    @location(1) m0: vec4<f32>,
    @location(2) m1: vec4<f32>,
    @location(3) m2: vec4<f32>,
    @location(4) m3: vec4<f32>,
) -> @builtin(position) vec4<f32> {
    // The last element of the object's matrix carries a flag, not a 1.
    let model = mat4x4<f32>(m0, m1, m2, vec4<f32>(m3.xyz, 1.0));
    let world = (model * vec4<f32>(position, 1.0)).xyz;
    // Lifted off the surface toward the eye, or the lines would fight the
    // triangles they lie on for depth.
    let to_eye = u.cam_pos.xyz - world;
    let dist = length(to_eye);
    var toward = to_eye / max(dist, 1e-6);
    if u.cam_pos.w > 0.5 {
        toward = -u.cam_forward.xyz;
    }
    return u.view_proj * vec4<f32>(world + toward * dist * u.params.x, 1.0);
}

@fragment
fn fs_wire() -> @location(0) vec4<f32> {
    return u.color;
}
