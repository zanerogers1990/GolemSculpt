use std::cell::Cell;
use std::path::{Path, PathBuf};
use std::time::Instant;

use glam::{Mat4, Quat, Vec2, Vec3};

use super::*;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

/// A low model's arrays, owned, to hand to `bake` as a `Target`.
struct LowModel {
    positions: Vec<Vec3>,
    normals: Vec<Vec3>,
    indices: Vec<u32>,
    uvs: Vec<Vec2>,
    to_world: Mat4,
}

impl LowModel {
    fn target(&self) -> Target<'_> {
        Target {
            positions: &self.positions,
            normals: &self.normals,
            indices: &self.indices,
            uvs: &self.uvs,
            to_world: self.to_world,
        }
    }
}

/// The square from -1 to 1 in X and Y, facing +Z, in two triangles, each
/// corner's UV given by `uv` from its position.
fn plane_with(uv: impl Fn(Vec3) -> Vec2) -> LowModel {
    let positions = vec![
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
    ];
    let indices = vec![0, 1, 2, 0, 2, 3];
    let uvs = indices.iter().map(|&i| uv(positions[i as usize])).collect();
    LowModel { normals: vec![Vec3::Z; 4], positions, indices, uvs, to_world: Mat4::IDENTITY }
}

/// The square with its UVs across the whole picture.
fn low_plane() -> LowModel {
    plane_with(|p| Vec2::new((p.x + 1.0) / 2.0, (p.y + 1.0) / 2.0))
}

/// A sheet over `x` and `y` (each from, to) in `n` quads a side, standing at
/// `height(x, y)`, facing +Z.
fn sheet_over(n: usize, x: [f32; 2], y: [f32; 2], height: impl Fn(f32, f32) -> f32) -> Mesh {
    let mut positions = Vec::with_capacity((n + 1) * (n + 1));
    for j in 0..=n {
        for i in 0..=n {
            let px = x[0] + (x[1] - x[0]) * i as f32 / n as f32;
            let py = y[0] + (y[1] - y[0]) * j as f32 / n as f32;
            positions.push(Vec3::new(px, py, height(px, py)));
        }
    }
    let mut indices = Vec::with_capacity(n * n * 6);
    for j in 0..n {
        for i in 0..n {
            let a = (j * (n + 1) + i) as u32;
            let (b, c, d) = (a + 1, a + n as u32 + 2, a + n as u32 + 1);
            indices.extend([a, b, c, a, c, d]);
        }
    }
    Mesh::new(positions, indices, None)
}

/// A sheet over the same square as the low plane.
fn sheet(n: usize, height: impl Fn(f32, f32) -> f32) -> Mesh {
    sheet_over(n, [-1.0, 1.0], [-1.0, 1.0], height)
}

const BUMP_HEIGHT: f32 = 0.1;
const BUMP_WIDTH: f32 = 0.25;

/// A smooth round bump at `centre`.
fn bump_at(centre: Vec2, x: f32, y: f32) -> f32 {
    let r2 = (Vec2::new(x, y) - centre).length_squared();
    BUMP_HEIGHT * (-r2 / (BUMP_WIDTH * BUMP_WIDTH)).exp()
}

fn bump(x: f32, y: f32) -> f32 {
    bump_at(Vec2::ZERO, x, y)
}

/// The true normal of a sheet standing at `height`, at `x, y`.
fn normal_of(height: impl Fn(f32, f32) -> f32, x: f32, y: f32) -> Vec3 {
    let e = 1e-3;
    let dx = (height(x + e, y) - height(x - e, y)) / (2.0 * e);
    let dy = (height(x, y + e) - height(x, y - e)) / (2.0 * e);
    Vec3::new(-dx, -dy, 1.0).normalize()
}

