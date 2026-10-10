//! Symmetry as plain data: one operation (`SymmetryOp`) and the list of them an
//! object is sculpted under (`SymmetryGroup`).
//!
//! A stroke is repeated once per element of the group. The elements are
//! orthogonal maps of the object's local space: mirror reflections, rotations
//! about an axis through the origin, or compositions of the two. Orthogonal, so
//! one map moves points, directions and normals alike (a normal's transform is
//! the inverse transpose, which for an orthogonal matrix is the matrix itself),
//! lengths are kept, and the inverse is the transpose.
//!
//! Handedness: nothing here depends on it. The matrices act on coordinates, so
//! a rotation by `step / count` of a turn has the same entries in the Unity app
//! (left-handed) and here (right-handed); only which way round it looks on
//! screen differs, and the set of rotations is the same either way. An op that
//! `reflects()` reverses winding, so a caller mirroring a mesh through it must
//! flip the triangles, exactly as `TriMesh::transform` does for a mirroring
//! matrix.

use std::f64::consts::PI;
use std::fmt;
use std::ops::Index;

use glam::{Mat3, Mat4, Quat, Vec3};
use rayon::prelude::*;

/// One element of a symmetry: an orthogonal linear map, plus a translation
/// applied to points only.
///
/// A map that is a pure per-axis sign flip (every mirror-only element) is
/// flagged `diagonal` and applied as exactly the component-wise product it
/// replaces. Mirrored halves of a mesh are meant to stay bitwise identical, and
/// a full 3x3 multiply, although it rounds to the same values, is not something
/// to lean on. Quarter turns about a coordinate axis come out as exact 0 or
/// +-1 matrices for the same reason, and a half turn about one is itself
/// diagonal.
///
/// A symmetry fixed in the scene rather than through the object's origin
/// carries a translation in the object's local space: `offset`. It is added to
/// points only (`apply_point`); directions and normals take the linear part
/// (`apply`). An op with a zero offset behaves exactly as if it had none.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SymmetryOp {
    /// Rows of the linear part.
    pub row0: Vec3,
    pub row1: Vec3,
    pub row2: Vec3,
    /// Translation applied after the linear part, for points only.
    pub offset: Vec3,
    /// The linear part is a pure per-axis sign flip (the identity included).
    pub diagonal: bool,
}

impl Default for SymmetryOp {
    fn default() -> SymmetryOp {
        SymmetryOp::IDENTITY
    }
}

impl SymmetryOp {
    pub const IDENTITY: SymmetryOp =
        SymmetryOp { row0: Vec3::X, row1: Vec3::Y, row2: Vec3::Z, offset: Vec3::ZERO, diagonal: true };

    /// The per-axis sign flip with signs `sign` (each +1 or -1).
    pub fn from_sign(sign: Vec3) -> SymmetryOp {
        SymmetryOp {
            row0: Vec3::new(sign.x, 0.0, 0.0),
            row1: Vec3::new(0.0, sign.y, 0.0),
            row2: Vec3::new(0.0, 0.0, sign.z),
            offset: Vec3::ZERO,
            diagonal: true,
        }
    }

    /// Rotation by `step / count` of a full turn about `axis` (need not be
    /// normalised); the identity when `count <= 0`, `axis` is zero or the step
    /// is a whole number of turns.
    ///
    /// Built in `f64` and rounded once, so every step is as close to its true
    /// rotation as a float matrix can be, never a power of the one-step matrix
    /// whose error would grow with the step. Values within 1e-12 of 0 or +-1 are
    /// snapped: that makes quarter turns about a coordinate axis exact (cos 90
    /// degrees is 0, not 6e-17), so N = 2 and N = 4 about X, Y or Z are exactly
    /// representable symmetries, like the mirrors.
    pub fn rotation(axis: Vec3, step: i32, count: i32) -> SymmetryOp {
        let (mut ax, mut ay, mut az) = (axis.x as f64, axis.y as f64, axis.z as f64);
        let length = (ax * ax + ay * ay + az * az).sqrt();
        if count <= 0 || length < 1e-12 {
            return SymmetryOp::IDENTITY;
        }
        ax /= length;
        ay /= length;
        az /= length;

        // Reduced to [0, count) so every equivalent step gets the identical matrix.
        let k = step.rem_euclid(count);
        if k == 0 {
            return SymmetryOp::IDENTITY;
        }
        let angle = 2.0 * PI * k as f64 / count as f64;
        let (c, s) = (snap(angle.cos()), snap(angle.sin()));
        let t = 1.0 - c;

        // Rodrigues. Snapped per entry, so an axis-aligned rotation's off-axis
        // entries (whose terms all carry a factor of a zero axis component) stay
        // exactly 0.
        let entry = |v: f64| snap(v) as f32;
        let mut op = SymmetryOp {
            row0: Vec3::new(entry(t * ax * ax + c), entry(t * ax * ay - s * az), entry(t * ax * az + s * ay)),
            row1: Vec3::new(entry(t * ax * ay + s * az), entry(t * ay * ay + c), entry(t * ay * az - s * ax)),
            row2: Vec3::new(entry(t * ax * az - s * ay), entry(t * ay * az + s * ax), entry(t * az * az + c)),
            offset: Vec3::ZERO,
            diagonal: false,
        };
        op.diagonal = op.is_sign_matrix();
        op
    }

