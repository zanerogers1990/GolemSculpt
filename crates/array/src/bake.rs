//! Baking an array: every copy's geometry, the object's own included, as one
//! mesh. What "Apply As One Object" swaps in.
//!
//! The source may have millions of triangles and there may be dozens of copies,
//! so the output is allocated once at its exact size and every copy is filled in
//! parallel, block by block, so even two copies of one huge mesh use every
//! thread. A copy whose transform mirrors keeps its outside outside: its
//! triangles are wound the other way, as `TriMesh::transform` does.

use glam::{Mat3, Mat4, Vec3};
use rayon::prelude::*;
use sculpt_geom::TriMesh;

use crate::types::{ArrayPlacement, Pose};

/// Past this many vertices in one mesh, baking refuses rather than freezing on
/// (or failing) the allocation.
pub const MAX_COMBINED_VERTICES: u64 = 24_000_000;

/// Vertices per parallel task: big enough that scheduling is noise, small
/// enough that one copy of a large mesh still spreads over every thread.
const VERTEX_BLOCK: usize = 16 * 1024;
/// Indices per parallel task: a multiple of three, so a triangle never straddles two.
const INDEX_BLOCK: usize = 3 * 8 * 1024;

/// Each copy's matrix in the object's own local space, in copy order: what
/// `bake` takes. Copy 0 is exactly the identity, so the object's own geometry
/// comes through untouched rather than sent out through its transform and back.
pub fn local_matrices(placements: &[ArrayPlacement], object: &Pose) -> Vec<Mat4> {
    let inverse = object.matrix().inverse();
    placements.iter().map(|p| p.local_matrix_with(&inverse, object)).collect()
}

/// `bake` for an array's placements: the copies of `source` (the object's mesh,
/// in its own local space) laid out as the array says, in that same space.
pub fn bake_array(source: &TriMesh, placements: &[ArrayPlacement], object: &Pose) -> Result<TriMesh, String> {
    bake(source, &local_matrices(placements, object))
}

/// Whether `copies` copies of a mesh with `vertex_count` vertices can be one
/// mesh, with a message the user can read if not.
pub fn check_size(vertex_count: usize, copies: usize) -> Result<(), String> {
    if vertex_count == 0 || copies == 0 {
        return Err("Nothing to apply.".to_string());
    }
    if vertex_count as u64 * copies as u64 > MAX_COMBINED_VERTICES {
        return Err(format!(
            "Too big for one object: {copies} copies x {} vertices. Lower the count, or apply as separate objects.",
            grouped(vertex_count)
        ));
    }
    Ok(())
}

/// One mesh holding a copy of `source` per matrix, in order: copy `c`'s
/// vertices are `transforms[c]` applied to the source's, and sit at
/// `c * vertex_count` onward, with its triangles indexing them.
///
/// A matrix that is exactly the identity copies the source verbatim. One with a
/// negative determinant reverses its copy's winding.
pub fn bake(source: &TriMesh, transforms: &[Mat4]) -> Result<TriMesh, String> {
    check_size(source.positions.len(), transforms.len())?;

    let moves: Vec<Option<Mat4>> = transforms.iter().map(|m| (*m != Mat4::IDENTITY).then_some(*m)).collect();
    let flips: Vec<bool> = moves.iter().map(|m| m.is_some_and(|m| m.determinant() < 0.0)).collect();

    let positions = par_copies(&source.positions, &moves, |m, p| m.transform_point3(p));
    let indices = bake_indices(&source.indices, source.positions.len(), &flips);
    Ok(TriMesh::new(positions, indices))
}

/// The per-vertex normals of a baked array, copy by copy, to go with `bake`'s
/// positions. Normals go through the inverse transpose: a non-uniformly scaled
/// object makes its copies' matrices shear in the object's own space, which the
/// plain matrix would tilt every normal wrong for. A mirrored copy needs no
/// reversal here; only its triangles' winding does.
pub fn bake_normals(normals: &[Vec3], transforms: &[Mat4]) -> Result<Vec<Vec3>, String> {
    check_size(normals.len(), transforms.len())?;

    let moves: Vec<Option<Mat3>> =
        transforms.iter().map(|m| (*m != Mat4::IDENTITY).then(|| Mat3::from_mat4(*m).inverse().transpose())).collect();
    Ok(par_copies(normals, &moves, |m, n| {
        let t = *m * n;
        let len = t.length();
        // A singular matrix has no normal matrix; keep the source's.
        if len > 1e-12 { t / len } else { n }
    }))
}

