//! An indexed triangle mesh as plain data, and an axis-aligned box.

use glam::{Mat4, Vec3};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: Vec3,
    pub max: Vec3,
}

impl Aabb {
    /// The box that holds nothing; growing it by a point gives that point.
    pub const EMPTY: Aabb = Aabb { min: Vec3::splat(f32::INFINITY), max: Vec3::splat(f32::NEG_INFINITY) };

    pub fn new(min: Vec3, max: Vec3) -> Aabb {
        Aabb { min, max }
    }

    pub fn of(points: &[Vec3]) -> Aabb {
        points.iter().fold(Aabb::EMPTY, |mut b, &p| {
            b.grow(p);
            b
        })
    }

    #[inline]
    pub fn grow(&mut self, p: Vec3) {
        self.min = self.min.min(p);
        self.max = self.max.max(p);
    }

    #[inline]
    pub fn union(a: Aabb, b: Aabb) -> Aabb {
        Aabb { min: a.min.min(b.min), max: a.max.max(b.max) }
    }

    /// The box pushed out by `by` on every side.
    #[inline]
    pub fn expanded(&self, by: f32) -> Aabb {
        Aabb { min: self.min - Vec3::splat(by), max: self.max + Vec3::splat(by) }
    }

    #[inline]
    pub fn center(&self) -> Vec3 {
        (self.min + self.max) * 0.5
    }

    #[inline]
    pub fn size(&self) -> Vec3 {
        (self.max - self.min).max(Vec3::ZERO)
    }

    #[inline]
    pub fn is_empty(&self) -> bool {
        self.min.cmpgt(self.max).any()
    }

    #[inline]
    pub fn contains(&self, p: Vec3) -> bool {
        p.cmpge(self.min).all() && p.cmple(self.max).all()
    }

    #[inline]
    pub fn overlaps(&self, other: &Aabb) -> bool {
        self.min.cmple(other.max).all() && other.min.cmple(self.max).all()
    }
}

/// Triangles as shared vertices plus three indices each. Winding is
/// counter-clockwise seen from outside, as everywhere in the app.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TriMesh {
    pub positions: Vec<Vec3>,
    pub indices: Vec<u32>,
}

impl TriMesh {
    pub fn new(positions: Vec<Vec3>, indices: Vec<u32>) -> TriMesh {
        TriMesh { positions, indices }
    }

    #[inline]
    pub fn vertex_count(&self) -> usize {
        self.positions.len()
    }

    #[inline]
    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }

    pub fn is_empty(&self) -> bool {
        self.indices.is_empty()
    }

    /// The corners of triangle `t`.
    #[inline]
    pub fn triangle(&self, t: usize) -> [Vec3; 3] {
        let i = &self.indices[t * 3..t * 3 + 3];
        [self.positions[i[0] as usize], self.positions[i[1] as usize], self.positions[i[2] as usize]]
    }

    pub fn bounds(&self) -> Aabb {
        Aabb::of(&self.positions)
    }

    /// Adds another mesh's triangles to this one. Nothing is welded.
    pub fn append(&mut self, other: &TriMesh) {
        let base = self.positions.len() as u32;
        self.positions.extend_from_slice(&other.positions);
        self.indices.extend(other.indices.iter().map(|&i| i + base));
    }

    /// Moves every vertex by `matrix`. A matrix that mirrors (negative
    /// determinant) also reverses the winding, so the outside stays outside.
    pub fn transform(&mut self, matrix: &Mat4) {
        for p in &mut self.positions {
            *p = matrix.transform_point3(*p);
        }
        if matrix.determinant() < 0.0 {
            self.flip();
        }
    }

    /// Reverses the winding of every triangle.
    pub fn flip(&mut self) {
        for t in self.indices.chunks_exact_mut(3) {
            t.swap(1, 2);
        }
    }

    /// Volume enclosed, positive for a closed mesh wound outward.
    pub fn signed_volume(&self) -> f64 {
        self.indices
            .chunks_exact(3)
            .map(|t| {
                let [a, b, c] = [t[0], t[1], t[2]].map(|i| self.positions[i as usize].as_dvec3());
                a.dot(b.cross(c))
            })
            .sum::<f64>()
            / 6.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tetra() -> TriMesh {
        let p = vec![Vec3::ZERO, Vec3::X, Vec3::Y, Vec3::Z];
        TriMesh::new(p, vec![0, 2, 1, 0, 1, 3, 0, 3, 2, 1, 2, 3])
    }

    #[test]
    fn volume_of_a_tetrahedron() {
        assert!((tetra().signed_volume() - 1.0 / 6.0).abs() < 1e-9);
    }

    #[test]
    fn mirroring_keeps_the_outside_out() {
        let mut m = tetra();
        m.transform(&Mat4::from_scale(Vec3::new(-1.0, 1.0, 1.0)));
        assert!(m.signed_volume() > 0.0);
    }

    #[test]
    fn append_offsets_indices() {
        let mut m = tetra();
        m.append(&tetra());
        assert_eq!(m.vertex_count(), 8);
        assert_eq!(m.indices[12..15], [4, 6, 5]);
        assert!(Aabb::EMPTY.is_empty() && !m.bounds().is_empty());
    }
}
