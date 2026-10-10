//! A dragged box, lasso or straight line as a screen-space stencil, and the
//! projection that puts a mesh's points under it.
//!
//! Screen positions are pixels with (0, 0) at the top-left corner and y running
//! down, as everywhere in the app.
//!
//! A box is stored as plain bounds; a lasso is scan-converted once into a
//! coverage grid over its own bounding box, so testing a vertex is an array
//! lookup rather than a walk of every lasso segment. That is the difference
//! between O(vertices) and O(vertices x lasso points), and the second runs into
//! hundreds of millions of operations for a detailed lasso on a dense mesh, all
//! of it in the frame the button is released.
//!
//! Trimming needs one thing hiding and masking never did: the shape's boundary,
//! not just its interior. Swept along the view direction the boundary is the
//! surface that does the cutting, and `arc_position` is the coordinate along it
//! that lets a cut loop be triangulated on that surface rather than flattened
//! onto a plane.

use glam::{Mat4, Vec2, Vec3};

use crate::polygon;

/// What a user sees when a drag is too small to be a shape.
pub const REGION_TOO_SMALL: &str = "Region too small.";

/// A drag shorter than this in both axes (for a lasso or a line, one whose
/// whole path is shorter than twice this) is a click rather than a tiny
/// region. Small enough that a deliberate little box still works, large enough
/// to absorb the tremor in a click.
pub const CLICK_SLOP_PIXELS: f32 = 6.0;

/// The least spacing between recorded lasso points. Recording every frame's
/// position puts hundreds of near-identical points on a slow drag, all of
/// which are then walked for no added accuracy.
pub const LASSO_POINT_SPACING_PIXELS: f32 = 4.0;

/// The end of a line drawn from `start` toward `end`, turned to the nearest
/// 15 degrees and kept the same length: what holding Shift does to a
/// straight-line cut.
pub fn snap_line_end(start: Vec2, end: Vec2) -> Vec2 {
    let d = end - start;
    let length = d.length();
    if length < 1e-3 {
        return end;
    }
    let step = 15f32.to_radians();
    let angle = (d.y.atan2(d.x) / step).round() * step;
    start + Vec2::from_angle(angle) * length
}

/// Model space to screen pixels: a model-view-projection matrix and the
/// viewport rectangle it draws into.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenProjection {
    /// Projection x view x model, for the object being tested.
    pub view_proj: Mat4,
    /// Top-left corner of the viewport, in pixels.
    pub origin: Vec2,
    /// Size of the viewport, in pixels.
    pub size: Vec2,
}

impl ScreenProjection {
    pub fn new(view_proj: Mat4, origin: Vec2, size: Vec2) -> ScreenProjection {
        ScreenProjection { view_proj, origin, size }
    }

    /// Pixel position of a model-space point, or `None` when it is at or
    /// behind the eye, where a projection would be mirrored nonsense.
    ///
    /// The one projection every region tool shares. Hiding and masking decide
    /// which vertices a shape covers with this, and trimming then hunts along
    /// mesh edges for the exact place the answer flips; if the two disagreed by
    /// even a rounding step the cut would land off the boundary the selection
    /// was made against. It matches the app's own `View::project`: only the
    /// perspective eye is tested, because the orthographic eye is parked far
    /// outside the scene and the depth range differs between renderers.
    #[inline]
    pub fn project(&self, p: Vec3) -> Option<Vec2> {
        let clip = self.view_proj * p.extend(1.0);
        if clip.w <= 1e-6 {
            return None;
        }
        let inv = 1.0 / clip.w;
        Some(Vec2::new(
            self.origin.x + (clip.x * inv * 0.5 + 0.5) * self.size.x,
            self.origin.y + (0.5 - clip.y * inv * 0.5) * self.size.y,
        ))
    }