/// A globe of radius 1 laid out as a map of the world: U once round, V from
/// the south pole (0) to the north (1), `rings` bands and `segments` around,
/// with a seam where U comes round from 1 to 0.
fn globe(rings: usize, segments: usize) -> LowModel {
    let mut positions = Vec::new();
    let mut grid_uv = Vec::new();
    for i in 0..=rings {
        let theta = std::f32::consts::PI * i as f32 / rings as f32;
        for j in 0..=segments {
            let phi = std::f32::consts::TAU * j as f32 / segments as f32;
            positions.push(Vec3::new(theta.sin() * phi.cos(), theta.sin() * phi.sin(), -theta.cos()));
            grid_uv.push(Vec2::new(j as f32 / segments as f32, i as f32 / rings as f32));
        }
    }
    let mut indices = Vec::new();
    for i in 0..rings {
        for j in 0..segments {
            let a = (i * (segments + 1) + j) as u32;
            let (b, c, d) = (a + 1, a + segments as u32 + 2, a + segments as u32 + 1);
            // The triangles that would have both corners on a pole are left out.
            if i != 0 {
                indices.extend([a, b, c]);
            }
            if i != rings - 1 {
                indices.extend([a, c, d]);
            }
        }
    }
    let uvs = indices.iter().map(|&i| grid_uv[i as usize]).collect();
    LowModel { normals: positions.clone(), positions, indices, uvs, to_world: Mat4::IDENTITY }
}

const NONE: Maps = Maps { normal: false, displacement: false, ao: false, colour: false };
const ALL: Maps = Maps { normal: true, displacement: true, ao: true, colour: true };

fn settings(maps: Maps) -> Settings {
    Settings { size: 512, reach: 0.2, maps, ao_rays: 16, padding: 8 }
}

fn plain(mesh: &Mesh) -> Source<'_> {
    Source { mesh, to_world: Mat4::IDENTITY, colours: None }
}

fn go(low: &LowModel, sources: &[Source], settings: &Settings) -> Baked {
    bake(&low.target(), sources, settings, &|_| true).expect("bakes")
}

/// The texel over the low plane's point `x, y` (UVs across the whole picture).
fn texel(x: f32, y: f32, size: u32) -> (u32, u32) {
    let (u, v) = ((x + 1.0) / 2.0, (y + 1.0) / 2.0);
    let at = |s: f32| ((s * size as f32) as u32).clamp(0, size - 1);
    (at(u), at(1.0 - v))
}

/// A folder for what the tests write, inside the build folder the tests run
/// from (`<target>/bake-tests`), never anyone's own folders.
fn out_dir(name: &str) -> PathBuf {
    let exe = std::env::current_exe().expect("the test program's path");
    // <target>/<profile>/deps/<program>
    let target = exe.parent().and_then(Path::parent).and_then(Path::parent).expect("inside a build folder");
    target.join("bake-tests").join(name)
}

/// Saves a picture for a person to look at, when SCULPT_BAKE_PICTURES is set.
fn keep(name: &str, save: impl FnOnce(&Path) -> image::ImageResult<()>) {
    if std::env::var_os("SCULPT_BAKE_PICTURES").is_some() {
        let folder = out_dir("pictures");
        std::fs::create_dir_all(&folder).expect("a folder for pictures");
        save(&folder.join(format!("{name}.png"))).expect("saves");
    }
}

/// Reads every covered texel of `normal` back into the scene through the low
/// model's tangent frame, as an engine's shader does, and checks it against
/// `want` (the sculpt's true normal under the low point). The largest and
/// the mean angle between them, in radians.
fn read_back(low: &LowModel, normal: &image::RgbImage, want: impl Fn(Vec3) -> Vec3) -> (f32, f32) {
    let size = normal.width();
    let model = Low::new(&low.target());
    let raster = Raster::new(&model.uvs, size);
    let (mut worst, mut sum, mut count) = (0.0f32, 0.0f32, 0usize);
    for band in 0..raster.band_count() {
        raster.band(
            band,
            |tri, x, y, w| {
                let c = tri as usize * 3;
                let corners = |v: &[Vec3]| [v[c], v[c + 1], v[c + 2]];
                let point = model.positions[c] * w[0] + model.positions[c + 1] * w[1] + model.positions[c + 2] * w[2];
                let tangents = [model.tangents[c], model.tangents[c + 1], model.tangents[c + 2]];
                let frame = Frame::at(corners(&model.normals), tangents, w);
                let got = frame.to_world(frame::decode(normal.get_pixel(x, y).0));
                let angle = got.dot(want(point)).clamp(-1.0, 1.0).acos();
                worst = worst.max(angle);
                sum += angle;
                count += 1;
            },
            || false,
        );
    }
    (worst, sum / count as f32)
}

