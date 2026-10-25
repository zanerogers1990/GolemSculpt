//! A model's faces as a file gives them, polygons of any size, made into the
//! triangles a mesh is built from. Every model reader ends here, so that each
//! is welded and tidied the same way and a model made of quads stays quads
//! (`RawMesh::polys`): subdividing it then uses Catmull-Clark, as ZBrush and
//! Blender would, and an export writes the quads back out.
//!
//! The triangles are each polygon cut as a fan from its first corner, the
//! cut every exporter makes. A polygon is kept whole or not at all: if any of
//! its triangles has to go (a corner that is not a number, two corners at one
//! place) the whole polygon goes, so the polygons and the triangles always
//! describe the same surface.

use glam::Vec3;

use super::{RawMesh, sound_triangle, weld_exact_mapped};
use crate::mesh::Polys;
use crate::mesh::paint::Rgba8;

/// Faces as a reader collects them: where each polygon's corners start in
/// `corners` (one more entry than there are polygons, the last being
/// `corners.len()`), the corners themselves (vertex numbers), and the face
/// group of each polygon (empty for none).
#[derive(Default)]
pub(super) struct Faces {
    pub start: Vec<u32>,
    pub corners: Vec<u32>,
    pub groups: Vec<u16>,
}

impl Faces {
    /// Adds one polygon. Corners past the vertex count are the caller's to
    /// leave out; a polygon of fewer than three is not added.
    pub fn push(&mut self, corners: &[u32], group: Option<u16>) {
        if corners.len() < 3 {
            return;
        }
        if self.start.is_empty() {
            self.start.push(0);
        }
        self.corners.extend_from_slice(corners);
        self.start.push(self.corners.len() as u32);
        if let Some(group) = group {
            self.groups.push(group);
        }
    }

    pub fn count(&self) -> usize {
        self.start.len().saturating_sub(1)
    }

    fn face(&self, f: usize) -> &[u32] {
        &self.corners[self.start[f] as usize..self.start[f + 1] as usize]
    }
}