    /// The reflection across the plane through the origin with normal `normal`
    /// (need not be normalised); the identity for a zero normal. A plane
    /// perpendicular to a coordinate axis comes out as the exact sign flip.
    pub fn reflection(normal: Vec3) -> SymmetryOp {
        let n = normal.as_dvec3();
        let length = n.length();
        if length < 1e-12 {
            return SymmetryOp::IDENTITY;
        }
        let n = n / length;
        // I - 2 n n^T, built in f64 and snapped like `rotation`.
        let entry = |i: usize, j: usize| snap((i == j) as u8 as f64 - 2.0 * n[i] * n[j]) as f32;
        let mut op = SymmetryOp {
            row0: Vec3::new(entry(0, 0), entry(0, 1), entry(0, 2)),
            row1: Vec3::new(entry(1, 0), entry(1, 1), entry(1, 2)),
            row2: Vec3::new(entry(2, 0), entry(2, 1), entry(2, 2)),
            offset: Vec3::ZERO,
            diagonal: false,
        };
        op.diagonal = op.is_sign_matrix();
        op
    }

    /// The per-axis signs of a diagonal op (the mirror sign it stands for).
    #[inline]
    pub fn sign(&self) -> Vec3 {
        Vec3::new(self.row0.x, self.row1.y, self.row2.z)
    }

    /// Offset is non-zero: the map does not fix the local origin.
    #[inline]
    pub fn has_offset(&self) -> bool {
        self.offset != Vec3::ZERO
    }

    #[inline]
    pub fn is_identity(&self) -> bool {
        self.diagonal && !self.has_offset() && self.row0.x > 0.0 && self.row1.y > 0.0 && self.row2.z > 0.0
    }

    /// True for a map that reverses handedness (an odd number of reflections).
    /// Mirroring a mesh through such a map must also reverse its winding.
    #[inline]
    pub fn reflects(&self) -> bool {
        self.determinant() < 0.0
    }

    pub fn determinant(&self) -> f32 {
        let (r0, r1, r2) = (self.row0, self.row1, self.row2);
        r0.x * (r1.y * r2.z - r1.z * r2.y) - r0.y * (r1.x * r2.z - r1.z * r2.x)
            + r0.z * (r1.x * r2.y - r1.y * r2.x)
    }

    fn is_sign_matrix(&self) -> bool {
        self.row0.y == 0.0
            && self.row0.z == 0.0
            && self.row1.x == 0.0
            && self.row1.z == 0.0
            && self.row2.x == 0.0
            && self.row2.y == 0.0
            && self.row0.x.abs() == 1.0
            && self.row1.y.abs() == 1.0
            && self.row2.z.abs() == 1.0
    }

    /// Maps a local point: the linear part, then the offset.
    #[inline]
    pub fn apply_point(&self, p: Vec3) -> Vec3 {
        if self.has_offset() { self.apply(p) + self.offset } else { self.apply(p) }
    }

    /// The inverse of `apply_point`.
    #[inline]
    pub fn apply_inverse_point(&self, p: Vec3) -> Vec3 {
        if self.has_offset() { self.apply_inverse(p - self.offset) } else { self.apply_inverse(p) }
    }

    /// Maps a local direction or normal: the linear part only. For a point use
    /// `apply_point`; the two agree whenever the op has no offset.
    #[inline]
    pub fn apply(&self, v: Vec3) -> Vec3 {
        if self.diagonal {
            return Vec3::new(v.x * self.row0.x, v.y * self.row1.y, v.z * self.row2.z);
        }
        Vec3::new(self.row0.dot(v), self.row1.dot(v), self.row2.dot(v))
    }

    /// The inverse map: the transpose.
    #[inline]
    pub fn apply_inverse(&self, v: Vec3) -> Vec3 {
        if self.diagonal {
            // A sign flip is its own inverse.
            return Vec3::new(v.x * self.row0.x, v.y * self.row1.y, v.z * self.row2.z);
        }
        Vec3::new(
            self.row0.x * v.x + self.row1.x * v.y + self.row2.x * v.z,
            self.row0.y * v.x + self.row1.y * v.y + self.row2.y * v.z,
            self.row0.z * v.x + self.row1.z * v.y + self.row2.z * v.z,
        )
    }