#[test]
fn a_bump_tilts_the_normal_map_the_way_its_sides_lean() {
    let low = low_plane();
    let high = sheet(256, bump);
    let baked = go(&low, &[plain(&high)], &settings(Maps { normal: true, ..NONE }));
    assert_eq!(baked.missed, 0.0);
    let normal = baked.normal.expect("a normal map");
    keep("bump_normal", |p| normal.save(p));
    let at = |x: f32, y: f32| {
        let (tx, ty) = texel(x, y, 512);
        normal.get_pixel(tx, ty).0
    };
    for (x, y) in [(-0.8, -0.8), (0.8, 0.7), (-0.7, 0.75)] {
        let t = at(x, y);
        assert!(t.iter().zip(FLAT).all(|(a, b)| a.abs_diff(b) <= 1), "flat away from the bump: {t:?}");
    }
    let (left, right, below, above) = (at(-0.18, 0.0), at(0.18, 0.0), at(0.0, -0.18), at(0.0, 0.18));
    // The bump's left side leans its normal towards -X: less red.
    assert!(left[0] < 98 && right[0] > 158, "red: left {left:?}, right {right:?}");
    // Its lower side (less V) leans towards -Y: less green, OpenGL's way.
    assert!(below[1] < 98 && above[1] > 158, "green: below {below:?}, above {above:?}");
    // And neither leans the other way.
    assert!(left[1].abs_diff(128) <= 2 && below[0].abs_diff(128) <= 2);
}

#[test]
fn the_normal_map_reads_back_to_the_sculpts_normals_through_the_engines_frame() {
    let low = low_plane();
    let high = sheet(256, bump);
    let baked = go(&low, &[plain(&high)], &settings(Maps { normal: true, ..NONE }));
    let (worst, mean) = read_back(&low, &baked.normal.unwrap(), |p| normal_of(bump, p.x, p.y));
    assert!(worst < 0.04 && mean < 0.01, "worst {worst}, mean {mean}");
}

#[test]
fn displacement_at_the_top_of_a_bump_is_its_height() {
    let low = low_plane();
    let high = sheet(256, bump);
    let baked = go(&low, &[plain(&high)], &settings(Maps { displacement: true, ..NONE }));
    let (height, lo, hi) = baked.displacement.expect("a displacement map");
    let at = |x: f32, y: f32| {
        let (tx, ty) = texel(x, y, 512);
        height.get_pixel(tx, ty).0[0]
    };
    assert!((at(0.0, 0.0) - BUMP_HEIGHT).abs() < 0.002, "the top: {}", at(0.0, 0.0));
    assert!(at(0.8, -0.8).abs() < 1e-4, "away from it: {}", at(0.8, -0.8));
    let side = bump(0.2, 0.0);
    assert!((at(0.2, 0.0) - side).abs() < 0.003, "on its side: {} against {side}", at(0.2, 0.0));
    assert!(lo.abs() < 1e-4 && (hi - BUMP_HEIGHT).abs() < 0.002, "range {lo} to {hi}");
}

