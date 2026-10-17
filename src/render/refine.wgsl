// Refining a still picture: each sample of the scene, drawn with the camera
// moved a fraction of a pixel and every noisy filter turned a new way, is
// folded into a running average kept at full float precision. The post
// effects then read the average as if it were the scene (see refine.rs).

struct RefineU {
    // x the new sample's share of the average: 1 / (samples so far + 1)
    weight: vec4<f32>,
};

@group(0) @binding(0) var<uniform> refine: RefineU;
// The average so far (or, for the seed, the scene the picture started as).
@group(0) @binding(1) var average: texture_2d<f32>;
// The new sample.
@group(0) @binding(2) var sample: texture_2d<f32>;

// One triangle over the whole target.
@vertex
fn vs_refine(@builtin(vertex_index) i: u32) -> @builtin(position) vec4<f32> {
    let p = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u)) * 2.0 - 1.0;
    return vec4<f32>(p, 0.0, 1.0);
}

// The picture the refining starts from, as the first sample of the average.
@fragment
fn fs_seed(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    return textureLoad(average, vec2<i32>(pos.xy), 0);
}

struct Folded {
    // The average with the new sample in it, kept for the next one...
    @location(0) average: vec4<f32>,
    // ...and the same for the post effects to read in place of the scene.
    @location(1) scene: vec4<f32>,
};

@fragment
fn fs_accumulate(@builtin(position) pos: vec4<f32>) -> Folded {
    let at = vec2<i32>(pos.xy);
    let before = textureLoad(average, at, 0);
    let fresh = textureLoad(sample, at, 0);
    let after = before + (fresh - before) * refine.weight.x;
    var out: Folded;
    out.average = after;
    out.scene = after;
    return out;
}
