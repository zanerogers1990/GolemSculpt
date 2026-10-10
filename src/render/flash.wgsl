// The selection flash: an object's own triangles drawn once more, in one flat
// translucent colour, over the finished picture. Only positions are read from
// the mesh; the object's matrix and the colour come per instance.

struct Uniforms {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct In {
    @location(0) position: vec3<f32>,
    // The object's matrix by columns, as in the main pass.
    @location(1) model_x: vec4<f32>,
    @location(2) model_y: vec4<f32>,
    @location(3) model_z: vec4<f32>,
    @location(4) model_w: vec4<f32>,
    // Display colour (what the screen should show at full strength), with the
    // strength of the flash this instant as alpha.
    @location(5) color: vec4<f32>,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) color: vec4<f32>,
};

@vertex
fn vs_flash(in: In) -> Out {
    var out: Out;
    // The same arithmetic as vs_mesh, so the depth it produces matches the
    // scene's depth buffer to the last bit and the depth test below sees the
    // object's own surface as "at" the depth, not behind it.
    let world = in.model_x.xyz * in.position.x + in.model_y.xyz * in.position.y
        + in.model_z.xyz * in.position.z + in.model_w.xyz;
    out.pos = u.view_proj * vec4<f32>(world, 1.0);
    out.color = in.color;
    return out;
}

@fragment
fn fs_flash(in: Out) -> @location(0) vec4<f32> {
    // The target holds finished, already encoded colour, so this goes in as is.
    return in.color;
}
