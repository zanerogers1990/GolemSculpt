
// ==== no shadows or ambient occlusion (WebGL2) ====================================
//
// Joined on in place of shade.wgsl when the browser draws with WebGL2
// (shadows.rs): every light reaches everywhere, so the finishes look as
// they always have there.

var<private> shade_shadow: f32 = 1.0;
var<private> shade_ao: f32 = 1.0;
var<private> shade_ao_direct: f32 = 1.0;
var<private> shade_bent: vec3<f32>;

fn shade_begin(pixel: vec2<f32>, world: vec3<f32>, smooth_n: vec3<f32>) {
    shade_bent = smooth_n;
}