    /// `a * b`: the map that applies `b` first and then `a`.
    pub fn compose(a: SymmetryOp, b: SymmetryOp) -> SymmetryOp {
        let mut op = if a.diagonal && b.diagonal {
            SymmetryOp::from_sign(a.sign() * b.sign())
        } else {
            // Columns of b, each mapped through a.
            let c0 = a.apply(Vec3::new(b.row0.x, b.row1.x, b.row2.x));
            let c1 = a.apply(Vec3::new(b.row0.y, b.row1.y, b.row2.y));
            let c2 = a.apply(Vec3::new(b.row0.z, b.row1.z, b.row2.z));
            let mut op = SymmetryOp {
                row0: Vec3::new(c0.x, c1.x, c2.x),
                row1: Vec3::new(c0.y, c1.y, c2.y),
                row2: Vec3::new(c0.z, c1.z, c2.z),
                offset: Vec3::ZERO,
                diagonal: false,
            };
            op.diagonal = op.is_sign_matrix();
            op
        };
        // (a*b)(p) = A(B p + tb) + ta.
        if a.has_offset() || b.has_offset() {
            op.offset = a.apply(b.offset) + a.offset;
        }
        op
    }

    /// `world_op`, a map of world space fixing the world origin, as the
    /// equivalent map of an object's LOCAL space, for the object whose rotation
    /// and position are given: `p -> L^-1 world_op L p`.
    ///
    /// The linear part is taken through the rotation alone (orthogonal, as the
    /// type requires); `world_to_local` supplies the exact offset, so the plane
    /// or axis passes through the right local point even on a non-uniformly
    /// scaled object, where only its tilt is approximate. Entries within 1e-6 of
    /// 0 or +-1 are snapped, so an object turned by a quarter turn still gets
    /// exact sign-flip ops.
    pub fn in_object_frame(world_op: SymmetryOp, rotation: Quat, position: Vec3, world_to_local: &Mat4) -> SymmetryOp {
        if world_op.is_identity() {
            return SymmetryOp::IDENTITY;
        }
        let inverse = rotation.inverse();
        // Column j of R^-1 G R is R^-1 G (R e_j).
        let c0 = inverse * world_op.apply(rotation * Vec3::X);
        let c1 = inverse * world_op.apply(rotation * Vec3::Y);
        let c2 = inverse * world_op.apply(rotation * Vec3::Z);
        let mut op = SymmetryOp {
            row0: Vec3::new(snap_f32(c0.x), snap_f32(c1.x), snap_f32(c2.x)),
            row1: Vec3::new(snap_f32(c0.y), snap_f32(c1.y), snap_f32(c2.y)),
            row2: Vec3::new(snap_f32(c0.z), snap_f32(c1.z), snap_f32(c2.z)),
            offset: Vec3::ZERO,
            diagonal: false,
        };
        op.diagonal = op.is_sign_matrix();
        op.offset = world_to_local.transform_vector3(world_op.apply(position) - position);
        op
    }

    /// Largest entry-wise difference, offset included: how two ops that should
    /// be the same element (say a rotation reached two ways) are recognised
    /// despite rounding.
    pub fn max_difference(a: SymmetryOp, b: SymmetryOp) -> f32 {
        let linear = (a.row0 - b.row0).abs().max_element().max((a.row1 - b.row1).abs().max_element());
        let linear = linear.max((a.row2 - b.row2).abs().max_element());
        linear.max((a.offset - b.offset).abs().max_element())
    }

    /// The linear part as a matrix (columns are the images of the axes).
    pub fn to_mat3(&self) -> Mat3 {
        Mat3::from_cols(
            Vec3::new(self.row0.x, self.row1.x, self.row2.x),
            Vec3::new(self.row0.y, self.row1.y, self.row2.y),
            Vec3::new(self.row0.z, self.row1.z, self.row2.z),
        )
    }

    /// The whole map on points, as a matrix.
    pub fn to_mat4(&self) -> Mat4 {
        Mat4::from_mat3_translation(self.to_mat3(), self.offset)
    }
}

impl fmt::Display for SymmetryOp {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.diagonal {
            write!(f, "Sign{}", self.sign())?;
        } else {
            write!(f, "[{} {} {}]", self.row0, self.row1, self.row2)?;
        }
        if self.has_offset() {
            write!(f, " + {}", self.offset)?;
        }
        Ok(())
    }
}

impl std::ops::Mul for SymmetryOp {
    type Output = SymmetryOp;

    /// `a * b` applies `b` first, as with matrices.
    fn mul(self, rhs: SymmetryOp) -> SymmetryOp {
        SymmetryOp::compose(self, rhs)
    }
}

fn snap(v: f64) -> f64 {
    if v.abs() < 1e-12 {
        0.0
    } else if (v - 1.0).abs() < 1e-12 {
        1.0
    } else if (v + 1.0).abs() < 1e-12 {
        -1.0
    } else {
        v
    }
}

fn snap_f32(v: f32) -> f32 {
    if v.abs() < 1e-6 {
        0.0
    } else if (v - 1.0).abs() < 1e-6 {
        1.0
    } else if (v + 1.0).abs() < 1e-6 {
        -1.0
    } else {
        v
    }
}

