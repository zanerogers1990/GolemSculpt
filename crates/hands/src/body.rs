//! A whole hand or foot as one signed distance, and the closed mesh of it.
//!
//! The pieces are put together in three layers, because one blend for everything would be wrong
//! somewhere:
//!
//!   - The core (palm, or heel, arch and ankle) is one smooth union with a generous fillet, so its
//!     pads and bones read as one mass of flesh.
//!   - Each digit is a smooth union of its own bones with a small fillet, so its joints are round.
//!   - Digits meet each other with a plain union, and the core with a smooth one. Two fingers side by
//!     side stay two fingers, with a crease where they touch, instead of melting into a paddle; where a
//!     finger leaves the palm the fillet is the web between the fingers.
//!
//! Cuts (the hollow under an arch) and a floor (a flat sole) are taken away last.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::{Aabb, TriMesh};
use sculpt_volume::{Grid, Surface, sdf};

use crate::shapes::{Shape, box_distance, smooth_max, smooth_min};

/// Most lattice samples one build may use (4 bytes each). Past this the cell grows instead.
pub const MAX_SAMPLES: usize = 48_000_000;

/// Shapes joined by a smooth union with one fillet size.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub shapes: Vec<Shape>,
    pub blend: f32,
}

impl Group {
    pub fn new(blend: f32) -> Group {
        Group { shapes: Vec::new(), blend }
    }

    pub fn push(&mut self, shape: Shape) {
        self.shapes.push(shape);
    }

    pub fn bounds(&self) -> Aabb {
        self.shapes.iter().map(Shape::bounds).fold(Aabb::EMPTY, Aabb::union)
    }

    fn mirrored(&self) -> Group {
        Group { shapes: self.shapes.iter().map(Shape::mirrored).collect(), blend: self.blend }
    }
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Body {
    pub core: Group,
    /// Fingers, thumb or toes: each its own group.
    pub digits: Vec<Group>,
    /// Fillet where a digit meets the core.
    pub web: f32,
    /// Hollows taken out of the whole, with this fillet.
    pub cuts: Vec<Shape>,
    pub cut_blend: f32,
    /// Everything below this height is cut away (a flat sole), with its edge rounded by the second
    /// value.
    pub floor: Option<(f32, f32)>,
}

/// One group, ready to evaluate quickly: its shapes with their boxes.
struct Prepared {
    shapes: Vec<(Shape, Aabb)>,
    bounds: Aabb,
    blend: f32,
}

impl Prepared {
    fn new(group: &Group) -> Prepared {
        let shapes: Vec<(Shape, Aabb)> = group.shapes.iter().map(|s| (*s, s.bounds())).collect();
        let bounds = shapes.iter().map(|(_, b)| *b).fold(Aabb::EMPTY, Aabb::union);
        Prepared { shapes, bounds, blend: group.blend }
    }

    /// The group's distance, or something above `limit` when it is that far away anyway. A shape is
    /// skipped when its box is further off than the nearest distance so far plus the fillet, which
    /// is when it cannot change the answer.
    fn distance(&self, p: Vec3, limit: f32) -> f32 {
        if self.shapes.is_empty() || box_distance(&self.bounds, p) > limit {
            return f32::MAX;
        }
        let mut d = f32::MAX;
        for (shape, bounds) in &self.shapes {
            if box_distance(bounds, p) > d.min(limit) + self.blend {
                continue;
            }
            let s = shape.distance(p);
            d = if d == f32::MAX { s } else { smooth_min(d, s, self.blend) };
        }
        d
    }
}

/// The body ready to sample.
pub struct Field {
    core: Prepared,
    digits: Vec<Prepared>,
    web: f32,
    cuts: Vec<Shape>,
    cut_blend: f32,
    floor: Option<(f32, f32)>,
    bounds: Aabb,
}

impl Field {
    /// The distance at `p`. Further than `band` from the surface the value is only promised to have
    /// the right sign, which is all extraction reads there.
    pub fn distance(&self, p: Vec3, band: f32) -> f32 {
        let limit = band + self.web;
        let core = self.core.distance(p, limit);
        let mut digits = f32::MAX;
        for digit in &self.digits {
            digits = digits.min(digit.distance(p, limit.min(digits)));
        }
        let mut d = if core == f32::MAX || digits == f32::MAX { core.min(digits) } else { smooth_min(core, digits, self.web) };
        if d == f32::MAX {
            return sdf::FAR;
        }
        for cut in &self.cuts {
            d = smooth_max(d, -cut.distance(p), self.cut_blend);
        }
        if let Some((height, round)) = self.floor {
            d = smooth_max(d, height - p.y, round);
        }
        d
    }

    pub fn bounds(&self) -> Aabb {
        self.bounds
    }
}

impl Body {
    pub fn field(&self) -> Field {
        let core = Prepared::new(&self.core);
        let digits: Vec<Prepared> = self.digits.iter().map(Prepared::new).filter(|d| !d.shapes.is_empty()).collect();
        let mut bounds = digits.iter().map(|d| d.bounds).fold(core.bounds, Aabb::union);
        if let Some((height, _)) = self.floor {
            bounds.min.y = bounds.min.y.max(height);
        }
        Field { core, digits, web: self.web, cuts: self.cuts.clone(), cut_blend: self.cut_blend, floor: self.floor, bounds }
    }

    /// The left-hand version of a right-handed body.
    pub fn mirrored(&self) -> Body {
        Body {
            core: self.core.mirrored(),
            digits: self.digits.iter().map(Group::mirrored).collect(),
            web: self.web,
            cuts: self.cuts.iter().map(Shape::mirrored).collect(),
            cut_blend: self.cut_blend,
            floor: self.floor,
        }
    }

