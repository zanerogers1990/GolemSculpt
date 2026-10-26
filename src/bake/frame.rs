//! The low model's tangent frame: the one a game engine reads a normal map in.
//!
//! A tangent-space normal map says how the sculpt's normal leans away from
//! the low model's own, measured along the low surface's U and V directions.
//! Red is the lean along U (the tangent), green along V (the bitangent), blue
//! straight out, each stored as `n * 0.5 + 0.5`, so a texel with no lean is
//! (128, 128, 255). Green leans towards +V, the OpenGL way that Blender and
//! Unity read; DirectX tools want green turned over.
//!
//! For the map to read back right, the baker has to measure in the very
//! frame the engine will rebuild. That frame is MikkTSpace, which Blender,
//! Unity, Unreal, Substance and xNormal share; `bevy_mikktspace` is a
//! byte-for-byte port of its reference code. It hands back a tangent and a
//! sign for every triangle corner, worked out from the corners' positions,
//! normals and UVs, so it has to be given the model as it will be exported
//! (`Low` in bake.rs takes care of that).
//!
//! Across a triangle the frame is put together as the engine's pixel shader
//! does: the corner normals and tangents are blended by the texel's weights
//! and not made unit length, and the bitangent is `sign * normal x tangent`,
//! the sign taken from the blended corner signs (as Blender's baker takes
//! it). The sculpt's normal goes into that frame by the exact inverse of the
//! shader's `normalize(x * T + y * B + z * N)`, the sampler formula given in
//! mikktspace.h, so a frame that is not quite square still reads back to the
//! normal it was baked from.
//!
//! The sign is what carries a mirrored UV island: there U runs the other way
//! round the face, MikkTSpace says so with a sign of -1, and the bitangent
//! turns with it, so green still means +V.

use bevy_mikktspace::{Geometry, StdOps, TangentSpace, generate_tangents};
use glam::{Vec2, Vec3, Vec4};

/// The corners of the low model as MikkTSpace reads them, and the tangents it
/// writes back.
struct Corners<'a> {
    positions: &'a [Vec3],
    normals: &'a [Vec3],
    uvs: &'a [Vec2],
    tangents: Vec<Vec4>,
}

impl Geometry for Corners<'_> {
    fn num_faces(&self) -> usize {
        self.positions.len() / 3
    }

    fn num_vertices_of_face(&self, _face: usize) -> usize {
        3
    }

    fn position(&self, face: usize, vert: usize) -> [f32; 3] {
        self.positions[face * 3 + vert].to_array()
    }

    fn normal(&self, face: usize, vert: usize) -> [f32; 3] {
        self.normals[face * 3 + vert].to_array()
    }

    fn tex_coord(&self, face: usize, vert: usize) -> [f32; 2] {
        self.uvs[face * 3 + vert].to_array()
    }

    fn set_tangent(&mut self, tangent: Option<TangentSpace>, face: usize, vert: usize) {
        self.tangents[face * 3 + vert] = tangent.map_or(Vec4::ZERO, |t| Vec4::from_array(t.tangent_encoded()));
    }
}

/// The MikkTSpace tangent of every triangle corner (`t * 3 + c`), with the
/// bitangent's sign in `w`. The arrays have one entry per corner.
pub(super) fn tangents(positions: &[Vec3], normals: &[Vec3], uvs: &[Vec2]) -> Vec<Vec4> {
    let mut corners = Corners { positions, normals, uvs, tangents: vec![Vec4::ZERO; positions.len()] };
    // The error type has no values: generation cannot fail.
    let _ = generate_tangents::<_, StdOps>(&mut corners);
    let mut tangents = corners.tangents;
    // A corner left without a tangent (a face with no area in the picture and
    // no neighbour to borrow from, which covers no texel anyway) takes any
    // direction across its normal, so nothing downstream sees a zero.
    for (t, n) in tangents.iter_mut().zip(normals) {
        if t.truncate().length_squared() < 1e-20 {
            let n = n.normalize_or(Vec3::Z);
            *t = n.any_orthonormal_pair().0.extend(1.0);
        }
    }
    tangents
}

/// The tangent frame at one texel, unnormalised as the shader has it.
pub(super) struct Frame {
    t: Vec3,
    b: Vec3,
    n: Vec3,
}