/// Every symmetry an object is sculpted under, as one list of `SymmetryOp`s: the
/// mirror reflections that are switched on, times N rotations about the radial
/// axis, duplicates removed. A brush dab is applied once per element.
///
/// Element 0 is always the identity, the stroke the user actually made.
/// Mirror-only lists come out in flip-mask order (X = 1, Y = 2, Z = 4, counting
/// up over the enabled axes), the enumeration the order-symmetric dab walk used
/// before rotations existed, so mirrored sculpting walks the same sequence it
/// always did.
///
/// Combining works as a real group whenever each enabled mirror plane either
/// contains the radial axis or is perpendicular to it, which is always true for
/// an X, Y or Z axis. Mirror X with radial about Y, for instance, is the
/// dihedral group of 2N elements; mirror X and Z with an even N about Y adds
/// nothing mirror X had not (X then Z IS the half turn), which is why
/// duplicates are removed rather than applied twice. A custom axis tilted
/// against an enabled mirror plane generates an infinite group; the list is
/// then just the product, `is_group()` is false, and the dab walk falls back to
/// applying it in plain sequence (see `interacting_subgroup`).
#[derive(Clone, Debug)]
pub struct SymmetryGroup {
    ops: Vec<SymmetryOp>,
    /// `[a * len + b]` = index of `a * b`, or `NO_ELEMENT` when not closed.
    product: Vec<u16>,
    radial_count: u32,
    is_group: bool,
}

const NO_ELEMENT: u16 = u16::MAX;

/// Two ops closer than this entry for entry are the same element reached two ways.
const MATCH_TOLERANCE: f32 = 1e-4;

impl SymmetryGroup {
    pub const MIN_RADIAL_COUNT: u32 = 2;
    pub const MAX_RADIAL_COUNT: u32 = 32;
    /// The most elements a group can have: every mirror combination for every rotation.
    pub const MAX_ELEMENTS: usize = Self::MAX_RADIAL_COUNT as usize * 8;

    /// One stroke, no repeats: what an object without symmetry uses.
    pub fn trivial() -> SymmetryGroup {
        SymmetryGroup::build(false, false, false, 1, Vec3::Y)
    }

    /// `radial_count` below 2 means no radial symmetry; it is clamped to
    /// `MAX_RADIAL_COUNT`. `radial_axis` is a local-space direction through the
    /// object's origin.
    pub fn build(mirror_x: bool, mirror_y: bool, mirror_z: bool, radial_count: u32, radial_axis: Vec3) -> SymmetryGroup {
        // Spelled out rather than `if n < MIN { 1 } else { n.min(MAX) }`: rustc 1.99.0
        // miscompiles that form for u32 at opt-level 1 and up, turning 0 into 0.
        let n = if radial_count < Self::MIN_RADIAL_COUNT {
            1
        } else if radial_count > Self::MAX_RADIAL_COUNT {
            Self::MAX_RADIAL_COUNT
        } else {
            radial_count
        };
        let active = mirror_x as u32 | (mirror_y as u32) << 1 | (mirror_z as u32) << 2;
        let mut ops: Vec<SymmetryOp> = Vec::with_capacity(n as usize * 8);
        for k in 0..n {
            let rotation = SymmetryOp::rotation(radial_axis, k as i32, n as i32);
            for mask in 0..8u32 {
                if mask & !active != 0 {
                    continue;
                }
                let flip = SymmetryOp::from_sign(Self::sign_of_flip_mask(mask));
                let op = SymmetryOp::compose(flip, rotation);
                if index_in(&ops, op).is_none() {
                    ops.push(op);
                }
            }
        }
        SymmetryGroup::from_ops(ops, n)
    }

    fn from_ops(ops: Vec<SymmetryOp>, radial_count: u32) -> SymmetryGroup {
        let n = ops.len();
        debug_assert!(n <= Self::MAX_ELEMENTS);
        let mut product = vec![NO_ELEMENT; n * n];
        let fill_row = |(a, row): (usize, &mut [u16])| {
            for (b, slot) in row.iter_mut().enumerate() {
                let composed = SymmetryOp::compose(ops[a], ops[b]);
                if let Some(index) = index_in(&ops, composed) {
                    *slot = index as u16;
                }
            }
        };
        // 256 elements make 65,536 products, each searched for in the list.
        if n > 16 {
            product.par_chunks_mut(n).enumerate().for_each(fill_row);
        } else {
            product.chunks_mut(n).enumerate().for_each(fill_row);
        }
        let is_group = product.iter().all(|&p| p != NO_ELEMENT);
        SymmetryGroup { ops, product, radial_count, is_group }
    }

    /// This group, built about the WORLD origin and axes, as the same symmetry
    /// of an object's local space (see `SymmetryOp::in_object_frame`).
    /// Conjugation keeps the product table, so the result walks exactly like
    /// this one.
    pub fn in_object_frame(&self, rotation: Quat, position: Vec3, world_to_local: &Mat4) -> SymmetryGroup {
        let ops = self.ops.iter().map(|op| SymmetryOp::in_object_frame(*op, rotation, position, world_to_local)).collect();
        SymmetryGroup::from_ops(ops, self.radial_count)
    }

