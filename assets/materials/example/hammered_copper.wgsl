// Hammered Copper: an example custom material for GolemSculpt.
//
// A custom material is two files with the same name in the shaders folder
// (Scene panel > Look > Materials says where it is): this .wgsl file and a
// .json sidecar. The file's name is the material's id, and it must define
//
//     fn finish_<id>(s: Surface, params: FinishParams) -> MaterialSample
//
// The app's own shader is compiled first, so everything in it is yours to
// use: the Surface and MaterialSample types, `material(s, albedo)` to start
// a sample, `value_noise`, `fbm`, `hash33`, `glow_cells`, `perturb_normal`,
// `surface_convexity`, `to_world_dir` and the rest.
//
// The rule: no screen-space derivatives (`dpdx`, `dpdy`, `fwidth`, or a
// `textureSample` without a level). A painted material is shaded inside a
// branch chosen pixel by pixel, where those give garbage. To fade detail
// that gets smaller than a pixel, use `s.footprint`, the size of one pixel
// on the model.
//
// Name your helper functions starting with your id, as `hammered_copper_dent`
// below: two materials' helpers with the same name could not be compiled
// together.

// Where a pixel is in the nearest dent: its centre in dent units, and how far
// out from it the pixel is (0 at the bottom, 1 at the rim).
struct HammeredCopperDent {
    centre: vec3<f32>,
    out: f32,
};

fn hammered_copper_dent(p: vec3<f32>) -> HammeredCopperDent {
    let cells = glow_cells(p);
    var d: HammeredCopperDent;
    d.centre = cells.nearest + hash33(cells.nearest);
    d.out = saturate(cells.f1 / 0.65);
    return d;
}

// params (laid out by hammered_copper.json):
//   v[0].rgb  the copper's colour
//   v[1].rgb  the patina's colour
//   v[2].x    dent size (a fraction of the model's size, scaled by the app)
//   v[2].y    dent depth
//   v[2].z    patina amount
//   v[2].w    polish
fn finish_hammered_copper(s: Surface, params: FinishParams) -> MaterialSample {
    let copper = params.v[0].rgb;
    let patina = params.v[1].rgb;
    let dent_size = max(params.v[2].x, 1e-5);
    let dent_depth = params.v[2].y;
    let patina_amount = params.v[2].z;
    let polish = params.v[2].w;

    // Dents live in the object's own space, so they stay put on the model.
    let p = s.p / dent_size;
    let dent = hammered_copper_dent(p);
    // Fade the dents out once they are smaller than a couple of pixels.
    let detail = saturate(1.5 - s.footprint / dent_size * 2.0);

    // Each dent is a shallow dish: its normal leans toward the dent's centre,
    // more toward the rim.
    let offset = to_world_dir(s, p - dent.centre);
    let along_surface = offset - s.n * dot(offset, s.n);
    let n = normalize(s.n - along_surface * dent_depth * 0.6 * dent.out * detail);

    // Green patina settles in the model's recesses (s.curv above 0.5 is a
    // recess; the sidecar asks for "curvature") and a little in the dents.
    let recess = saturate((s.curv - 0.5) * 5.0);
    let blotch = fbm(s.p / (dent_size * 3.0));
    let coat = saturate(patina_amount * (recess * 1.4 + (1.0 - dent.out) * 0.25 * detail) * (0.6 + 0.8 * blotch));

    let tone = mix(0.85, 1.05, value_noise(p * 0.37));
    var m = material(s, mix(copper * tone, patina, coat));
    m.metallic = 1.0 - coat;
    m.smoothness = mix(polish, 0.3, coat);
    m.occlusion = 1.0 - 0.4 * recess;
    m.normal = n;
    return m;
}
