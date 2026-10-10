//! Starting shapes. All are generated mirror-exact across the three
//! coordinate planes (positions and triangulation), so symmetric sculpting
//! starts from a symmetric mesh.

use std::collections::HashMap;

use std::f32::consts::{FRAC_PI_2, FRAC_PI_4};

use glam::{Vec2, Vec3};

use super::Mesh;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Primitive {
    Sphere,
    Cube,
    Cylinder,
    Capsule,
    Cone,
    Wedge,
}

impl Primitive {
    pub const ALL: [Primitive; 6] = [
        Primitive::Cube,
        Primitive::Sphere,
        Primitive::Cylinder,
        Primitive::Capsule,
        Primitive::Cone,
        Primitive::Wedge,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Primitive::Sphere => "Sphere",
            Primitive::Cube => "Cube",
            Primitive::Cylinder => "Cylinder",
            Primitive::Capsule => "Capsule",
            Primitive::Cone => "Cone",
            Primitive::Wedge => "Wedge",
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            Primitive::Sphere => "prim_sphere",
            Primitive::Cube => "prim_cube",
            Primitive::Cylinder => "prim_cylinder",
            Primitive::Capsule => "prim_capsule",
            Primitive::Cone => "prim_cone",
            Primitive::Wedge => "prim_wedge",
        }
    }
}

/// Maps the square [-1, 1]^2 onto the unit disc, edge to rim, evenly and
/// mirror-exact in both axes.
fn disc(a: f32, b: f32) -> Vec2 {
    if a == 0.0 && b == 0.0 {
        Vec2::ZERO
    } else if a.abs() >= b.abs() {
        let phi = FRAC_PI_4 * (b / a);
        Vec2::new(a * phi.cos(), a * phi.sin())
    } else {
        let phi = FRAC_PI_4 * (a / b);
        Vec2::new(b * phi.sin(), b * phi.cos())
    }
}

/// Where a point `p` on the surface of the cube [-1, 1]^3 goes for each
/// shape. Every shape is the cube lattice bent into place, so its sharp edges
/// run along mesh edges and there are no slivers. Sizes are in units of the
/// half-extent.
///
/// The cylinder and the capsule stand twice as tall as they are wide, as
/// Unity's built-in ones do (radius 0.5, height 2, the capsule's height
/// taking in its domes), so a shape added here is the shape the Unity app
/// adds.
fn shape(kind: Primitive, p: Vec3) -> Vec3 {
    let d = disc(p.x, p.z);
    let rho = d.length();
    let around = if rho > 0.0 { d / rho } else { Vec2::ZERO };
    match kind {
        Primitive::Sphere => p.normalize(),
        Primitive::Cube => p,
        Primitive::Cylinder => Vec3::new(d.x, p.y * CYLINDER_HALF_HEIGHT, d.y),
        Primitive::Capsule => {
            // Wall from the side faces, a dome from each end face.
            let (radius, half) = (1.0, CAPSULE_HALF_WALL);
            if p.y.abs() == 1.0 {
                let theta = rho * FRAC_PI_2;
                let ring = around * (radius * theta.sin());
                Vec3::new(ring.x, (half + radius * theta.cos()).copysign(p.y), ring.y)
            } else {
                Vec3::new(d.x * radius, p.y * half, d.y * radius)
            }
        }
        Primitive::Cone => {
            // The top face and the sides together unroll onto the slope; the
            // bottom face is the base.
            if p.y == -1.0 {
                Vec3::new(d.x, -1.0, d.y)
            } else {
                let t = if p.y == 1.0 { 0.5 * rho } else { 0.5 + (1.0 - p.y) * 0.25 };
                Vec3::new(around.x * t, 1.0 - 2.0 * t, around.y * t)
            }
        }
        Primitive::Wedge => {
            // A ramp: the top and front faces lie side by side on the slope.
            let (u, v) = ((p.z + 1.0) * 0.5, (p.y + 1.0) * 0.5);
            let z = -(1.0 - u) * (1.0 - v) + u * (1.0 - v) - (1.0 - u) * v;
            let y = -(1.0 - u) * (1.0 - v) - u * (1.0 - v) + (1.0 - u) * v;
            Vec3::new(p.x, y, z)
        }
    }
}

