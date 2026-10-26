//! How much a set of UVs stretches the surface: Sander's L2 stretch (Sander,
//! Snyder, Gortler and Hoppe, "Texture Mapping Progressive Meshes", 2001),
//! normalized for the overall scale.
//!
//! For each triangle, the map from its UVs back onto the surface stretches a
//! small step in the picture by between `g` and `G` times, depending on the
//! step's direction. The triangle's stretch is the root mean square of the
//! two, `sqrt((G² + g²) / 2)`, and the model's is the root mean square over
//! all triangles weighted by surface area. Multiplied by the square root of
//! UV area over surface area, that is 1 exactly when every triangle is laid
//! flat at one shared scale, and grows with any stretching, squashing or
//! shearing. It is the measure texture-atlas papers report, so numbers can be
//! compared with theirs.

use glam::{DVec2, DVec3, Vec2, Vec3};

/// The normalized L2 stretch of `uvs` (one per triangle corner) over the
/// model. Triangles with no surface area are left out, and so are triangles
/// with no UV area (squashed ones the unwrapper parks on a point); a model
/// with none left measures 1.
pub(crate) fn l2_stretch(positions: &[Vec3], triangles: &[u32], uvs: &[Vec2]) -> f32 {
    let (mut weighted, mut surface, mut flat) = (0.0f64, 0.0f64, 0.0f64);
    for (t, corners) in triangles.chunks_exact(3).enumerate() {
        let [q1, q2, q3] = [0, 1, 2].map(|c| positions[corners[c] as usize].as_dvec3());
        let [p1, p2, p3] = [0, 1, 2].map(|c| uvs[t * 3 + c].as_dvec2());
        let area = (q2 - q1).cross(q3 - q1).length() / 2.0;
        let uv_area = signed_area(p1, p2, p3).abs();
        if !(area > 0.0) || !(uv_area > 1e-14) {
            continue;
        }
        let (s_dir, t_dir) = surface_steps([q1, q2, q3], [p1, p2, p3]);
        weighted += area * (s_dir.length_squared() + t_dir.length_squared()) / 2.0;
        surface += area;
        flat += uv_area;
    }
    if !(surface > 0.0) {
        return 1.0;
    }
    ((weighted / surface) * (flat / surface)).sqrt() as f32
}

/// How far along the surface one UV unit across and one UV unit down go on
/// a triangle: the two columns of the map's Jacobian.
fn surface_steps(q: [DVec3; 3], p: [DVec2; 3]) -> (DVec3, DVec3) {
    let twice = 2.0 * signed_area(p[0], p[1], p[2]);
    let across = (q[0] * (p[1].y - p[2].y) + q[1] * (p[2].y - p[0].y) + q[2] * (p[0].y - p[1].y)) / twice;
    let down = (q[0] * (p[2].x - p[1].x) + q[1] * (p[0].x - p[2].x) + q[2] * (p[1].x - p[0].x)) / twice;
    (across, down)
}

fn signed_area(a: DVec2, b: DVec2, c: DVec2) -> f64 {
    (b - a).perp_dot(c - a) / 2.0
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A flat square of two triangles, and its UVs made by `map` from each
    /// point's x and y.
    fn square(map: impl Fn(f32, f32) -> Vec2) -> f32 {
        let positions = [Vec3::ZERO, Vec3::X, Vec3::new(1.0, 1.0, 0.0), Vec3::Y];
        let triangles = [0, 1, 2, 0, 2, 3];
        let uvs: Vec<Vec2> = triangles.iter().map(|&v| map(positions[v as usize].x, positions[v as usize].y)).collect();
        l2_stretch(&positions, &triangles, &uvs)
    }

    #[test]
    fn a_square_laid_flat_at_any_scale_or_turn_measures_one() {
        assert!((square(Vec2::new) - 1.0).abs() < 1e-6);
        assert!((square(|x, y| Vec2::new(x, y) * 0.01) - 1.0).abs() < 1e-6);
        assert!((square(|x, y| Vec2::new(0.3 - y, 0.2 + x) * 0.5) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_square_stretched_twice_as_wide_measures_the_known_amount() {
        // Steps across shrink by half back on the surface and steps down do
        // not: sqrt((1/4 + 1) / 2), times sqrt(2) for the doubled area.
        let expected = (0.625f32 * 2.0).sqrt();
        assert!((square(|x, y| Vec2::new(2.0 * x, y)) - expected).abs() < 1e-5);
        // Sheared, the same way about.
        assert!(square(|x, y| Vec2::new(x + 0.5 * y, y)) > 1.05);
    }
}