    /// Normalised device x and y (-1..1, y up) of a pixel position: the inverse
    /// of the viewport half of `project`.
    #[inline]
    pub fn ndc(&self, screen: Vec2) -> Vec2 {
        let uv = (screen - self.origin) / self.size;
        Vec2::new(uv.x * 2.0 - 1.0, 1.0 - uv.y * 2.0)
    }
}

/// The camera as the cutting tools need it: where a point lands on screen, and
/// how far in front of the eye it is.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CutView {
    pub projection: ScreenProjection,
    /// View x model, for the object being cut. View space looks down -Z, so a
    /// point in front of the eye has negative z.
    pub model_to_view: Mat4,
}

impl CutView {
    /// `view_proj` and `model_to_view` are both for the object being cut:
    /// projection x view x model, and view x model.
    pub fn new(view_proj: Mat4, model_to_view: Mat4, origin: Vec2, size: Vec2) -> CutView {
        CutView { projection: ScreenProjection::new(view_proj, origin, size), model_to_view }
    }
}

/// What one cell of a lasso's grid can say about the points inside it. Only
/// cells no outline segment passes through can answer on their own; the rest
/// defer to the exact test, which is what keeps the region's boundary a real
/// curve instead of a staircase.
const CELL_OUTSIDE: u8 = 0;
const CELL_INSIDE: u8 = 1;
const CELL_STRADDLES: u8 = 2;

/// The arc "perimeter" of a straight cut. A line never closes, so no cut loop
/// can wind round it; this only has to be far beyond any real arc difference,
/// so the trimmer's wrap-around arithmetic (which rounds a difference to whole
/// perimeters) never fires, while staying finite for that same arithmetic.
const HALF_PLANE_PERIMETER: f32 = 1e7;

#[derive(Clone, Debug)]
enum Shape {
    /// Exact bounds of the drag, in unsnapped pixels.
    Rect { min: Vec2, max: Vec2 },
    /// A grid over the lasso's bounding box, one cell per pixel.
    Lasso { min_x: i32, min_y: i32, width: i32, height: i32, cells: Vec<u8> },
    /// Everything on one side of the line through `origin` along the unit
    /// vector `direction`. Unbounded, so it is tested and charted exactly
    /// rather than through the outline.
    HalfPlane { origin: Vec2, direction: Vec2 },
}

/// A box, lasso or straight-line region in screen pixels.
#[derive(Clone, Debug)]
pub struct ScreenRegion {
    shape: Shape,
    /// The closed outline: four corners for a box, the drawn path for a lasso,
    /// the two drawn ends for a line.
    boundary: Vec<Vec2>,
    /// Cumulative arc length reaching each outline point. One more entry than
    /// `boundary`: the last is the full perimeter, the length of the implicit
    /// closing segment included.
    arc: Vec<f32>,
}

impl ScreenRegion {
    fn with_outline(shape: Shape, boundary: Vec<Vec2>) -> ScreenRegion {
        let n = boundary.len();
        let mut arc = Vec::with_capacity(n + 1);
        arc.push(0.0f32);
        for i in 0..n {
            arc.push(arc[i] + boundary[i].distance(boundary[(i + 1) % n]));
        }
        ScreenRegion { shape, boundary, arc }
    }

    /// The box with opposite corners `a` and `b`.
    ///
    /// The outline is the raw drag, and the coverage test compares against
    /// those same floats: a cut placed against a boundary even half a pixel
    /// away from the one classifying the vertices would leave slivers on the
    /// wrong side.
    pub fn rect(a: Vec2, b: Vec2) -> Result<ScreenRegion, String> {
        let (lo, hi) = (a.min(b), a.max(b));
        if !(hi.x - lo.x > 0.0 && hi.y - lo.y > 0.0) {
            return Err(REGION_TOO_SMALL.to_string());
        }
        let corners = vec![lo, Vec2::new(hi.x, lo.y), hi, Vec2::new(lo.x, hi.y)];
        Ok(ScreenRegion::with_outline(Shape::Rect { min: lo, max: hi }, corners))
    }

