//! Cutting a mesh in two along a flat plane.
//!
//! The Split tool cuts along a line drawn on screen; the tool's demo and this
//! crate's tests want the same cut without a screen, so this builds the
//! camera that line would have been drawn through and hands it to
//! `sculpt_cut::split`.

use glam::{Mat4, Vec2, Vec3, Vec4};
use sculpt_cut::{CutView, ScreenRegion, Split, SplitPass, split};
use sculpt_geom::{SymmetryOp, TriMesh};

use crate::types::any_perpendicular;

/// Right-handed view matrix from an orthonormal camera basis, looking down
/// -Z as the app's own does.
fn look_to(eye: Vec3, right: Vec3, up: Vec3, forward: Vec3) -> Mat4 {
    Mat4::from_cols(
        Vec4::new(right.x, up.x, -forward.x, 0.0),
        Vec4::new(right.y, up.y, -forward.y, 0.0),
        Vec4::new(right.z, up.z, -forward.z, 0.0),
        Vec4::new(-right.dot(eye), -up.dot(eye), forward.dot(eye), 1.0),
    )
}

/// An orthographic camera that sees the plane through `point` with normal
/// `normal` edge-on, as a level line across the middle of its picture with
/// the normal pointing up the screen. `reach` is how far from `point` the
/// mesh extends.
fn edge_on_view(point: Vec3, normal: Vec3, reach: f32) -> CutView {
    let up = normal.normalize_or(Vec3::Y);
    let forward = any_perpendicular(up);
    let right = forward.cross(up);
    let view = look_to(point - forward * reach * 2.0, right, up, forward);
    let (near, far) = (reach * 0.01, reach * 10.0);
    let range = far - near;
    let proj = Mat4::from_cols(
        Vec4::new(1.0 / reach, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / reach, 0.0, 0.0),
        Vec4::new(0.0, 0.0, 1.0 / range, 0.0),
        Vec4::new(0.0, 0.0, far / range, 1.0),
    );
    CutView::new(proj * view, view, Vec2::ZERO, Vec2::splat(1000.0))
}

/// Splits `mesh` along the plane through `point` with normal `normal`, both
/// in the mesh's own space. `Split::inside` is the piece on the side the
/// normal points to, `Split::outside` the piece behind it.
pub fn split_by_plane(mesh: &TriMesh, point: Vec3, normal: Vec3) -> Result<Split, String> {
    let bounds = mesh.bounds();
    if bounds.is_empty() {
        return Err("No geometry.".to_string());
    }
    let reach = (bounds.max.distance(point)).max(bounds.min.distance(point)).max(bounds.size().length()).max(1e-3);
    let view = edge_on_view(point, normal, reach);
    let centre = view.projection.project(point).ok_or("The cut plane is behind the camera.")?;
    // A line covers what is on its left looking along it; drawn left to
    // right, that is the upper half of the picture, where the normal points.
    let region = ScreenRegion::half_plane(centre - Vec2::X * 100.0, centre + Vec2::X * 100.0)?;
    split(&mesh.positions, &mesh.indices, None, &[SplitPass { view, symmetry: SymmetryOp::IDENTITY }], &region)
}

/// As `split_by_plane`, but along a surface that bows toward the normal by
/// `sag` at `across` from `point` (and four times that at twice as far): a
/// gently curved cut, as a lasso drawn by hand makes.
#[cfg(test)]
pub(crate) fn split_by_bow(mesh: &TriMesh, point: Vec3, normal: Vec3, sag: f32, across: f32) -> Result<Split, String> {
    let bounds = mesh.bounds();
    let reach = (bounds.max.distance(point)).max(bounds.min.distance(point)).max(bounds.size().length());
    let view = edge_on_view(point, normal, reach);
    let centre = view.projection.project(point).ok_or("The cut is behind the camera.")?;
    let per_unit = 500.0 / reach;
    let bow = |u: f32| centre + Vec2::new(u, -sag * (u / across) * (u / across)) * per_unit;
    let mut outline: Vec<Vec2> = (0..=400).map(|i| bow((i as f32 / 200.0 - 1.0) * reach * 0.9)).collect();
    // Closed round the top of the picture: everything above the bow.
    outline.push(centre + Vec2::new(0.9, -0.98) * reach * per_unit);
    outline.push(centre + Vec2::new(-0.9, -0.98) * reach * per_unit);
    let region = ScreenRegion::lasso(&outline)?;
    split(&mesh.positions, &mesh.indices, None, &[SplitPass { view, symmetry: SymmetryOp::IDENTITY }], &region)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{assert_closed, icosphere};

    #[test]
    fn the_piece_the_normal_points_into_is_the_inside_one() {
        let sphere = icosphere(4, 1.0);
        for normal in [Vec3::Y, Vec3::new(0.4, -0.3, 0.8).normalize(), Vec3::NEG_X] {
            let point = normal * 0.2;
            let cut = split_by_plane(&sphere, point, normal).unwrap();
            assert_closed(&cut.inside.mesh, "inside");
            assert_closed(&cut.outside.mesh, "outside");
            assert!(cut.inside.mesh.positions.iter().all(|p| (*p - point).dot(normal) > -1e-4));
            assert!(cut.outside.mesh.positions.iter().all(|p| (*p - point).dot(normal) < 1e-4));
            assert_eq!(cut.face_pairs, vec![(0, 0)]);
            // The smaller cap is the side the plane was moved toward.
            assert!(cut.inside.mesh.signed_volume() < cut.outside.mesh.signed_volume());
        }
    }
}