/// `states.len()` consecutive copies of `source` in one exactly sized buffer,
/// copy `c` being `apply(state, v)` of each vertex `v` (verbatim for a `None`
/// state), written in parallel.
///
/// The buffer is never zeroed first: that would be a single-threaded pass over
/// hundreds of megabytes just before every byte is overwritten.
fn par_copies<S: Sync>(source: &[Vec3], states: &[Option<S>], apply: impl Fn(&S, Vec3) -> Vec3 + Sync) -> Vec<Vec3> {
    let len = source.len();
    let total = len * states.len();
    if total == 0 {
        return Vec::new();
    }

    let mut out: Vec<Vec3> = Vec::with_capacity(total);
    out.spare_capacity_mut()[..total].par_chunks_mut(len).zip(states.par_iter()).for_each(|(copy, state)| {
        copy.par_chunks_mut(VERTEX_BLOCK).zip(source.par_chunks(VERTEX_BLOCK)).for_each(|(dst, src)| match state {
            Some(s) => dst.iter_mut().zip(src).for_each(|(d, &v)| _ = d.write(apply(s, v))),
            None => dst.iter_mut().zip(src).for_each(|(d, &v)| _ = d.write(v)),
        });
    });
    // SAFETY: `total` is `len * states.len()`, so `par_chunks_mut(len)` yields
    // exactly one chunk per state, each exactly `len` long; the inner chunks of a
    // copy and of the source are the same length and number, because both are
    // `len` long cut at the same block size. Every element of every chunk is
    // therefore written above, and `for_each` has returned (or has resumed a
    // panic, in which case this line is never reached) before the length is set.
    unsafe { out.set_len(total) };
    out
}

/// The source's indices once per copy, each shifted to its copy's vertices, with
/// the second and third corner of every triangle swapped for the copies in
/// `flips`.
fn bake_indices(source: &[u32], vertex_count: usize, flips: &[bool]) -> Vec<u32> {
    let len = source.len();
    let mut out = vec![0u32; len * flips.len()];
    if len == 0 {
        return out;
    }

    out.par_chunks_mut(len).zip(flips.par_iter()).enumerate().for_each(|(c, (copy, &flip))| {
        let base = (c * vertex_count) as u32;
        copy.par_chunks_mut(INDEX_BLOCK).zip(source.par_chunks(INDEX_BLOCK)).for_each(|(dst, src)| {
            // A source whose length is not a multiple of three (damaged, but not
            // ours to refuse) keeps its trailing indices, shifted, unflipped.
            let (dst_tris, dst_rest) = dst.as_chunks_mut::<3>();
            let (src_tris, src_rest) = src.as_chunks::<3>();
            if flip {
                for (d, s) in dst_tris.iter_mut().zip(src_tris) {
                    *d = [s[0] + base, s[2] + base, s[1] + base];
                }
            } else {
                dst_tris.iter_mut().zip(src_tris).for_each(|(d, s)| *d = s.map(|i| i + base));
            }
            dst_rest.iter_mut().zip(src_rest).for_each(|(d, &s)| *d = s + base);
        });
    });
    out
}