    /// The area a freehand path encloses, with the path closed from its last
    /// point back to its first: exactly what a lasso means by "the bit I drew
    /// around", however open the drawn path was left. Even-odd, so a path that
    /// crosses itself leaves the doubly-wrapped part out.
    pub fn lasso(points: &[Vec2]) -> Result<ScreenRegion, String> {
        // Fewer than three points cannot enclose anything.
        if points.len() < 3 || points.iter().any(|p| !p.is_finite()) {
            return Err(REGION_TOO_SMALL.to_string());
        }
        let lo = points.iter().fold(Vec2::splat(f32::MAX), |m, p| m.min(*p));
        let hi = points.iter().fold(Vec2::splat(f32::MIN), |m, p| m.max(*p));
        let (min_x, min_y) = (lo.x.floor() as i32, lo.y.floor() as i32);
        let (width, height) = (hi.x.ceil() as i32 - min_x, hi.y.ceil() as i32 - min_y);
        if width <= 0 || height <= 0 {
            return Err(REGION_TOO_SMALL.to_string());
        }

        let mut cells = vec![CELL_OUTSIDE; width as usize * height as usize];
        fill_rows(points, &mut cells, min_x, min_y, width);
        // The fill only sampled each cell's centre, so a cell the outline runs
        // through is right about its middle and wrong about the rest of itself.
        // Marking every such cell sends the points inside it to the exact test
        // instead, which is what turns the grid from an answer into a mere
        // accelerator.
        mark_straddling_cells(points, &mut cells, min_x, min_y, width, height);

        Ok(ScreenRegion::with_outline(Shape::Lasso { min_x, min_y, width, height, cells }, points.to_vec()))
    }

    /// A straight-line cut: the line through `a` and `b`, extended without end,
    /// covering everything on its left as seen on screen when looking from `a`
    /// toward `b`.
    ///
    /// Swept along the view direction that line is a plane (through the eye
    /// under a perspective camera, parallel to the view under an orthographic
    /// one), so every cut face it makes is exactly flat, which is what a
    /// printed part's mating faces want.
    pub fn half_plane(a: Vec2, b: Vec2) -> Result<ScreenRegion, String> {
        let d = b - a;
        let length = d.length();
        if length < 1e-3 || !length.is_finite() {
            return Err(REGION_TOO_SMALL.to_string());
        }
        Ok(ScreenRegion::with_outline(Shape::HalfPlane { origin: a, direction: d / length }, vec![a, b]))
    }

    /// The outline as a closed polyline in screen pixels.
    pub fn boundary(&self) -> &[Vec2] {
        &self.boundary
    }

    /// Total length of the closed outline, in pixels. `arc_position` returns a
    /// value in `0..perimeter`.
    pub fn perimeter(&self) -> f32 {
        match self.shape {
            Shape::HalfPlane { .. } => HALF_PLANE_PERIMETER,
            _ => self.arc[self.arc.len() - 1],
        }
    }

    /// True for a straight-line cut.
    pub fn is_half_plane(&self) -> bool {
        matches!(self.shape, Shape::HalfPlane { .. })
    }