#[test]
fn a_wall_beside_a_floor_darkens_the_floor_near_it() {
    let low = low_plane();
    let floor = sheet(40, |_, _| 0.0);
    // A second sculpt, stretched into a wall standing on the floor at x 0.35 to 0.45.
    let block = primitives::build(Primitive::Cube, 2_000, 0.5);
    let place = Mat4::from_translation(Vec3::new(0.4, 0.0, 0.5)) * Mat4::from_scale(Vec3::new(0.1, 2.0, 1.0));
    let wall = Source { mesh: &block, to_world: place, colours: None };
    let baked = go(&low, &[plain(&floor), wall], &Settings { ao_rays: 64, ..settings(Maps { ao: true, ..NONE }) });
    let ao = baked.ao.expect("an occlusion map");
    keep("wall_ao", |p| ao.save(p));
    // A few texels together, to see through the grain.
    let around = |x: f32, y: f32| {
        let (tx, ty) = texel(x, y, 512);
        let mut sum = 0u32;
        for dy in 0..5 {
            for dx in 0..5 {
                sum += ao.get_pixel(tx + dx - 2, ty + dy - 2).0[0] as u32;
            }
        }
        sum as f32 / 25.0
    };
    let (far, near, nearer) = (around(-0.8, 0.0), around(0.2, 0.0), around(0.3, 0.0));
    assert!(far >= 254.0, "open floor: {far}");
    assert!(near < far - 40.0 && nearer < near - 10.0, "far {far}, near {near}, nearer {nearer}");
}

#[test]
fn the_colour_map_carries_the_paint() {
    let low = low_plane();
    let high = sheet(64, |_, _| 0.0);
    let (red, blue) = ([255, 0, 0, 255], [0, 0, 255, 255]);
    let paint: Vec<[u8; 4]> = high.positions.iter().map(|p| if p.x < -1e-4 { red } else { blue }).collect();
    let painted = Source { mesh: &high, to_world: Mat4::IDENTITY, colours: Some(&paint) };
    let baked = go(&low, &[painted], &settings(Maps { colour: true, ..NONE }));
    let colour = baked.colour.expect("a colour map");
    keep("paint_colour", |p| colour.save(p));
    let at = |x: f32, y: f32| {
        let (tx, ty) = texel(x, y, 512);
        colour.get_pixel(tx, ty).0
    };
    assert_eq!(at(-0.5, 0.3), red);
    assert_eq!(at(0.5, -0.3), blue);
}

#[test]
fn an_unpainted_sculpt_bakes_no_colour_map() {
    let low = low_plane();
    let high = sheet(8, |_, _| 0.0);
    let baked = go(&low, &[plain(&high)], &settings(Maps { normal: true, colour: true, ..NONE }));
    assert!(baked.colour.is_none() && baked.normal.is_some());
    let only = bake(&low.target(), &[plain(&high)], &settings(Maps { colour: true, ..NONE }), &|_| true);
    assert!(only.err().is_some_and(|e| e.contains("paint")));
}

#[test]
fn a_slightly_larger_sphere_bakes_near_flat_normals_and_even_displacement() {
    let low = globe(24, 48);
    let high = primitives::build(Primitive::Sphere, 100_000, 1.02);
    let baked = go(
        &low,
        &[plain(&high)],
        &Settings { reach: 0.1, ..settings(Maps { normal: true, displacement: true, ..NONE }) },
    );
    assert_eq!(baked.missed, 0.0);
    let normal = baked.normal.unwrap();
    keep("sphere_normal", |p| normal.save(p));
    let (worst, mean) = read_back(&low, &normal, |p| p.normalize());
    assert!(worst < 0.03 && mean < 0.01, "worst {worst}, mean {mean}");
    let near_flat = normal.pixels().filter(|p| p.0.iter().zip(FLAT).all(|(a, b)| a.abs_diff(b) <= 3)).count();
    assert!(near_flat as f32 > 0.99 * (512.0 * 512.0), "{near_flat} near flat");
    // The low facets sit inside the true sphere by at most a few thousandths.
    let (height, lo, hi) = baked.displacement.unwrap();
    assert!(lo > 0.018 && hi < 0.026, "{lo} to {hi}");
    assert!(height.pixels().all(|p| (0.018..0.026).contains(&p.0[0])));
}