impl Frame {
    /// The frame at `weights` in a triangle with these corner normals and tangents.
    pub(super) fn at(normals: [Vec3; 3], tangents: [Vec4; 3], weights: [f32; 3]) -> Frame {
        let n = normals[0] * weights[0] + normals[1] * weights[1] + normals[2] * weights[2];
        let t = tangents[0] * weights[0] + tangents[1] * weights[1] + tangents[2] * weights[2];
        let sign = if t.w < 0.0 { -1.0 } else { 1.0 };
        let t = t.truncate();
        Frame { t, b: sign * n.cross(t), n }
    }

    /// `normal` (a direction in the scene) measured in this frame, unit
    /// length: what the shader's `normalize(x * T + y * B + z * N)` turns back
    /// into `normal`.
    pub(super) fn to_tangent(&self, normal: Vec3) -> Vec3 {
        let (t, b, n) = (self.t, self.b, self.n);
        let rows = [b.cross(n), n.cross(t), t.cross(b)];
        let sign = if t.dot(rows[0]) < 0.0 { -1.0 } else { 1.0 };
        let v = Vec3::new(normal.dot(rows[0]), normal.dot(rows[1]), normal.dot(rows[2])) * sign;
        v.normalize_or(Vec3::Z)
    }

    /// The way back, as the engine's shader goes: a tangent-space normal as a
    /// direction in the scene.
    #[cfg(test)]
    pub(super) fn to_world(&self, tangent: Vec3) -> Vec3 {
        (self.t * tangent.x + self.b * tangent.y + self.n * tangent.z).normalize_or_zero()
    }
}

/// A unit tangent-space normal as the three bytes of a normal-map texel.
pub(super) fn encode(n: Vec3) -> [u8; 3] {
    n.to_array().map(|c| ((c * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8)
}

/// A normal-map texel back to the unit normal it holds.
#[cfg(test)]
pub(super) fn decode(texel: [u8; 3]) -> Vec3 {
    Vec3::from_array(texel.map(|c| c as f32 / 255.0 * 2.0 - 1.0)).normalize_or_zero()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_skewed_frame_reads_back_to_the_normal_it_was_given() {
        // Corner tangents not square to their normals, of different lengths,
        // one corner with the other sign: the inverse still undoes the shader.
        let normals = [Vec3::new(0.1, 0.2, 1.0).normalize(), Vec3::Z, Vec3::new(-0.3, 0.0, 1.0).normalize()];
        let tangents = [Vec4::new(1.0, 0.3, 0.1, 1.0), Vec4::new(0.8, -0.2, 0.0, 1.0), Vec4::new(1.2, 0.1, 0.2, -1.0)];
        let frame = Frame::at(normals, tangents, [0.5, 0.3, 0.2]);
        for k in 0..50 {
            let a = k as f32 * 0.37;
            let want = Vec3::new(a.cos() * 0.6, (a * 1.3).sin() * 0.6, 0.7).normalize();
            let back = frame.to_world(frame.to_tangent(want));
            assert!(back.distance(want) < 1e-5, "{back} against {want}");
        }
    }

    #[test]
    fn a_flat_normal_is_the_usual_lilac() {
        assert_eq!(encode(Vec3::Z), [128, 128, 255]);
        assert!(decode([128, 128, 255]).distance(Vec3::Z) < 0.01);
    }

    #[test]
    fn mikktspace_tangents_follow_u_and_their_sign_follows_mirroring() {
        // A square in the XY plane facing +Z, U along +X and V along +Y.
        let p =
            [Vec3::new(0.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0), Vec3::new(1.0, 1.0, 0.0), Vec3::new(0.0, 1.0, 0.0)];
        let positions = [p[0], p[1], p[2], p[0], p[2], p[3]];
        let normals = [Vec3::Z; 6];
        let uv = |p: Vec3| Vec2::new(p.x, p.y);
        let uvs: Vec<Vec2> = positions.iter().map(|&p| uv(p)).collect();
        for t in tangents(&positions, &normals, &uvs) {
            assert!(t.truncate().distance(Vec3::X) < 1e-5 && t.w > 0.0, "{t}");
        }
        // U turned round: the tangent runs along -X and the sign goes negative.
        let mirrored: Vec<Vec2> = uvs.iter().map(|uv| Vec2::new(1.0 - uv.x, uv.y)).collect();
        for t in tangents(&positions, &normals, &mirrored) {
            assert!(t.truncate().distance(-Vec3::X) < 1e-5 && t.w < 0.0, "{t}");
        }
    }
}
