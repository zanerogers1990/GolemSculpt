// Face group colours: each object's own triangles drawn once more over the
// finished picture, every triangle in the flat colour of its group, blended
// so that the picture's shading shows through.
//
// A group is a property of a triangle, and a vertex on the border of two
// groups belongs to both, so the colour cannot ride on the vertices the main
// pass draws. Here every corner is looked up instead: the draw has no vertex
// buffer of its own, and reads the mesh's index and position buffers, and one
// colour per triangle, by the number of the corner being drawn.

struct Uniforms {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// The mesh's own buffers, as the main pass has them: three floats a vertex,
// and three corners a triangle that shows.
@group(1) @binding(0) var<storage, read> positions: array<f32>;
@group(1) @binding(1) var<storage, read> corners: array<u32>;
// One colour per triangle that shows: red, green, blue and strength, a byte each.
@group(1) @binding(2) var<storage, read> colours: array<u32>;

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) @interpolate(flat) colour: vec4<f32>,
};

@vertex
fn vs_groups(
    @builtin(vertex_index) corner: u32,
    // The object's matrix by columns, as in the main pass.
    @location(1) model_x: vec4<f32>,
    @location(2) model_y: vec4<f32>,
    @location(3) model_z: vec4<f32>,
    @location(4) model_w: vec4<f32>,
) -> Out {
    var out: Out;
    let v = corners[corner] * 3u;
    let position = vec3<f32>(positions[v], positions[v + 1u], positions[v + 2u]);
    // The same arithmetic as vs_mesh, so the depth it produces matches the
    // scene's depth buffer and the depth test sees the object's own surface
    // as "at" the depth, not behind it.
    let world = model_x.xyz * position.x + model_y.xyz * position.y + model_z.xyz * position.z + model_w.xyz;
    out.pos = u.view_proj * vec4<f32>(world, 1.0);
    out.colour = unpack4x8unorm(colours[corner / 3u]);
    return out;
}

@fragment
fn fs_groups(in: Out) -> @location(0) vec4<f32> {
    // The blend makes of this `colour * (strength + picture)`: the picture
    // tinted, and lifted a little so the colour still reads on a dark
    // material. A triangle in no group is white at no strength, which leaves
    // the picture as it is.
    return in.colour;
}