#[test]
fn padding_spreads_an_island_outward_and_no_further() {
    // The island fills texels 128 to 383 each way; padding is 8.
    let low = plane_with(|p| Vec2::new(0.25 + (p.x + 1.0) / 4.0, 0.25 + (p.y + 1.0) / 4.0));
    let high = sheet(32, |_, _| 0.05);
    let paint = vec![[200, 30, 10, 255]; high.vertex_count()];
    let painted = Source { mesh: &high, to_world: Mat4::IDENTITY, colours: Some(&paint) };
    let baked = go(&low, &[painted], &settings(Maps { colour: true, displacement: true, ..NONE }));
    let colour = baked.colour.unwrap();
    let (height, ..) = baked.displacement.unwrap();
    keep("padding_colour", |p| colour.save(p));
    let filled = |x: u32, y: u32| {
        colour.get_pixel(x, y).0 == [200, 30, 10, 255] && (height.get_pixel(x, y).0[0] - 0.05).abs() < 1e-5
    };
    let empty = |x: u32, y: u32| colour.get_pixel(x, y).0 == [0, 0, 0, 0] && height.get_pixel(x, y).0[0] == 0.0;
    assert!(filled(128, 256) && filled(383, 256), "the island's own edge");
    assert!(filled(124, 256) && filled(120, 256) && filled(391, 256), "within the padding");
    assert!(filled(121, 121) && filled(390, 390), "round the corners too");
    assert!(empty(119, 256) && empty(392, 256) && empty(256, 119) && empty(118, 118), "and no further");
}

#[test]
fn a_mirrored_uv_island_still_bakes_the_right_normals() {
    // The left half laid out as usual; the right half mirrored, U running
    // towards -X, the two halves meeting along the seam at U = 0.5.
    let positions = vec![
        Vec3::new(-1.0, -1.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(0.0, 1.0, 0.0),
        Vec3::new(-1.0, 1.0, 0.0),
        Vec3::new(1.0, -1.0, 0.0),
        Vec3::new(1.0, 1.0, 0.0),
    ];
    let indices = vec![0, 1, 2, 0, 2, 3, 1, 4, 5, 1, 5, 2];
    let uv =
        |p: Vec3, right: bool| Vec2::new(if right { 1.0 - p.x / 2.0 } else { (p.x + 1.0) / 2.0 }, (p.y + 1.0) / 2.0);
    let uvs = indices.iter().enumerate().map(|(k, &i): (usize, &u32)| uv(positions[i as usize], k >= 6)).collect();
    let low = LowModel { normals: vec![Vec3::Z; 6], positions, indices, uvs, to_world: Mat4::IDENTITY };
    let height = |x: f32, y: f32| bump_at(Vec2::new(-0.5, 0.0), x, y) + bump_at(Vec2::new(0.5, 0.0), x, y);
    let high = sheet(256, height);
    let baked = go(&low, &[plain(&high)], &settings(Maps { normal: true, ..NONE }));
    let normal = baked.normal.unwrap();
    keep("mirrored_normal", |p| normal.save(p));
    let at = |u: f32, v: f32| normal.get_pixel((u * 512.0) as u32, ((1.0 - v) * 512.0) as u32).0;
    // Left island: the left bump's left side (x -0.7) leans -X = -U: less red;
    // its lower side leans -Y = -V: less green.
    assert!(at(0.15, 0.5)[0] < 98, "left island, red {:?}", at(0.15, 0.5));
    assert!(at(0.25, 0.41)[1] < 98, "left island, green {:?}", at(0.25, 0.41));
    // Right island: the right bump's left side (x 0.3) leans -X, which is +U
    // there: more red; its lower side still leans -V: less green, which only
    // the MikkTSpace sign gets right.
    assert!(at(0.85, 0.5)[0] > 158, "right island, red {:?}", at(0.85, 0.5));
    assert!(at(0.75, 0.41)[1] < 98, "right island, green {:?}", at(0.75, 0.41));
    let (worst, mean) = read_back(&low, &normal, |p| normal_of(height, p.x, p.y));
    assert!(worst < 0.04 && mean < 0.01, "worst {worst}, mean {mean}");
}

#[test]
fn objects_are_baked_where_they_stand_even_reflected() {
    let off_centre = |x: f32, y: f32| bump_at(Vec2::new(0.3, 0.2), x, y);
    let high = sheet(200, off_centre);
    let maps = Maps { normal: true, displacement: true, ..NONE };
    let here = go(&low_plane(), &[plain(&high)], &settings(maps));
    let turned = Mat4::from_scale_rotation_translation(
        Vec3::ONE,
        Quat::from_euler(glam::EulerRot::XYZ, 0.4, -0.9, 1.3),
        Vec3::new(3.0, -2.0, 0.5),
    );
    let reflected = Mat4::from_translation(Vec3::new(-1.0, 0.0, 2.0)) * Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0));
    for place in [turned, reflected] {
        let low = LowModel { to_world: place, ..low_plane() };
        let there = go(&low, &[Source { mesh: &high, to_world: place, colours: None }], &settings(maps));
        let (a, b) = (here.normal.as_ref().unwrap(), there.normal.as_ref().unwrap());
        let worst =
            a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|c| p.0[c].abs_diff(q.0[c])).max().unwrap()).max();
        assert!(worst <= Some(2), "normal maps differ by {worst:?}");
        let (a, b) = (&here.displacement.as_ref().unwrap().0, &there.displacement.as_ref().unwrap().0);
        let worst = a.pixels().zip(b.pixels()).map(|(p, q)| (p.0[0] - q.0[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-4, "displacement differs by {worst}");
    }
}