/// The model these faces make: positions welded where they are exactly equal
/// (files split vertices along seams, and a sculpt needs one connected
/// surface), polygons that would hurt a sculpt dropped, then vertices nothing
/// uses. `colours`, if not empty, is one per position and follows the weld
/// (a welded vertex takes the first colour given for its place). Groups that
/// are all the same are no groups.
pub(super) fn raw_mesh(name: String, positions: Vec<Vec3>, faces: &Faces, real_size: bool, colours: Vec<Rgba8>) -> RawMesh {
    let count = positions.len();
    // Corners naming no vertex: such a polygon is left out.
    let usable = |f: usize| faces.face(f).iter().all(|&c| (c as usize) < count);
    let (positions, remap, first) = weld_exact_mapped(positions);
    let colours = if colours.len() == count { first.iter().map(|&i| colours[i as usize]).collect() } else { Vec::new() };

    let mut indices = Vec::with_capacity(faces.corners.len() * 3);
    let mut kept_faces = Vec::with_capacity(faces.count());
    let mut polygon = Vec::new();
    for f in 0..faces.count() {
        if !usable(f) {
            continue;
        }
        polygon.clear();
        polygon.extend(faces.face(f).iter().map(|&c| remap[c as usize]));
        let fan = |k: usize| [polygon[0], polygon[k], polygon[k + 1]];
        if (1..polygon.len() - 1).all(|k| sound_triangle(&positions, &fan(k))) {
            for k in 1..polygon.len() - 1 {
                indices.extend_from_slice(&fan(k));
            }
            kept_faces.push(f);
        }
    }

    // The vertices the kept faces use, renumbered in the file's order.
    let mut used = vec![false; positions.len()];
    for &i in &indices {
        used[i as usize] = true;
    }
    let mut new_id = vec![u32::MAX; positions.len()];
    let mut out = Vec::new();
    let mut out_colours = Vec::new();
    for (i, _) in used.iter().enumerate().filter(|(_, used)| **used) {
        new_id[i] = out.len() as u32;
        out.push(positions[i]);
        if !colours.is_empty() {
            out_colours.push(colours[i]);
        }
    }
    for i in &mut indices {
        *i = new_id[*i as usize];
    }

    let polys = kept_faces.iter().any(|&f| faces.face(f).len() > 3).then(|| {
        let mut start = Vec::with_capacity(kept_faces.len() + 1);
        let mut corners = Vec::new();
        start.push(0);
        for &f in &kept_faces {
            corners.extend(faces.face(f).iter().map(|&c| new_id[remap[c as usize] as usize]));
            start.push(corners.len() as u32);
        }
        Polys { start, indices: corners }
    });

    let mut groups = Vec::new();
    if faces.groups.len() == faces.count() {
        for &f in &kept_faces {
            let fan = faces.face(f).len() - 2;
            groups.extend(std::iter::repeat_n(faces.groups[f], fan));
        }
        if groups.iter().all(|&g| Some(&g) == groups.first()) {
            groups.clear();
        }
    }
    RawMesh { name, positions: out, indices, real_size, groups, colours: out_colours, polys }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square_pair() -> (Vec<Vec3>, Faces) {
        // Two quads side by side, the shared edge given twice over (as a file split along a seam).
        let positions = vec![
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(2.0, 0.0, 0.0),
            Vec3::new(2.0, 1.0, 0.0),
            Vec3::new(1.0, 1.0, 0.0),
        ];
        let mut faces = Faces::default();
        faces.push(&[0, 1, 2, 3], Some(0));
        faces.push(&[4, 5, 6, 7], Some(1));
        (positions, faces)
    }

    #[test]
    fn quads_stay_quads_and_the_seam_is_welded() {
        let (positions, faces) = square_pair();
        let raw = raw_mesh("Pair".into(), positions, &faces, false, Vec::new());
        assert_eq!(raw.positions.len(), 6, "the shared edge is one edge");
        assert_eq!(raw.indices.len(), 12, "each quad is two triangles");
        let polys = raw.polys.expect("the quads are kept");
        assert_eq!(polys.start, vec![0, 4, 8]);
        // Each quad's fan is the triangles, corner for corner.
        for (q, t) in polys.start.windows(2).zip(raw.indices.chunks_exact(6)) {
            let c = &polys.indices[q[0] as usize..q[1] as usize];
            assert_eq!(t, [c[0], c[1], c[2], c[0], c[2], c[3]]);
        }
        assert_eq!(raw.groups, vec![0, 0, 1, 1]);
        assert_eq!(raw.name, "Pair");
    }

    #[test]
    fn triangles_alone_give_no_polygons_and_one_group_is_none() {
        let positions = vec![Vec3::ZERO, Vec3::X, Vec3::Y];
        let mut faces = Faces::default();
        faces.push(&[0, 1, 2], Some(3));
        let raw = raw_mesh(String::new(), positions, &faces, true, Vec::new());
        assert!(raw.polys.is_none());
        assert!(raw.groups.is_empty());
        assert!(raw.real_size);
    }

    #[test]
    fn a_broken_polygon_goes_whole_and_takes_its_lone_vertices() {
        let (mut positions, mut faces) = square_pair();
        // A third quad with two corners on one point, and one off the end.
        positions.push(Vec3::new(3.0, 0.0, 0.0));
        faces.push(&[5, 8, 8, 6], None);
        faces.push(&[5, 8, 99, 6], None);
        // Groups now do not cover every face, so none are kept.
        let raw = raw_mesh(String::new(), positions, &faces, false, Vec::new());
        assert_eq!(raw.indices.len(), 12);
        assert_eq!(raw.positions.len(), 6, "the vertex only the broken quads used is gone");
        assert_eq!(raw.polys.unwrap().start.len(), 3);
        assert!(raw.groups.is_empty());
    }

    #[test]
    fn colours_follow_the_weld_and_the_tidy() {
        let (positions, faces) = square_pair();
        let colours: Vec<Rgba8> = (0..8).map(|i| Rgba8([i as u8, 0, 0, 255])).collect();
        let raw = raw_mesh(String::new(), positions, &faces, false, colours);
        // Vertex 4 welded onto vertex 1 and 7 onto 2: each keeps the first colour of its place.
        assert_eq!(raw.colours.iter().map(|c| c.0[0]).collect::<Vec<_>>(), vec![0, 1, 2, 3, 5, 6]);
    }
}
