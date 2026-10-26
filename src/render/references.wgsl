// Reference pictures (src/references.rs): flat pictures standing in the
// scene, each drawn as two triangles with its image on them. Drawn in the
// overlay pass, after the scene is finished and graded, so what reaches the
// screen is the image's own colours: the 8-bit target takes sRGB values,
// and the image's bytes are sRGB already, so they go out as they are.

struct Uniforms {
    view_proj: mat4x4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// Premultiplied by its own alpha when it was sent (render/references.rs).
@group(1) @binding(0) var picture: texture_2d<f32>;
@group(1) @binding(1) var filtered: sampler;

struct In {
    @location(0) position: vec3<f32>,
    // Where in the image this corner is: 0 to 1 from its top left.
    @location(1) uv: vec2<f32>,
    // The picture's Opacity, 0 to 1.
    @location(2) opacity: f32,
};

struct Out {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) opacity: f32,
};

@vertex
fn vs_reference(in: In) -> Out {
    var out: Out;
    out.pos = u.view_proj * vec4<f32>(in.position, 1.0);
    out.uv = in.uv;
    out.opacity = in.opacity;
    return out;
}

@fragment
fn fs_reference(in: Out) -> @location(0) vec4<f32> {
    // Premultiplied, so fading the whole of it is one multiply, and blended
    // as such: a cut-out's soft edge does not darken as it is filtered.
    return textureSample(picture, filtered, in.uv) * in.opacity;
}