    /// Moves every piece by `offset`.
    pub fn translate(&mut self, offset: Vec3) {
        let mut shift = |s: &mut Shape| match s {
            Shape::Cone { a, b, .. } => {
                *a += offset;
                *b += offset;
            }
            Shape::Ellipsoid { center, .. } | Shape::Box { center, .. } => *center += offset,
        };
        self.core.shapes.iter_mut().for_each(&mut shift);
        self.digits.iter_mut().flat_map(|d| d.shapes.iter_mut()).for_each(&mut shift);
        self.cuts.iter_mut().for_each(&mut shift);
        if let Some((height, _)) = &mut self.floor {
            *height += offset.y;
        }
    }

    /// The closed surface of the body, sampled every `cell`. A cell so fine that the lattice would
    /// pass [`MAX_SAMPLES`] is made coarser to fit.
    pub fn surface(&self, cell: f32) -> Result<Surface, String> {
        let field = self.field();
        let bounds = field.bounds();
        if bounds.is_empty() {
            return Err("There is nothing to build.".into());
        }
        let grid = grid_for(&bounds, cell)?;
        let band = grid.cell * 3.0;
        let [nx, ny, _] = grid.n;
        let mut values = vec![0.0f32; grid.sample_count()];
        values.par_chunks_mut(nx).enumerate().for_each(|(row, out)| {
            let (j, k) = (row % ny, row / ny);
            for (i, v) in out.iter_mut().enumerate() {
                let d = field.distance(grid.position(i, j, k), band);
                // An inside value of exactly zero would read as outside.
                *v = if d <= 0.0 { d.min(-sdf::MIN_MAGNITUDE) } else { d };
            }
        });
        sdf::surface_from_sdf(&values, &grid, [false; 3])
    }

    pub fn mesh(&self, cell: f32) -> Result<TriMesh, String> {
        Ok(self.surface(cell)?.into_mesh())
    }
}

/// The lattice of spacing `cell` over `bounds`, or a coarser one if that would be more than
/// [`MAX_SAMPLES`].
fn grid_for(bounds: &Aabb, cell: f32) -> Result<Grid, String> {
    // The lattice keeps its coordinates in 16 bits, so no axis may have more samples than that.
    let mut cell = cell.max(bounds.size().max_element() / 8000.0);
    loop {
        let grid = Grid::covering(bounds, cell)?;
        if grid.sample_count() <= MAX_SAMPLES {
            return Ok(grid);
        }
        cell *= ((grid.sample_count() as f64 / MAX_SAMPLES as f64).cbrt() as f32).max(1.01);
    }
}

#[cfg(test)]
mod tests {
    use glam::Mat3;
    use sculpt_geom::edge_census::EdgeCensus;

    use super::*;

    fn two_fingers(gap: f32) -> Body {
        let mut body = Body { web: 0.05, ..Body::default() };
        body.core = Group::new(0.05);
        body.core.push(Shape::Box { center: Vec3::ZERO, axes: Mat3::IDENTITY, half: Vec3::new(0.3, 0.2, 0.08), round: 0.06 });
        for x in [-0.08 - gap * 0.5, 0.08 + gap * 0.5] {
            let mut digit = Group::new(0.01);
            digit.push(Shape::cone(Vec3::new(x, 0.15, 0.0), Vec3::new(x, 0.6, 0.0), 0.08, 0.07));
            body.digits.push(digit);
        }
        body
    }

    #[test]
    fn a_body_meshes_closed() {
        let mesh = two_fingers(0.02).mesh(0.02).unwrap();
        assert!(mesh.triangle_count() > 500);
        assert!(EdgeCensus::of(&mesh).is_closed());
        assert!(mesh.signed_volume() > 0.0, "wound outward");
    }

    #[test]
    fn digits_side_by_side_do_not_melt_together() {
        // Halfway up, between two fingers a little apart, is outside: no fillet joins them there.
        let field = two_fingers(0.02).field();
        assert!(field.distance(Vec3::new(0.0, 0.5, 0.0), 1.0) > 0.0);
        // Down by the palm the web fills in.
        assert!(field.distance(Vec3::new(0.0, 0.205, 0.0), 1.0) < 0.0);
    }

    #[test]
    fn far_from_everything_reads_as_outside() {
        let field = two_fingers(0.02).field();
        assert!(field.distance(Vec3::splat(10.0), 0.05) > 0.0);
        assert!(field.distance(Vec3::ZERO, 0.05) < 0.0);
    }

    #[test]
    fn a_floor_cuts_the_bottom_flat() {
        let mut body = two_fingers(0.02);
        body.floor = Some((0.0, 0.01));
        let mesh = body.mesh(0.02).unwrap();
        assert!(mesh.bounds().min.y > -0.02);
        assert!(EdgeCensus::of(&mesh).is_closed());
    }

    #[test]
    fn a_mirrored_body_is_the_reflection() {
        let mut body = two_fingers(0.02);
        body.translate(Vec3::new(0.3, 0.0, 0.0));
        let (a, b) = (body.field(), body.mirrored().field());
        for p in [Vec3::new(0.2, 0.3, 0.05), Vec3::new(0.5, 0.1, -0.1), Vec3::new(0.38, 0.5, 0.0)] {
            assert!((a.distance(p, 1.0) - b.distance(Vec3::new(-p.x, p.y, p.z), 1.0)).abs() < 1e-5);
        }
    }

    #[test]
    fn too_fine_a_cell_is_made_coarser_rather_than_refused() {
        let bounds = two_fingers(0.02).field().bounds();
        let grid = grid_for(&bounds, 1e-5).unwrap();
        assert!(grid.sample_count() <= MAX_SAMPLES && grid.cell > 1e-5);
    }
}