#[test]
fn a_reflected_sculpt_is_taken_the_right_way_out() {
    // A live mirror copy: the sheet's own data mirrored (and wound back the
    // right way out), placed by a reflection, stands exactly where the sheet
    // does, so it bakes the same maps.
    let off_centre = |x: f32, y: f32| bump_at(Vec2::new(0.3, 0.2), x, y);
    let high = sheet(200, off_centre);
    let positions = high.positions.iter().map(|p| Vec3::new(-p.x, p.y, p.z)).collect();
    let indices = high.indices.chunks_exact(3).flat_map(|t| [t[0], t[2], t[1]]).collect();
    let copy = Mesh::new(positions, indices, None);
    let maps = Maps { normal: true, displacement: true, ..NONE };
    let here = go(&low_plane(), &[plain(&high)], &settings(maps));
    let mirror = Source { mesh: &copy, to_world: Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)), colours: None };
    let there = go(&low_plane(), &[mirror], &settings(maps));
    assert_eq!(there.missed, 0.0);
    let (a, b) = (here.normal.unwrap(), there.normal.unwrap());
    let worst = a.pixels().zip(b.pixels()).map(|(p, q)| (0..3).map(|c| p.0[c].abs_diff(q.0[c])).max().unwrap()).max();
    assert!(worst <= Some(2), "normal maps differ by {worst:?}");
}

#[test]
fn texels_that_find_nothing_are_counted_as_missed() {
    // The sculpt covers only the left half; the right half finds it only
    // within reach (0.1) of its edge.
    let low = low_plane();
    let half = sheet_over(64, [-1.0, 0.0], [-1.0, 1.0], |_, _| 0.0);
    let baked = go(&low, &[plain(&half)], &Settings { reach: 0.1, ..settings(Maps { displacement: true, ..NONE }) });
    assert!((baked.missed - 0.45).abs() < 0.01, "missed {}", baked.missed);
}