    /// Whether the shape covers this screen point: exactly, to floating-point
    /// precision, not to the nearest pixel.
    ///
    /// That distinction is the whole reason this is not just a grid lookup.
    /// Trimming finds its cut by bisecting this predicate, so whatever shape
    /// the predicate has is the shape of the cut surface. A per-pixel grid
    /// makes that surface a staircase of pixel-wide steps, and on a dense
    /// sculpt one pixel is about one triangle, so the cut comes out visibly
    /// serrated. Measured in the Unity app on a 359k-triangle model: the cut
    /// wandered 0.8 px either side of the drawn circle, against 0.14 px once
    /// the test became exact.
    ///
    /// The grid is kept, but only as an accelerator: a cell no outline segment
    /// crosses is wholly inside or wholly outside and answers at once, so the
    /// O(outline) polygon test runs only for points in the thin band along the
    /// boundary.
    #[inline]
    pub fn contains(&self, p: Vec2) -> bool {
        match &self.shape {
            Shape::Rect { min, max } => p.x >= min.x && p.x <= max.x && p.y >= min.y && p.y <= max.y,
            Shape::HalfPlane { origin, direction } => {
                // Left of the line on a screen whose y runs down.
                direction.x * (p.y - origin.y) - direction.y * (p.x - origin.x) < 0.0
            }
            Shape::Lasso { min_x, min_y, width, height, cells } => {
                let (x, y) = (p.x.floor() as i32 - min_x, p.y.floor() as i32 - min_y);
                if x < 0 || y < 0 || x >= *width || y >= *height {
                    return false;
                }
                match cells[(y * width + x) as usize] {
                    CELL_INSIDE => true,
                    CELL_OUTSIDE => false,
                    // Same rule as the scanline fill that built the grid, which
                    // is what lets the two be mixed without ever disagreeing.
                    _ => polygon::contains(&self.boundary, p),
                }
            }
        }
    }

    /// Whether `p` lies within `distance` pixels of the outline, on either
    /// side of it. `distance` is meant to be a small fraction of a pixel: the
    /// trimmer uses this to tell a vertex the outline passes through from one
    /// that is merely near it.
    #[inline]
    pub fn near_boundary(&self, p: Vec2, distance: f32) -> bool {
        match &self.shape {
            Shape::Rect { min, max } => {
                let outside = p - p.clamp(*min, *max);
                if outside != Vec2::ZERO {
                    return outside.length() <= distance;
                }
                let inset = (p - *min).min(*max - p);
                inset.min_element() <= distance
            }
            Shape::HalfPlane { origin, direction } => {
                (direction.x * (p.y - origin.y) - direction.y * (p.x - origin.x)).abs() <= distance
            }
            Shape::Lasso { min_x, min_y, width, height, cells } => {
                let (x, y) = (p.x.floor() as i32 - min_x, p.y.floor() as i32 - min_y);
                if x >= 0 && y >= 0 && x < *width && y < *height && cells[(y * width + x) as usize] != CELL_STRADDLES {
                    // Every cell the outline comes within half a pixel of is
                    // marked as straddling it.
                    return false;
                }
                let (lo, hi) = (
                    Vec2::new(*min_x as f32, *min_y as f32),
                    Vec2::new((min_x + width) as f32, (min_y + height) as f32),
                );
                if (p - p.clamp(lo, hi)).length() > distance {
                    return false;
                }
                polygon::distance_to_outline(&self.boundary, p) <= distance
            }
        }
    }

    /// Distance along the closed outline, from its first point, of the outline
    /// point nearest `p`. This is the "which way around the shape am I"
    /// coordinate of the swept cutting surface: paired with view depth it gives
    /// that surface a flat 2D chart, which is what lets a cut loop be
    /// triangulated on the surface, including across a box's corner, where any
    /// single flat plane through the loop would slice the corner off.
    ///
    /// A plain scan over the segments. Called once per cut vertex (thousands at
    /// most, against a few hundred lasso segments), never per mesh vertex.
    pub fn arc_position(&self, p: Vec2) -> f32 {
        // Along a straight cut the coordinate is just distance along the line,
        // signed and unclamped, since the line runs on past both drawn ends.
        if let Shape::HalfPlane { origin, direction } = self.shape {
            return (p - origin).dot(direction);
        }
        let n = self.boundary.len();
        let (mut best_sqr, mut best_arc) = (f32::MAX, 0.0);
        for i in 0..n {
            let a = self.boundary[i];
            let ab = self.boundary[(i + 1) % n] - a;
            let len_sqr = ab.length_squared();
            let t = if len_sqr > 1e-12 { ((p - a).dot(ab) / len_sqr).clamp(0.0, 1.0) } else { 0.0 };
            let sqr = (p - (a + ab * t)).length_squared();
            if sqr < best_sqr {
                best_sqr = sqr;
                best_arc = self.arc[i] + t * len_sqr.sqrt();
            }
        }
        best_arc
    }