    /// Signs of the flip mask: bit 0 flips X, bit 1 Y, bit 2 Z.
    pub fn sign_of_flip_mask(mask: u32) -> Vec3 {
        let flip = |bit: u32| if mask & bit != 0 { -1.0 } else { 1.0 };
        Vec3::new(flip(1), flip(2), flip(4))
    }

    #[inline]
    pub fn len(&self) -> usize {
        self.ops.len()
    }

    /// Never true: element 0, the identity, is always there.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    #[inline]
    pub fn ops(&self) -> &[SymmetryOp] {
        &self.ops
    }

    pub fn iter(&self) -> std::slice::Iter<'_, SymmetryOp> {
        self.ops.iter()
    }

    /// Closed under composition, so the order-symmetric walk can average over it.
    #[inline]
    pub fn is_group(&self) -> bool {
        self.is_group
    }

    /// Rotations about the radial axis (1 when radial symmetry is off).
    #[inline]
    pub fn radial_count(&self) -> u32 {
        self.radial_count
    }

    /// Index of `a * b`, or `None` when the list is not closed and the product
    /// falls outside it.
    #[inline]
    pub fn product(&self, a: usize, b: usize) -> Option<usize> {
        match self.product[a * self.ops.len() + b] {
            NO_ELEMENT => None,
            index => Some(index as usize),
        }
    }

    /// Index of the element matching `op` to within rounding.
    pub fn index_of(&self, op: SymmetryOp) -> Option<usize> {
        index_in(&self.ops, op)
    }

    /// The subgroup generated by every element that maps `point` to within
    /// `distance` of itself, as ascending indices (identity first), written into
    /// `subgroup`. Those are the copies of a dab at `point` that can reach each
    /// other's footprint: `g p` and `g' p` are `|p - g^-1 g' p|` apart, so two
    /// copies interact exactly when `g^-1 g'` is one of the generators, and
    /// copies in different cosets of this subgroup never do.
    ///
    /// Just the identity for a list that is not a group: nothing can be averaged
    /// over then. Takes the output list from the caller so a stroke's per-dab
    /// walk allocates nothing.
    pub fn interacting_subgroup(&self, point: Vec3, distance: f32, subgroup: &mut Vec<usize>) {
        subgroup.clear();
        subgroup.push(0);
        if !self.is_group || self.ops.len() == 1 {
            return;
        }

        let limit_sqr = distance * distance;
        let mut member = [false; Self::MAX_ELEMENTS];
        member[0] = true;
        let mut generators = 0;
        for (g, op) in self.ops.iter().enumerate().skip(1) {
            if (point - op.apply_point(point)).length_squared() >= limit_sqr {
                continue;
            }
            member[g] = true;
            subgroup.push(g);
            generators += 1;
        }
        if generators == 0 {
            return;
        }

        // Closure: every element found so far times every generator, until
        // nothing new appears. The generators are subgroup[1..=generators],
        // which stay put as the list grows.
        let mut i = 0;
        while i < subgroup.len() {
            for j in 1..=generators {
                let Some(product) = self.product(subgroup[i], subgroup[j]) else { continue };
                if !member[product] {
                    member[product] = true;
                    subgroup.push(product);
                }
            }
            i += 1;
        }
        subgroup.sort_unstable();
    }

    /// The left cosets of `subgroup` (ascending indices, identity first), each
    /// named by its lowest-index element, ascending, written into
    /// `representatives`. With a mirror-only list that representative is the
    /// coset's "far" flips alone, the enumeration the dab walk has always used.
    pub fn coset_representatives(&self, subgroup: &[usize], representatives: &mut Vec<usize>) {
        representatives.clear();
        let mut covered = [false; Self::MAX_ELEMENTS];
        for g in 0..self.ops.len() {
            if covered[g] {
                continue;
            }
            representatives.push(g);
            for &h in subgroup {
                if let Some(member) = self.product(g, h) {
                    covered[member] = true;
                }
            }
        }
    }
}

impl Default for SymmetryGroup {
    fn default() -> SymmetryGroup {
        SymmetryGroup::trivial()
    }
}

impl Index<usize> for SymmetryGroup {
    type Output = SymmetryOp;

    #[inline]
    fn index(&self, index: usize) -> &SymmetryOp {
        &self.ops[index]
    }
}

impl<'a> IntoIterator for &'a SymmetryGroup {
    type Item = &'a SymmetryOp;
    type IntoIter = std::slice::Iter<'a, SymmetryOp>;

    fn into_iter(self) -> Self::IntoIter {
        self.ops.iter()
    }
}