#[test]
fn a_sculpt_out_of_reach_gives_a_plain_error() {
    let low = low_plane();
    let high = sheet(16, bump);
    let away = Source { mesh: &high, to_world: Mat4::from_translation(Vec3::new(10.0, 0.0, 0.0)), colours: None };
    let result = bake(&low.target(), &[away], &settings(Maps { normal: true, ..NONE }), &|_| true);
    let error = result.err().expect("an error");
    assert!(error.contains("not in the same place") && error.contains("Reach"), "{error}");
}

#[test]
fn a_model_without_uvs_or_with_the_wrong_count_or_size_is_refused() {
    let high = sheet(8, bump);
    let maps = Maps { normal: true, ..NONE };
    let refused = |low: &LowModel, settings: &Settings| {
        bake(&low.target(), &[plain(&high)], settings, &|_| true).err().expect("refused")
    };
    let bare = LowModel { uvs: Vec::new(), ..low_plane() };
    assert!(refused(&bare, &settings(maps)).contains("no UVs"));
    let short = LowModel { uvs: vec![Vec2::ZERO; 5], ..low_plane() };
    assert!(refused(&short, &settings(maps)).contains("do not match"));
    for size in [256, 16_384] {
        assert!(refused(&low_plane(), &Settings { size, ..settings(maps) }).contains("512"));
    }
    assert!(refused(&low_plane(), &settings(NONE)).contains("at least one map"));
    let outside = plane_with(|p| Vec2::new(p.x + 3.0, p.y + 3.0));
    assert!(refused(&outside, &settings(maps)).contains("cover none"));
}

#[test]
fn stopping_the_bake_gives_the_cancelled_error_promptly() {
    let low = low_plane();
    let high = sheet(256, bump);
    let maps = Maps { normal: true, ao: true, ..NONE };
    let at_once = bake(&low.target(), &[plain(&high)], &settings(maps), &|_| false);
    assert_eq!(at_once.err().as_deref(), Some(CANCELLED));
    // A long bake, stopped at the second report.
    let calls = Cell::new(0);
    let stopped_at = Cell::new(None);
    let heavy = Settings { size: 4096, ao_rays: 256, ..settings(maps) };
    let result = bake(&low.target(), &[plain(&high)], &heavy, &|_| {
        calls.set(calls.get() + 1);
        if calls.get() < 2 {
            return true;
        }
        stopped_at.set(stopped_at.get().or(Some(Instant::now())));
        false
    });
    assert_eq!(result.err().as_deref(), Some(CANCELLED));
    let after = stopped_at.get().expect("asked to stop").elapsed().as_secs_f32();
    assert!(after < 1.0, "stopped {after} s after being asked");
}

#[test]
fn the_same_bake_twice_gives_the_same_pictures() {
    let low = low_plane();
    let high = sheet(128, bump);
    let paint: Vec<[u8; 4]> = high.positions.iter().map(|p| [(p.x * 100.0 + 128.0) as u8, 9, 99, 255]).collect();
    let source = || Source { mesh: &high, to_world: Mat4::IDENTITY, colours: Some(&paint) };
    let (a, b) = (go(&low, &[source()], &settings(ALL)), go(&low, &[source()], &settings(ALL)));
    assert!(a.normal == b.normal && a.ao == b.ao && a.colour == b.colour);
    assert_eq!(a.displacement.unwrap().0.as_raw(), b.displacement.unwrap().0.as_raw());
}