    /// The inverse of `arc_position`: the outline point `arc` pixels along from
    /// the first, wrapping round as many times as needed.
    ///
    /// This is what lets a cut face be evaluated on the cutting surface rather
    /// than interpolated across it: a point averaged from its neighbours sits
    /// on the chord between them, off a curved outline by the chord's sagitta.
    pub fn point_at_arc(&self, arc: f32) -> Vec2 {
        if let Shape::HalfPlane { origin, direction } = self.shape {
            return origin + direction * arc;
        }
        let n = self.boundary.len();
        let perimeter = self.perimeter();
        if perimeter <= 0.0 {
            return self.boundary[0];
        }
        let arc = (arc - perimeter * (arc / perimeter).floor()).max(0.0);

        // The last segment closes the outline, so the search runs over `n`
        // segments and `arc[i + 1]` is always a valid end.
        let (mut lo, mut hi) = (0, n - 1);
        while lo < hi {
            let mid = (lo + hi + 1) >> 1;
            if self.arc[mid] <= arc {
                lo = mid;
            } else {
                hi = mid - 1;
            }
        }
        let length = self.arc[lo + 1] - self.arc[lo];
        let t = if length > 1e-6 { (arc - self.arc[lo]) / length } else { 0.0 };
        let (a, b) = (self.boundary[lo], self.boundary[(lo + 1) % n]);
        a + (b - a) * t
    }
}

/// Even-odd scanline fill through each cell's centre.
fn fill_rows(points: &[Vec2], cells: &mut [u8], min_x: i32, min_y: i32, width: i32) {
    let mut crossings: Vec<f32> = Vec::new();
    for (row, line) in cells.chunks_exact_mut(width as usize).enumerate() {
        let y = (min_y + row as i32) as f32 + 0.5;
        crossings.clear();
        let mut p1 = points[points.len() - 1];
        for &p2 in points {
            // Half-open comparison (<=, not <) counts a point sitting exactly
            // on the scanline once rather than twice, which is what keeps the
            // parity right through a horizontal run of points.
            if (p1.y <= y) != (p2.y <= y) {
                let t = (y - p1.y) / (p2.y - p1.y);
                crossings.push(p1.x + t * (p2.x - p1.x));
            }
            p1 = p2;
        }
        crossings.sort_unstable_by(f32::total_cmp);
        for pair in crossings.chunks_exact(2) {
            let x0 = ((pair[0] - 0.5).ceil() as i32 - min_x).max(0);
            let x1 = ((pair[1] - 0.5).floor() as i32 - min_x).min(width - 1);
            if x0 <= x1 {
                line[x0 as usize..=x1 as usize].fill(CELL_INSIDE);
            }
        }
    }
}