/// Lattice coordinate in [-1, 1], built so that `coord(i) == -coord(n - i)`
/// holds exactly. `warp` spaces samples by equal angle, which evens out
/// triangle size once the cube is projected onto a sphere.
fn lattice(n: usize, warp: bool) -> Vec<f32> {
    let f = |k: usize| {
        let t = k as f64 / n as f64;
        if warp { (t * std::f64::consts::FRAC_PI_4).tan() as f32 } else { t as f32 }
    };
    (0..=n)
        .map(|i| match (2 * i).cmp(&n) {
            std::cmp::Ordering::Less => -f(n - 2 * i),
            std::cmp::Ordering::Equal => 0.0,
            std::cmp::Ordering::Greater => f(2 * i - n),
        })
        .collect()
}

/// Half the cylinder's height, in units of its radius.
const CYLINDER_HALF_HEIGHT: f32 = 2.0;
/// Half the height of the capsule's straight wall, in units of its radius;
/// the domes add one radius at each end.
const CAPSULE_HALF_WALL: f32 = 1.0;
/// The most steps the lattice takes across a shape.
const MAX_STEPS: usize = 2048;

/// How many steps the lattice takes along x, y and z to come out at about
/// `target_tris` triangles. Even counts keep the diagonals mirrored.
///
/// A tall shape gets more steps up its wall than across it, so the wall's
/// triangles stay as even as a cube's instead of being stretched to fit.
fn steps(kind: Primitive, target_tris: usize) -> [usize; 3] {
    // Steps up the wall for each step across it: the wall's height over a
    // quarter of the way round it.
    let tall = match kind {
        Primitive::Cylinder => 2.0 * CYLINDER_HALF_HEIGHT as f64 / std::f64::consts::FRAC_PI_2,
        Primitive::Capsule => 2.0 * CAPSULE_HALF_WALL as f64 / std::f64::consts::FRAC_PI_2,
        _ => 1.0,
    };
    let even = |steps: f64, most: usize| ((steps / 2.0).round() as usize * 2).clamp(2, most);
    // 4 side faces of n * ny quads and 2 end faces of n * n, 2 triangles each.
    let n = even((target_tris as f64 / (4.0 * (2.0 * tall + 1.0))).sqrt(), MAX_STEPS);
    let ny = if tall == 1.0 { n } else { even(n as f64 * tall, MAX_STEPS * 3) };
    [n, ny, n]
}

/// How many triangles [`build`] makes of a shape for this target.
#[cfg(test)]
pub fn triangle_count(kind: Primitive, target_tris: usize) -> usize {
    let [x, y, z] = steps(kind, target_tris);
    4 * (x * y + y * z + z * x)
}