#[test]
fn saved_maps_read_back() {
    let low = low_plane();
    let high = sheet(128, bump);
    let paint: Vec<[u8; 4]> =
        high.positions.iter().map(|p| if p.y < 0.0 { [250, 200, 20, 255] } else { [20, 90, 240, 255] }).collect();
    let painted = Source { mesh: &high, to_world: Mat4::IDENTITY, colours: Some(&paint) };
    let baked = go(&low, &[painted], &settings(ALL));
    let folder = out_dir("save");
    let _ = std::fs::remove_dir_all(&folder);
    let written = save(&baked, &folder, "rock").expect("saves");
    let names: Vec<String> = written.iter().map(|p| p.file_name().unwrap().to_string_lossy().into_owned()).collect();
    assert_eq!(
        names,
        [
            "rock_normal.png",
            "rock_displacement.exr",
            "rock_displacement.png",
            "rock_displacement.txt",
            "rock_ao.png",
            "rock_colour.png"
        ]
    );
    let open = |name: &str| image::open(folder.join(name)).expect("reads back");
    assert!(open("rock_normal.png").to_rgb8() == *baked.normal.as_ref().unwrap());
    assert!(open("rock_ao.png").to_luma8() == *baked.ao.as_ref().unwrap());
    assert!(open("rock_colour.png").to_rgba8() == *baked.colour.as_ref().unwrap());
    let (height, lo, hi) = baked.displacement.as_ref().unwrap();
    let exr = open("rock_displacement.exr").to_rgb32f();
    assert!(exr.pixels().zip(height.pixels()).all(|(e, h)| e.0 == [h.0[0]; 3]), "the EXR holds the distances exactly");
    let grey = open("rock_displacement.png").to_luma16();
    let (tx, ty) = texel(0.0, 0.0, 512);
    assert_eq!(grey.get_pixel(tx, ty).0[0], 65535, "the top of the bump is white");
    assert_eq!(
        grey.get_pixel(5, 5).0[0],
        ((0.0 - lo) / (hi - lo) * 65535.0).round() as u16,
        "the flat sheet is the zero grey"
    );
    let note = std::fs::read_to_string(folder.join("rock_displacement.txt")).unwrap();
    assert!(note.contains("Midlevel") && note.contains(&format!("{hi:.6}")));
}

/// How long a bake of a game-sized model takes: a globe of 5,000 quads with
/// UVs, from a million-triangle sculpt. Run with `cargo test --release
/// bake_timing -- --ignored --nocapture`.
#[test]
#[ignore]
fn bake_timing() {
    let low = globe(50, 100);
    let sphere = primitives::build(Primitive::Sphere, 1_000_000, 1.0);
    let wavy = |p: Vec3| p * (1.0 + 0.01 * (20.0 * p.x).sin() * (20.0 * p.y).sin() * (20.0 * p.z).sin());
    let high = Mesh::new(sphere.positions.iter().map(|&p| wavy(p)).collect(), sphere.indices.clone(), None);
    let paint: Vec<[u8; 4]> = high.positions.iter().map(|p| [(p.x * 120.0 + 128.0) as u8, 128, 128, 255]).collect();
    println!("low: {} triangles; high: {} triangles", low.indices.len() / 3, high.triangle_count());
    for (what, maps, rays, size) in [
        ("normal + displacement", Maps { normal: true, displacement: true, ..NONE }, 0, 2048),
        ("normal + displacement + colour", Maps { normal: true, displacement: true, colour: true, ..NONE }, 0, 2048),
        ("ambient occlusion, 16 rays", Maps { ao: true, ..NONE }, 16, 2048),
        ("ambient occlusion, 64 rays", Maps { ao: true, ..NONE }, 64, 2048),
        ("normal + displacement", Maps { normal: true, displacement: true, ..NONE }, 0, 4096),
    ] {
        let source = Source { mesh: &high, to_world: Mat4::IDENTITY, colours: Some(&paint) };
        let settings = Settings { size, reach: 0.05, maps, ao_rays: rays, padding: 8 };
        let start = Instant::now();
        let baked = bake(&low.target(), &[source], &settings, &|_| true).expect("bakes");
        println!("{size}x{size} {what}: {:.2} s (missed {:.4})", start.elapsed().as_secs_f32(), baked.missed);
        if what.starts_with("normal") && size == 2048 && rays == 0 {
            keep("timing_normal", |p| baked.normal.as_ref().unwrap().save(p));
        }
    }
}