/// Flags every cell any outline segment passes through, plus its immediate
/// neighbours. Walked at half-pixel steps with a one-cell halo rather than by
/// an exact line rasteriser: a segment can clip the corner of a cell over a
/// span far shorter than the sampling step, and the halo covers that case for
/// the cost of eight extra writes.
fn mark_straddling_cells(boundary: &[Vec2], cells: &mut [u8], min_x: i32, min_y: i32, width: i32, height: i32) {
    let n = boundary.len();
    for i in 0..n {
        let (a, b) = (boundary[i], boundary[(i + 1) % n]);
        let steps = ((a.distance(b) * 2.0).ceil() as i32).max(1);
        for s in 0..=steps {
            let p = a + (b - a) * (s as f32 / steps as f32);
            let (cx, cy) = (p.x.floor() as i32 - min_x, p.y.floor() as i32 - min_y);
            for y in (cy - 1).max(0)..=(cy + 1).min(height - 1) {
                for x in (cx - 1).max(0)..=(cx + 1).min(width - 1) {
                    cells[(y * width + x) as usize] = CELL_STRADDLES;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{Rng, ortho_view};

    /// A star with deep notches: concave everywhere, and nowhere axis-aligned.
    pub(crate) fn star(centre: Vec2, outer: f32, inner: f32, points: usize) -> Vec<Vec2> {
        (0..points * 2)
            .map(|i| {
                let angle = i as f32 * std::f32::consts::PI / points as f32 + 0.1;
                let r = if i % 2 == 0 { outer } else { inner };
                centre + Vec2::new(angle.cos(), angle.sin()) * r
            })
            .collect()
    }

    #[test]
    fn a_snapped_line_keeps_its_length_and_turns_to_fifteen_degrees() {
        let start = Vec2::new(100.0, 200.0);
        for degrees in [0.0f32, 7.0, 8.0, 44.0, 91.0, 179.0, -100.0, 268.0] {
            let end = start + Vec2::from_angle(degrees.to_radians()) * 150.0;
            let snapped = snap_line_end(start, end) - start;
            assert!((snapped.length() - 150.0).abs() < 1e-3);
            let angle = snapped.y.atan2(snapped.x).to_degrees();
            assert!((angle / 15.0 - (angle / 15.0).round()).abs() < 1e-3, "{degrees} went to {angle}");
            assert!(snapped.angle_to(end - start).abs().to_degrees() <= 7.5 + 1e-3);
        }
        assert_eq!(snap_line_end(start, start), start);
    }

    #[test]
    fn degenerate_drags_are_refused() {
        assert_eq!(ScreenRegion::rect(Vec2::ONE, Vec2::new(1.0, 5.0)).unwrap_err(), REGION_TOO_SMALL);
        assert!(ScreenRegion::lasso(&[Vec2::ZERO, Vec2::ONE]).is_err());
        assert!(ScreenRegion::lasso(&[Vec2::ZERO, Vec2::X, Vec2::X * 2.0]).is_err());
        assert!(ScreenRegion::half_plane(Vec2::ONE, Vec2::ONE).is_err());
        assert!(ScreenRegion::rect(Vec2::ZERO, Vec2::ONE).is_ok());
    }

    #[test]
    fn box_covers_exactly_its_drag() {
        let r = ScreenRegion::rect(Vec2::new(30.5, 80.25), Vec2::new(10.25, 20.75)).unwrap();
        assert!(r.contains(Vec2::new(10.25, 20.75)) && r.contains(Vec2::new(30.5, 80.25)));
        assert!(!r.contains(Vec2::new(10.2, 50.0)) && !r.contains(Vec2::new(20.0, 80.3)));
        assert!((r.perimeter() - 2.0 * (20.25 + 59.5)).abs() < 1e-3);
        assert_eq!(r.boundary().len(), 4);
    }

    #[test]
    fn lasso_matches_brute_force_everywhere() {
        let outline = star(Vec2::new(210.3, 180.7), 150.0, 55.0, 7);
        let region = ScreenRegion::lasso(&outline).unwrap();
        let mut rng = Rng::new(7);
        let (mut inside, mut disagreements) = (0, 0);
        for _ in 0..200_000 {
            let p = Vec2::new(rng.range(20.0, 400.0), rng.range(0.0, 360.0));
            let want = polygon::contains(&outline, p);
            inside += want as usize;
            disagreements += (region.contains(p) != want) as usize;
        }
        assert_eq!(disagreements, 0);
        assert!(inside > 20_000, "the sample should land inside often: {inside}");
    }

    #[test]
    fn lasso_boundary_is_exact_not_pixelated() {
        // A sloping edge: points a hundredth of a pixel either side of it.
        let outline = [Vec2::new(10.0, 10.0), Vec2::new(110.0, 47.0), Vec2::new(20.0, 120.0)];
        let region = ScreenRegion::lasso(&outline).unwrap();
        let (a, b) = (outline[0], outline[1]);
        let normal = Vec2::new(-(b - a).y, (b - a).x).normalize();
        for i in 1..100 {
            let on = a + (b - a) * (i as f32 / 100.0);
            assert!(region.contains(on + normal * 0.01), "just inside at step {i}");
            assert!(!region.contains(on - normal * 0.01), "just outside at step {i}");
        }
    }

    #[test]
    fn near_boundary_is_a_thin_band_either_side_of_the_outline() {
        let outline = star(Vec2::new(210.3, 180.7), 150.0, 55.0, 7);
        let shapes = [
            (ScreenRegion::rect(Vec2::new(60.5, 40.25), Vec2::new(350.0, 300.0)).unwrap(), "box"),
            (ScreenRegion::lasso(&outline).unwrap(), "lasso"),
            (ScreenRegion::half_plane(Vec2::new(30.0, 20.0), Vec2::new(380.0, 330.0)).unwrap(), "line"),
        ];
        let mut rng = Rng::new(21);
        for (region, name) in &shapes {
            // Straight from the outline, one segment at a time (the line's two
            // drawn points stand for the whole line, so it is done apart).
            let distance = |p: Vec2| match *name {
                "line" => {
                    (p.y - 20.0 - (p.x - 30.0) * (310.0 / 350.0)).abs() / (1.0f32 + (310.0f32 / 350.0).powi(2)).sqrt()
                }
                _ => polygon::distance_to_outline(region.boundary(), p),
            };
            let (mut near, mut wrong) = (0, 0);
            for _ in 0..40_000 {
                // Mostly right on the outline, where the answer matters.
                let on = region.point_at_arc(rng.range(0.0, 2000.0));
                let p = on + Vec2::new(rng.range(-0.02, 0.02), rng.range(-0.02, 0.02));
                let d = distance(p);
                // Leave the last hair of the band to rounding.
                if (d - 0.01).abs() > 1e-4 {
                    near += (d < 0.01) as usize;
                    wrong += (region.near_boundary(p, 0.01) != (d < 0.01)) as usize;
                }
            }
            assert_eq!(wrong, 0, "{name}");
            assert!(near > 5_000, "{name}: {near} samples in the band");
            for _ in 0..20_000 {
                let p = Vec2::new(rng.range(-50.0, 450.0), rng.range(-50.0, 400.0));
                assert_eq!(region.near_boundary(p, 0.01), distance(p) <= 0.01, "{name} at {p}");
            }
        }
        // Either side of a box's edge, and round its corner.
        let (region, _) = &shapes[0];
        assert!(region.near_boundary(Vec2::new(60.5, 100.0), 0.0));
        assert!(
            region.near_boundary(Vec2::new(60.495, 100.0), 0.01)
                && region.near_boundary(Vec2::new(60.505, 100.0), 0.01)
        );
        assert!(
            !region.near_boundary(Vec2::new(60.48, 100.0), 0.01)
                && !region.near_boundary(Vec2::new(60.52, 100.0), 0.01)
        );
        assert!(
            region.near_boundary(Vec2::new(60.495, 40.245), 0.01)
                && !region.near_boundary(Vec2::new(60.49, 40.24), 0.01)
        );
    }

    #[test]
    fn self_crossing_lasso_is_even_odd() {
        // A figure of eight: both lobes are inside, the crossing is a point.
        let eight = [Vec2::new(0.0, 0.0), Vec2::new(100.0, 100.0), Vec2::new(100.0, 0.0), Vec2::new(0.0, 100.0)];
        let region = ScreenRegion::lasso(&eight).unwrap();
        assert!(region.contains(Vec2::new(80.0, 50.0)) && region.contains(Vec2::new(20.0, 50.0)));
        assert!(!region.contains(Vec2::new(50.0, 20.0)) && !region.contains(Vec2::new(50.0, 80.0)));
    }

    #[test]
    fn half_plane_covers_the_left_as_seen_on_screen() {
        // Drawn left to right: the covered side is above the line on screen,
        // which is smaller y.
        let r = ScreenRegion::half_plane(Vec2::new(100.0, 300.0), Vec2::new(500.0, 300.0)).unwrap();
        assert!(r.contains(Vec2::new(250.0, 100.0)) && !r.contains(Vec2::new(250.0, 500.0)));
        assert!(r.contains(Vec2::new(-4000.0, 299.0)), "the line has no ends");
        // Drawn top to bottom: looking down the screen, left is larger x.
        let r = ScreenRegion::half_plane(Vec2::new(300.0, 100.0), Vec2::new(300.0, 500.0)).unwrap();
        assert!(r.contains(Vec2::new(400.0, 0.0)) && !r.contains(Vec2::new(200.0, 0.0)));
        assert!(r.is_half_plane());
        assert!((r.arc_position(Vec2::new(250.0, 700.0)) - 600.0).abs() < 1e-3);
        assert!(r.point_at_arc(-50.0).distance(Vec2::new(300.0, 50.0)) < 1e-3);
    }

    #[test]
    fn arc_position_and_point_at_arc_are_inverses() {
        let shapes = [
            ScreenRegion::rect(Vec2::new(10.0, 20.0), Vec2::new(110.0, 70.0)).unwrap(),
            ScreenRegion::lasso(&star(Vec2::new(200.0, 200.0), 120.0, 60.0, 5)).unwrap(),
        ];
        for region in &shapes {
            let perimeter = region.perimeter();
            for i in 0..400 {
                let arc = perimeter * (i as f32 + 0.37) / 400.0;
                let p = region.point_at_arc(arc);
                assert!((region.arc_position(p) - arc).abs() < 0.02, "arc {arc} of {perimeter}");
                assert!(region.point_at_arc(arc + perimeter * 3.0).distance(p) < 0.05, "wraps forwards");
                assert!(region.point_at_arc(arc - perimeter).distance(p) < 0.05, "wraps backwards");
            }
        }
        // Round a box's corner: 100 along the top, then 10 down the side.
        assert!(shapes[0].point_at_arc(110.0).distance(Vec2::new(110.0, 30.0)) < 1e-3);
    }

    #[test]
    fn projection_puts_up_at_the_top_and_drops_what_is_behind() {
        let view = ortho_view(Vec3::ZERO, 0.0, 0.0, 2.0, Vec2::new(40.0, 10.0), Vec2::new(800.0, 400.0));
        let p = view.projection;
        // Looking down -Z from +Z: +X is right, +Y is up, which is smaller y.
        assert!(p.project(Vec3::ZERO).unwrap().distance(Vec2::new(440.0, 210.0)) < 1e-3);
        assert!(p.project(Vec3::new(0.0, 2.0, 0.0)).unwrap().distance(Vec2::new(440.0, 10.0)) < 1e-3);
        assert!(p.project(Vec3::new(4.0, 0.0, 0.5)).unwrap().distance(Vec2::new(840.0, 210.0)) < 1e-3);
        assert!(p.ndc(Vec2::new(440.0, 10.0)).distance(Vec2::new(0.0, 1.0)) < 1e-6);

        let persp = crate::testing::perspective_view(Vec3::ZERO, 0.0, 0.0, 5.0, Vec2::ZERO, Vec2::new(600.0, 600.0));
        assert!(persp.projection.project(Vec3::ZERO).unwrap().distance(Vec2::new(300.0, 300.0)) < 1e-3);
        assert!(persp.projection.project(Vec3::new(0.0, 0.0, 6.0)).is_none(), "behind the eye");
        let up = persp.projection.project(Vec3::new(0.0, 1.0, 0.0)).unwrap();
        assert!(up.y < 300.0 && (up.x - 300.0).abs() < 1e-3);
    }
}