fn index_in(ops: &[SymmetryOp], op: SymmetryOp) -> Option<usize> {
    ops.iter().position(|&o| SymmetryOp::max_difference(o, op) <= MATCH_TOLERANCE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_only_groups_are_flip_masks_in_order() {
        let g = SymmetryGroup::build(true, false, true, 1, Vec3::Y);
        assert_eq!(g.len(), 4);
        assert!(g.is_group());
        for (i, mask) in [0u32, 1, 4, 5].into_iter().enumerate() {
            assert!(g[i].diagonal, "element {i}");
            assert_eq!(g[i].sign(), SymmetryGroup::sign_of_flip_mask(mask), "element {i}");
        }
    }

    #[test]
    fn radial_group_has_n_rotations_and_is_closed() {
        for n in [2u32, 3, 5, 6, 8, 12, 32] {
            let g = SymmetryGroup::build(false, false, false, n, Vec3::Y);
            assert_eq!(g.len(), n as usize);
            assert!(g.is_group());
            assert!(g[0].is_identity());
            for i in 0..n as usize {
                assert!(!g[i].reflects(), "rotation {i} reverses handedness");
                assert!((g[i].determinant() - 1.0).abs() < 1e-5);
                let p = g[i].apply(Vec3::new(0.3, 0.7, -0.2));
                assert!((p.y - 0.7).abs() < 1e-6, "a rotation about Y must keep the height");
                assert!(((p.x * p.x + p.z * p.z).sqrt() - (0.3f32 * 0.3 + 0.2 * 0.2).sqrt()).abs() < 1e-6);
            }
        }
    }

    /// Quarter turns about a coordinate axis are exact, so N = 2 and 4 are as exact as mirrors.
    #[test]
    fn quarter_turns_about_coordinate_axes_are_exact() {
        for dir in [Vec3::X, Vec3::Y, Vec3::Z] {
            let g = SymmetryGroup::build(false, false, false, 4, dir);
            let p = Vec3::new(0.123, -0.456, 0.789);
            for i in 0..4 {
                let q = g[i].apply(p);
                // Every coordinate of a quarter-turned point is +-one of the originals, bit for bit.
                for c in q.to_array() {
                    assert!(c.abs() == 0.123 || c.abs() == 0.456 || c.abs() == 0.789, "axis {dir} step {i}: {q:?}");
                }
            }
            assert!(g[2].diagonal, "the half turn is a pure sign flip");
        }
    }

    /// Mirror X with radial about Y is the dihedral group; adding Z adds nothing for even N
    /// (X then Z is the half turn), so it must not add duplicate elements that would apply a
    /// stroke twice in the same place.
    #[test]
    fn dihedral_groups_are_deduplicated() {
        for n in [3u32, 4, 6] {
            let n_us = n as usize;
            let x = SymmetryGroup::build(true, false, false, n, Vec3::Y);
            assert_eq!(x.len(), 2 * n_us);
            assert!(x.is_group());

            let xz = SymmetryGroup::build(true, false, true, n, Vec3::Y);
            assert_eq!(xz.len(), if n % 2 == 0 { 2 * n_us } else { 4 * n_us });
            assert!(xz.is_group());

            let xy = SymmetryGroup::build(true, true, false, n, Vec3::Y);
            assert_eq!(xy.len(), 4 * n_us, "Y is perpendicular to the axis: a new element per rotation");
            assert!(xy.is_group());
        }
    }

    /// A custom axis tilted against a mirror plane generates an infinite group: the list is the
    /// plain product and says it is not a group, so the dab walk falls back to plain order.
    #[test]
    fn tilted_custom_axis_with_mirror_is_not_a_group() {
        let axis = Vec3::new(0.3, 1.0, 0.2);
        let g = SymmetryGroup::build(true, false, false, 5, axis);
        assert_eq!(g.len(), 10);
        assert!(!g.is_group());

        let mut subgroup = Vec::new();
        g.interacting_subgroup(Vec3::ZERO, 1.0, &mut subgroup);
        assert_eq!(subgroup, [0]);

        // Without the mirror a custom axis is an ordinary cyclic group.
        assert!(SymmetryGroup::build(false, false, false, 5, axis).is_group());
    }

    /// The dab walk's planning: copies that can meet form a subgroup, and the cosets partition
    /// the group.
    #[test]
    fn interacting_subgroup_and_cosets() {
        let g = SymmetryGroup::build(true, false, false, 6, Vec3::Y); // D6, 12 elements
        let mut h = Vec::new();
        let mut reps = Vec::new();

        // Far from the axis and off the X plane (at 30 degrees) but ON a mirror plane of the
        // dihedral group: the copy reflected across it lands on the dab itself.
        let on_reflection_plane = Vec3::new((std::f32::consts::PI / 6.0).cos(), 0.0, (std::f32::consts::PI / 6.0).sin());
        g.interacting_subgroup(on_reflection_plane, 0.05, &mut h);
        assert_eq!(h.len(), 2, "one reflection plane through the dab: the pair");
        g.coset_representatives(&h, &mut reps);
        assert_eq!(reps.len(), 6);

        g.interacting_subgroup(Vec3::new(0.01, 0.5, 0.005), 0.1, &mut h);
        assert_eq!(h.len(), 12, "next to the axis every copy meets every other");
        g.coset_representatives(&h, &mut reps);
        assert_eq!(reps, [0]);

        g.interacting_subgroup(Vec3::new(0.8, 0.0, 0.3), 0.05, &mut h);
        assert_eq!(h, [0], "a generic point far from the axis meets nothing");
    }

    #[test]
    fn identity_and_trivial_group() {
        let g = SymmetryGroup::trivial();
        assert_eq!(g.len(), 1);
        assert_eq!(g.radial_count(), 1);
        assert!(g.is_group() && g[0].is_identity());
        assert_eq!(g[0], SymmetryOp::IDENTITY);
        assert_eq!(g.product(0, 0), Some(0));
        let p = Vec3::new(1.0, -2.0, 3.0);
        assert_eq!(SymmetryOp::IDENTITY.apply_point(p), p);
    }

    #[test]
    fn radial_count_is_clamped_and_below_two_is_off() {
        assert_eq!(SymmetryGroup::build(false, false, false, 0, Vec3::Y).len(), 1);
        assert_eq!(SymmetryGroup::build(false, false, false, 1, Vec3::Y).len(), 1);
        let big = SymmetryGroup::build(true, false, false, 999, Vec3::Y);
        assert_eq!(big.radial_count(), SymmetryGroup::MAX_RADIAL_COUNT);
        assert_eq!(big.len(), 2 * SymmetryGroup::MAX_RADIAL_COUNT as usize);
    }

    /// The largest group there is, every mirror with 32 rotations, fits the fixed scratch arrays
    /// and is closed.
    #[test]
    fn the_largest_group_is_closed_and_fits() {
        let g = SymmetryGroup::build(true, true, true, 32, Vec3::Y);
        assert_eq!(g.len(), 128, "mirror X and Z duplicate the half turn; Y is a new element per rotation");
        assert!(g.is_group());
        let mut h = Vec::new();
        let mut reps = Vec::new();
        g.interacting_subgroup(Vec3::ZERO, 1.0, &mut h);
        assert_eq!(h.len(), g.len());
        g.coset_representatives(&h, &mut reps);
        assert_eq!(reps, [0]);
        assert!(g.len() <= SymmetryGroup::MAX_ELEMENTS);
    }

    #[test]
    fn inverse_undoes_the_map_for_points_and_directions() {
        let axis = Vec3::new(0.3, 1.0, -0.4);
        let mut op = SymmetryOp::compose(SymmetryOp::from_sign(Vec3::new(1.0, -1.0, 1.0)), SymmetryOp::rotation(axis, 2, 7));
        op.offset = Vec3::new(0.5, -0.25, 2.0);
        let p = Vec3::new(0.3, 0.9, -1.7);
        assert!(op.apply_inverse_point(op.apply_point(p)).distance(p) < 1e-5);
        assert!(op.apply_inverse(op.apply(p)).distance(p) < 1e-5);
        // Offsets move points, never directions.
        assert!(op.apply_point(p).distance(op.apply(p) + op.offset) < 1e-6);
    }

    #[test]
    fn compose_matches_applying_one_after_the_other() {
        let a = SymmetryOp::rotation(Vec3::new(1.0, 2.0, 3.0), 1, 5);
        let mut b = SymmetryOp::compose(SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0)), SymmetryOp::rotation(Vec3::Y, 1, 3));
        b.offset = Vec3::new(1.0, 0.0, -2.0);
        let mut a = a;
        a.offset = Vec3::new(-0.5, 0.25, 0.0);
        let p = Vec3::new(0.7, -0.2, 0.4);
        let ab = SymmetryOp::compose(a, b);
        assert!(ab.apply_point(p).distance(a.apply_point(b.apply_point(p))) < 1e-5);
        assert!(ab.apply(p).distance(a.apply(b.apply(p))) < 1e-5);
        // The matrix form agrees with the maps.
        assert!(ab.to_mat4().transform_point3(p).distance(ab.apply_point(p)) < 1e-5);
        assert!((ab.to_mat3() * p).distance(ab.apply(p)) < 1e-5);
    }

    #[test]
    fn reflection_flips_the_normal_and_fixes_the_plane() {
        let n = Vec3::new(1.0, 2.0, -2.0);
        let r = SymmetryOp::reflection(n);
        assert!(r.reflects() && !r.diagonal);
        assert!(r.apply(n).distance(-n) < 1e-6);
        let in_plane = n.cross(Vec3::Y);
        assert!(r.apply(in_plane).distance(in_plane) < 1e-6);
        assert!(SymmetryOp::compose(r, r).apply(Vec3::new(1.0, 2.0, 3.0)).distance(Vec3::new(1.0, 2.0, 3.0)) < 1e-5);
        // A coordinate plane is the exact sign flip.
        let x = SymmetryOp::reflection(Vec3::new(5.0, 0.0, 0.0));
        assert!(x.diagonal && x.sign() == Vec3::new(-1.0, 1.0, 1.0));
        assert!(SymmetryOp::reflection(Vec3::ZERO).is_identity());
    }

    #[test]
    fn rotation_steps_wrap_and_zero_axis_is_identity() {
        let a = SymmetryOp::rotation(Vec3::Y, 1, 6);
        assert_eq!(a, SymmetryOp::rotation(Vec3::Y, 7, 6));
        assert_eq!(a, SymmetryOp::rotation(Vec3::new(0.0, 5.0, 0.0), -5, 6));
        assert!(SymmetryOp::rotation(Vec3::Y, 6, 6).is_identity());
        assert!(SymmetryOp::rotation(Vec3::ZERO, 1, 6).is_identity());
        assert!(SymmetryOp::rotation(Vec3::Y, 1, 0).is_identity());
    }

    fn object_frame(rotation: Quat, position: Vec3) -> (Mat4, Mat4) {
        let local_to_world = Mat4::from_rotation_translation(rotation, position);
        (local_to_world, local_to_world.inverse())
    }

    /// A world symmetry seen from a moved and turned object must do to local points what the
    /// world map does to the same points in the world.
    #[test]
    fn in_object_frame_is_the_world_map_conjugated() {
        let rotation = Quat::from_axis_angle(Vec3::new(0.2, 1.0, -0.3).normalize(), 0.9);
        let position = Vec3::new(1.5, -0.5, 2.0);
        let (to_world, to_local) = object_frame(rotation, position);
        let world = SymmetryGroup::build(true, false, true, 3, Vec3::Y);
        let local = world.in_object_frame(rotation, position, &to_local);
        assert_eq!(local.len(), world.len());
        assert_eq!(local.is_group(), world.is_group());

        let p = Vec3::new(0.4, 0.3, -0.8);
        for (w, l) in world.iter().zip(local.iter()) {
            let expected = to_local.transform_point3(w.apply_point(to_world.transform_point3(p)));
            assert!(l.apply_point(p).distance(expected) < 1e-4, "{l} gives {} not {expected}", l.apply_point(p));
            // The world map fixes the world origin, so its local form moves only by the offset.
            let expected_dir = rotation.inverse() * w.apply(rotation * p);
            assert!(l.apply(p).distance(expected_dir) < 1e-5);
        }
    }

    #[test]
    fn in_object_frame_of_an_unturned_object_at_the_origin_changes_nothing() {
        let world = SymmetryGroup::build(true, true, false, 4, Vec3::Y);
        let local = world.in_object_frame(Quat::IDENTITY, Vec3::ZERO, &Mat4::IDENTITY);
        for (w, l) in world.iter().zip(local.iter()) {
            assert_eq!(w, l);
        }
    }

    /// A quarter turn of the object turns the mirror X plane into the mirror Z plane, and the
    /// result is still the exact sign flip.
    #[test]
    fn in_object_frame_snaps_quarter_turns_to_sign_flips() {
        let rotation = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2);
        let world = SymmetryGroup::build(true, false, false, 1, Vec3::Y);
        let local = world.in_object_frame(rotation, Vec3::ZERO, &Mat4::from_quat(rotation.inverse()));
        assert!(local[1].diagonal);
        assert_eq!(local[1].sign(), Vec3::new(1.0, 1.0, -1.0));
        assert!(!local[1].has_offset());
    }

    #[test]
    fn a_moved_object_gets_a_different_world_offset() {
        let world = SymmetryGroup::build(true, false, false, 1, Vec3::Y);
        let at = |x: f32| {
            let position = Vec3::new(x, 0.0, 0.0);
            let (_, to_local) = object_frame(Quat::IDENTITY, position);
            world.in_object_frame(Quat::IDENTITY, position, &to_local)[1]
        };
        assert_ne!(at(0.0).offset, at(0.25).offset);
        // Mirroring across the world YZ plane an object standing 0.25 to its right: the plane is
        // 0.25 to the object's left, so local x = -0.25 stays put.
        assert!(at(0.25).apply_point(Vec3::new(-0.25, 1.0, 0.0)).distance(Vec3::new(-0.25, 1.0, 0.0)) < 1e-6);
    }

    #[test]
    fn display_names_the_signs_and_the_offset() {
        let mut op = SymmetryOp::from_sign(Vec3::new(-1.0, 1.0, 1.0));
        assert!(op.to_string().starts_with("Sign"));
        op.offset = Vec3::X;
        assert!(op.to_string().contains(" + "));
    }

    /// Building the biggest groups, which tabulate every product; run with `--release --ignored --nocapture`.
    #[test]
    #[ignore]
    fn timing_of_the_biggest_groups() {
        for (label, axis) in [("axis Y, 128 elements", Vec3::Y), ("tilted axis, 256 elements", Vec3::new(0.3, 1.0, 0.2))] {
            let t = std::time::Instant::now();
            let g = SymmetryGroup::build(true, true, true, 32, axis);
            println!("{label}: {} elements, group {}, built in {:?}", g.len(), g.is_group(), t.elapsed());
        }
    }
}