/// A subdivided cube bent into the shape asked for. `target_tris` is
/// approximate; `size` is the radius (sphere), half the width (cylinder and
/// capsule, which are twice as tall) or the half-extent (the others).
pub fn build(kind: Primitive, target_tris: usize, size: f32) -> Mesh {
    let counts = steps(kind, target_tris);
    let sphere = kind == Primitive::Sphere;
    let coords = counts.map(|n| lattice(n, sphere));

    let quads = 2 * (counts[0] * counts[1] + counts[1] * counts[2] + counts[2] * counts[0]);
    let mut ids: HashMap<[u16; 3], u32> = HashMap::with_capacity(quads + 2);
    let mut positions: Vec<Vec3> = Vec::with_capacity(quads + 2);
    let mut indices: Vec<u32> = Vec::with_capacity(quads * 6);

    // (fixed axis, fixed side, u axis, v axis) with u x v pointing outward.
    let faces: [(usize, bool, usize, usize); 6] = [
        (0, true, 1, 2),
        (0, false, 2, 1),
        (1, true, 2, 0),
        (1, false, 0, 2),
        (2, true, 0, 1),
        (2, false, 1, 0),
    ];

    for (axis, positive, ua, va) in faces {
        let (nu, nv) = (counts[ua], counts[va]);
        let mut vertex = |i: usize, j: usize| -> u32 {
            let mut key = [0u16; 3];
            key[axis] = if positive { counts[axis] as u16 } else { 0 };
            key[ua] = i as u16;
            key[va] = j as u16;
            *ids.entry(key).or_insert_with(|| {
                let p = Vec3::new(
                    coords[0][key[0] as usize],
                    coords[1][key[1] as usize],
                    coords[2][key[2] as usize],
                );
                positions.push(shape(kind, p) * size);
                positions.len() as u32 - 1
            })
        };
        for j in 0..nv {
            for i in 0..nu {
                let a = vertex(i, j);
                let b = vertex(i + 1, j);
                let c = vertex(i + 1, j + 1);
                let d = vertex(i, j + 1);
                // Diagonals fan out from the face centre, which is symmetric
                // under both mirror lines of the face.
                if (i < nu / 2) == (j < nv / 2) {
                    indices.extend_from_slice(&[a, b, c, a, c, d]);
                } else {
                    indices.extend_from_slice(&[a, b, d, b, c, d]);
                }
            }
        }
    }

    Mesh::new(positions, indices, None)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn the_cylinder_and_capsule_are_twice_as_tall_as_wide_like_unitys() {
        // Unity's built-in cylinder and capsule: radius 0.5, height 2.
        for kind in [Primitive::Cylinder, Primitive::Capsule] {
            let mesh = build(kind, 20_000, 0.5);
            let size = mesh.bounds().size();
            assert!((size - Vec3::new(1.0, 2.0, 1.0)).abs().max_element() < 1e-5, "{}: {size}", kind.name());
            assert!(mesh.bounds().center().length() < 1e-6, "{} is centred", kind.name());
        }
        // The cylinder's wall is a true circle of radius 0.5 with flat ends.
        let cylinder = build(Primitive::Cylinder, 20_000, 0.5);
        for p in &cylinder.positions {
            let r = Vec2::new(p.x, p.z).length();
            assert!(r < 0.5 + 1e-5);
            assert!((r - 0.5).abs() < 1e-5 || (p.y.abs() - 1.0).abs() < 1e-6, "{p} is on the wall or an end");
        }
        // Every point of the capsule is 0.5 from its axis, which is 1 long.
        let capsule = build(Primitive::Capsule, 20_000, 0.5);
        for p in &capsule.positions {
            let on_axis = Vec3::new(0.0, p.y.clamp(-0.5, 0.5), 0.0);
            assert!((p.distance(on_axis) - 0.5).abs() < 1e-5, "{p}");
        }
        // The other shapes are as they were: 1 across every way.
        for kind in [Primitive::Cube, Primitive::Sphere, Primitive::Cone, Primitive::Wedge] {
            let size = build(kind, 20_000, 0.5).bounds().size();
            assert!((size - Vec3::ONE).abs().max_element() < 1e-5, "{}: {size}", kind.name());
        }
    }

    #[test]
    fn every_shape_comes_out_near_the_triangle_count_asked_for() {
        for kind in Primitive::ALL {
            for target in [20_000usize, 200_000, 1_000_000, 4_000_000] {
                let got = triangle_count(kind, target);
                let off = got.abs_diff(target) as f64 / target as f64;
                assert!(off < 0.08, "{} at {target}: {got}", kind.name());
                if target == 20_000 {
                    assert_eq!(build(kind, target, 0.5).triangle_count(), got);
                }
            }
        }
        // Nothing but the two tall shapes changed its lattice.
        assert_eq!(steps(Primitive::Sphere, 200_000), [130; 3]);
        assert_eq!(steps(Primitive::Cube, 20_000), [40; 3]);
    }

    #[test]
    fn the_tall_shapes_keep_even_triangles_and_stay_mirror_exact() {
        for kind in [Primitive::Cylinder, Primitive::Capsule] {
            let mesh = build(kind, 20_000, 0.5);
            // On the straight wall a triangle's sides are a step round, a
            // step up and the diagonal: close to a right isosceles triangle,
            // not a sliver stretched to twice the height.
            let half_wall = if kind == Primitive::Cylinder { 1.0 } else { 0.5 };
            let mut worst = 1.0f32;
            for t in mesh.indices.chunks_exact(3) {
                let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize]);
                let on_wall = p.iter().all(|q| q.y.abs() <= half_wall && (Vec2::new(q.x, q.z).length() - 0.5).abs() < 1e-5);
                if on_wall {
                    let sides = [p[0].distance(p[1]), p[1].distance(p[2]), p[2].distance(p[0])];
                    let (short, long) = (sides.iter().copied().fold(f32::MAX, f32::min), sides.iter().copied().fold(0.0, f32::max));
                    worst = worst.max(long / short);
                }
            }
            assert!(worst > 1.3 && worst < 1.75, "{}: longest side is {worst} times the shortest", kind.name());

            let key = |p: Vec3| [(p.x + 0.0).to_bits(), (p.y + 0.0).to_bits(), (p.z + 0.0).to_bits()];
            let all: HashSet<[u32; 3]> = mesh.positions.iter().map(|&p| key(p)).collect();
            for &p in &mesh.positions {
                for flip in [Vec3::new(-1.0, 1.0, 1.0), Vec3::new(1.0, -1.0, 1.0), Vec3::new(1.0, 1.0, -1.0)] {
                    assert!(all.contains(&key(p * flip)), "{}: {p} has no twin across {flip}", kind.name());
                }
            }
        }
    }
}