/// 24000000 as "24,000,000".
pub(crate) fn grouped(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use glam::Quat;

    use super::*;
    use crate::testing::{tetra, uv_sphere};

    fn some_transforms() -> Vec<Mat4> {
        vec![
            Mat4::IDENTITY,
            Mat4::from_translation(Vec3::new(3.0, 0.0, 0.0)),
            Mat4::from_rotation_translation(Quat::from_rotation_y(0.7), Vec3::new(0.0, 4.0, -1.0)),
            Mat4::from_scale_rotation_translation(Vec3::new(0.8, 1.3, 0.6), Quat::from_rotation_x(1.1), Vec3::ONE),
            Mat4::from_scale_rotation_translation(Vec3::new(-1.0, 1.0, 1.0), Quat::from_rotation_z(0.3), Vec3::X * 9.0),
        ]
    }

    #[test]
    fn the_sphere_used_here_is_wound_outward() {
        let sphere = uv_sphere(24, 48, 1.0);
        let exact = 4.0 / 3.0 * std::f64::consts::PI;
        assert!((sphere.signed_volume() - exact).abs() / exact < 0.01, "{}", sphere.signed_volume());
    }

    #[test]
    fn n_copies_have_n_times_the_triangles_and_n_times_the_volume() {
        let sphere = uv_sphere(24, 48, 1.0);
        let one = sphere.signed_volume();
        for n in [2usize, 5, 37] {
            let transforms: Vec<Mat4> = (0..n)
                .map(|i| {
                    Mat4::from_rotation_translation(Quat::from_rotation_y(i as f32 * 0.3), Vec3::X * (2.5 * i as f32))
                })
                .collect();
            let baked = bake(&sphere, &transforms).unwrap();
            assert_eq!(baked.triangle_count(), n * sphere.triangle_count());
            assert_eq!(baked.vertex_count(), n * sphere.vertex_count());
            let ratio = baked.signed_volume() / one;
            assert!((ratio - n as f64).abs() < 1e-3 * n as f64, "{n} copies hold {ratio} volumes");
        }
    }

    #[test]
    fn scaled_copies_scale_the_volume_by_the_cube() {
        let sphere = uv_sphere(24, 48, 1.0);
        let half = Mat4::from_scale(Vec3::splat(0.5));
        let baked = bake(&sphere, &[Mat4::IDENTITY, half]).unwrap();
        let ratio = baked.signed_volume() / sphere.signed_volume();
        assert!((ratio - 1.125).abs() < 1e-4, "{ratio}");
    }

    #[test]
    fn a_mirroring_copy_keeps_its_outside_out() {
        let sphere = uv_sphere(24, 48, 1.0);
        let one = sphere.signed_volume();
        for mirror in [
            Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)),
            Mat4::from_scale(Vec3::new(-1.0, -1.0, -1.0)),
            Mat4::from_translation(Vec3::new(5.0, 0.0, 0.0)) * Mat4::from_scale(Vec3::new(1.0, 1.0, -2.0)),
            Mat4::from_scale(Vec3::new(-1.0, -1.0, 1.0)),
        ] {
            let det = mirror.determinant();
            let baked = bake(&sphere, &[mirror]).unwrap();
            let expected = one * det.abs() as f64;
            assert!(baked.signed_volume() > 0.0, "det {det}: the copy is inside out");
            assert!((baked.signed_volume() - expected).abs() < 1e-4 * expected, "det {det}");
            // A reflection reverses winding; a rotation (two reflections) leaves it alone.
            let reversed = baked.indices[1] == sphere.indices[2];
            assert_eq!(reversed, det < 0.0, "det {det}");
        }
    }

    #[test]
    fn baking_is_the_same_as_building_it_copy_by_copy() {
        // Bigger than two parallel blocks of vertices and of indices, so block
        // boundaries inside a copy are covered.
        let sphere = uv_sphere(150, 300, 1.0);
        assert!(sphere.vertex_count() > 2 * VERTEX_BLOCK && sphere.indices.len() > 2 * INDEX_BLOCK);
        let transforms = some_transforms();

        let mut expected = TriMesh::default();
        for m in &transforms {
            let mut copy = sphere.clone();
            copy.transform(m);
            expected.append(&copy);
        }
        assert_eq!(bake(&sphere, &transforms).unwrap(), expected);
    }

    #[test]
    fn copy_zero_comes_through_bit_for_bit() {
        let mut odd = tetra();
        // -0.0 is the one value an identity multiply-and-add would change.
        odd.positions[0] = Vec3::new(-0.0, 0.3, f32::MIN_POSITIVE);
        let baked = bake(&odd, &[Mat4::IDENTITY, Mat4::from_translation(Vec3::ONE)]).unwrap();
        for (a, b) in baked.positions[..4].iter().zip(&odd.positions) {
            assert_eq!(a.to_array().map(f32::to_bits), b.to_array().map(f32::to_bits));
        }
        assert_eq!(baked.indices[..12], odd.indices[..]);
    }

    #[test]
    fn indices_follow_their_copys_vertices() {
        let t = tetra();
        let baked = bake(&t, &[Mat4::IDENTITY, Mat4::from_translation(Vec3::X), Mat4::IDENTITY]).unwrap();
        for c in 0..3usize {
            for i in 0..12 {
                assert_eq!(baked.indices[c * 12 + i], t.indices[i] + 4 * c as u32);
            }
        }
    }

    #[test]
    fn a_source_with_a_stray_index_keeps_it() {
        let mut t = tetra();
        t.indices.push(2);
        let baked = bake(&t, &[Mat4::IDENTITY, Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0))]).unwrap();
        assert_eq!(baked.indices.len(), 26);
        assert_eq!(baked.indices[25], 2 + 4, "the stray index is shifted but not flipped");
        assert_eq!(baked.indices[13..16], [4, 5, 6], "the mirrored copy's first triangle has corners 2 and 3 swapped");
    }

    #[test]
    fn a_mesh_with_no_triangles_still_gets_its_vertices_copied() {
        let points = TriMesh::new(vec![Vec3::X, Vec3::Y], Vec::new());
        let baked = bake(&points, &[Mat4::IDENTITY, Mat4::from_translation(Vec3::Z)]).unwrap();
        assert_eq!((baked.vertex_count(), baked.indices.len()), (4, 0));
        assert_eq!(baked.positions[2], Vec3::new(1.0, 0.0, 1.0));
    }

    #[test]
    fn nothing_to_bake_is_said_plainly() {
        let t = tetra();
        assert_eq!(bake(&t, &[]).unwrap_err(), "Nothing to apply.");
        assert_eq!(bake(&TriMesh::default(), &[Mat4::IDENTITY]).unwrap_err(), "Nothing to apply.");
        assert_eq!(bake_normals(&[], &[Mat4::IDENTITY]).unwrap_err(), "Nothing to apply.");
    }

    #[test]
    fn too_big_for_one_mesh_says_how_big() {
        assert!(check_size(240_000, 100).is_ok());
        let e = check_size(1_000_000, 25).unwrap_err();
        assert!(e.contains("25 copies x 1,000,000 vertices"), "{e}");
        assert!(check_size(24_000_001, 1).is_err());
        assert_eq!(grouped(0), "0");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1000), "1,000");
        assert_eq!(grouped(1_234_567), "1,234,567");
    }

    #[test]
    fn the_largest_array_still_bakes() {
        let t = tetra();
        let baked = bake(&t, &vec![Mat4::IDENTITY; 100]).unwrap();
        assert_eq!(baked.vertex_count(), 400);
        assert_eq!(baked.triangle_count(), 400);
    }

    #[test]
    fn normals_stay_perpendicular_to_a_stretched_and_mirrored_surface() {
        let sphere = uv_sphere(12, 24, 1.0);
        let normals: Vec<Vec3> = sphere.positions.iter().map(|p| p.normalize()).collect();
        let transforms = some_transforms();
        let baked = bake_normals(&normals, &transforms).unwrap();
        assert_eq!(baked.len(), normals.len() * transforms.len());

        for (c, m) in transforms.iter().enumerate() {
            let linear = Mat3::from_mat4(*m);
            let sign = linear.determinant().signum();
            for (i, &n) in normals.iter().enumerate().step_by(7) {
                let got = baked[c * normals.len() + i];
                assert!((got.length() - 1.0).abs() < 1e-5);
                // Two tangents of the sphere here, carried to the copy: their cross
                // product is the copy's surface normal, up to the determinant's sign.
                let t1 = n.any_orthonormal_vector();
                let t2 = n.cross(t1);
                let surface = (linear * t1).cross(linear * t2).normalize() * sign;
                assert!(got.dot(surface) > 0.9999, "copy {c} normal {i}: {got} vs {surface}");
            }
        }
        assert_eq!(baked[..normals.len()], normals[..], "copy 0 verbatim");
    }

    /// Timing, on whatever machine runs it:
    /// `cargo test -p sculpt-array --release --target-dir target/w/array -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_of_a_large_bake() {
        // A 1.1 million triangle sphere, 21 copies: about 23 million triangles.
        let sphere = uv_sphere(740, 740, 1.0);
        let copies = 21usize;
        let transforms: Vec<Mat4> = (0..copies)
            .map(|i| Mat4::from_rotation_translation(Quat::from_rotation_y(i as f32), Vec3::X * 3.0 * i as f32))
            .collect();
        let normals: Vec<Vec3> = sphere.positions.iter().map(|p| p.normalize()).collect();

        let start = Instant::now();
        let baked = bake(&sphere, &transforms).unwrap();
        let bake_time = start.elapsed();
        let start = Instant::now();
        let baked_normals = bake_normals(&normals, &transforms).unwrap();
        let normals_time = start.elapsed();
        println!(
            "{} triangles x {copies} copies = {} triangles, {} vertices: bake {bake_time:?}, normals {normals_time:?}",
            sphere.triangle_count(),
            baked.triangle_count(),
            baked.vertex_count()
        );
        assert_eq!(baked.triangle_count(), copies * sphere.triangle_count());
        assert_eq!(baked_normals.len(), baked.vertex_count());
    }
}
