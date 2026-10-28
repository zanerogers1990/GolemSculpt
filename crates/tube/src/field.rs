//! A set of tubes as an analytic signed distance field, and the mesh of it.
//!
//! Every pair of neighbouring stations is a round cone (the hull of the two
//! stations' spheres), so the union of them is exactly the sphere swept along
//! the centre line. Round ends come free, as the end station's sphere; a flat
//! end is a local cut across the tube (see [`TubeField::add_tube`]).
//!
//! Why a field rather than a swept mesh of rings: the union of convex pieces
//! is right in every case a sweep is not. A bend tighter than the tube is
//! thick folds a sweep's inner wall through itself, a tube crossing itself
//! leaves two walls inside the overlap, and a point sized far thicker than
//! its neighbours is a ball no ring can draw. Sampling the field on a lattice
//! and extracting its surface gives one clean, watertight, evenly tessellated
//! mesh whatever was drawn, and it is fast enough (a tube's own volume, not
//! the lattice's) that the live preview is this too, at a coarser cell, so
//! what is edited is what Merge and Keep Separate make.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::Aabb;
use sculpt_volume::{Grid, Surface, grid, sdf};

use crate::types::{CapStyle, Sample};

/// What a [`Lookup`] returns beyond its band, and what a lattice sample no
/// tube reaches holds.
pub const FAR: f32 = sdf::FAR;

/// Most samples a [`TubeField::voxelize`] lattice may hold (4 bytes each):
/// past this the cell grows instead.
pub const MAX_VOXEL_SAMPLES: usize = 64_000_000;

/// Fewest samples a lattice is ever cut down to.
const MIN_VOXEL_SAMPLES: usize = 4096;

/// Most cells a [`Lookup`] spreads its cones over.
const MAX_LOOKUP_CELLS: f64 = 2_000_000.0;

fn sign(v: f32) -> f32 {
    if v > 0.0 {
        1.0
    } else if v < 0.0 {
        -1.0
    } else {
        0.0
    }
}

/// The hull of two spheres, with everything its distance function needs
/// worked out once.
#[derive(Clone, Copy, Debug)]
struct Cone {
    a: Vec3,
    b: Vec3,
    ba: Vec3,
    ra: f32,
    rb: f32,
    /// Squared length of the axis, its inverse, the difference of the radii,
    /// and `l2 - rr * rr`.
    l2: f32,
    il2: f32,
    rr: f32,
    a2: f32,
    /// One sphere holds the other, or the two share a centre: the hull is
    /// just the larger sphere.
    spheres_only: bool,
    min: Vec3,
    max: Vec3,
    /// A flattened cone is a round one in a space stretched along `up`: `a` and `b` above are in that
    /// space, `ends` where they really are. `stretch` is how much is added along `up` (0 for a round
    /// cone) and `flat` the cone's depth as a fraction of its width.
    up: Vec3,
    stretch: f32,
    flat: f32,
    ends: (Vec3, Vec3),
    /// What fills under a muscle's cross-section, down into the model.
    keel: Option<Keel>,
}

/// The fill under a muscle between two stations: straight down from where its cross-section meets
/// the surface it rests on, as wide as the cross-section is there, to below the model. A box swept
/// from one station to the other, its depths and width going linearly between theirs. Where the
/// muscle bends, the boxes either side of a station fan apart below it: each reaches on past a
/// station between two of them (`reach`), so the fan has no gaps.
#[derive(Clone, Copy, Debug)]
struct Keel {
    start: Vec3,
    /// Along the run, its length; the way up square to it, and across.
    axis: Vec3,
    length: f32,
    up: Vec3,
    across: Vec3,
    /// From each end: below the centre line, where the fill starts and where it ends; its half width.
    top: [f32; 2],
    bottom: [f32; 2],
    width: [f32; 2],
    reach: [f32; 2],
}

impl Keel {
    /// Between two stations, if either is filled under. `inner`: whether each is a station between
    /// two runs, which the box reaches on past.
    fn between(a: &Sample, b: &Sample, inner: [bool; 2]) -> Option<Keel> {
        if !(a.under > a.rest || b.under > b.rest) {
            return None;
        }
        let up = (a.up + b.up).normalize_or_zero();
        let run = b.center - a.center;
        let length = run.length();
        let axis = if length > 1e-9 { run / length } else { up.any_orthonormal_vector() };
        let up = (up - axis * up.dot(axis)).normalize_or_zero();
        if up == Vec3::ZERO {
            return None;
        }
        // As wide as the cross-section where it meets the surface: the ellipse's chord there.
        let width = |s: &Sample| {
            let half_depth = (s.radius * s.flat).max(1e-9);
            s.radius * (1.0 - (s.rest / half_depth).clamp(-1.0, 1.0).powi(2)).max(0.0).sqrt()
        };
        let depth = |s: &Sample| (s.rest, s.under.max(s.rest));
        let ((ta, ba), (tb, bb)) = (depth(a), depth(b));
        // A bend of up to about 25 degrees between two runs leaves no gap however far down.
        let overlap = 0.5 * ba.max(bb).max(0.0);
        let reach = inner.map(|inner| if inner { overlap } else { 0.0 });
        Some(Keel {
            start: a.center,
            axis,
            length,
            up,
            across: axis.cross(up),
            top: [ta, tb],
            bottom: [ba, bb],
            width: [width(a), width(b)],
            reach,
        })
    }

    /// Its corners, for a box round it.
    fn corners(&self) -> [Vec3; 8] {
        let mut corners = [Vec3::ZERO; 8];
        for (i, corner) in corners.iter_mut().enumerate() {
            let end = i & 1;
            let along = if end == 0 { -self.reach[0] } else { self.length + self.reach[1] };
            let at = self.start + self.axis * along;
            let below = if i & 2 == 0 { self.top[end] } else { self.bottom[end] };
            let side = if i & 4 == 0 { 1.0 } else { -1.0 };
            *corner = at - self.up * below + self.across * (side * self.width[end]);
        }
        corners
    }

    /// Signed distance, near enough: exact to the box at the nearest place along the run.
    #[inline]
    fn distance(&self, p: Vec3) -> f32 {
        let d = p - self.start;
        let along = d.dot(self.axis);
        let t = if self.length > 1e-9 { (along / self.length).clamp(0.0, 1.0) } else { 0.0 };
        let past = (-along - self.reach[0]).max(along - self.length - self.reach[1]);
        let lerp = |v: [f32; 2]| v[0] + (v[1] - v[0]) * t;
        let (top, bottom, width) = (lerp(self.top), lerp(self.bottom), lerp(self.width));
        let v = d.dot(self.up);
        let x = d.dot(self.across).abs();
        // Down is negative along `up`: the box is from -bottom to -top.
        let qv = (v + 0.5 * (top + bottom)).abs() - 0.5 * (bottom - top);
        let qx = x - width;
        let outside = Vec3::new(qx.max(0.0), qv.max(0.0), past.max(0.0)).length();
        outside + qx.max(qv).max(past).min(0.0)
    }
}

impl Cone {
    /// The hull of two spheres squeezed along `up` to `flat` of its width: an elliptical cross-section,
    /// `ra` and `rb` its half widths.
    fn flattened(a: Vec3, b: Vec3, ra: f32, rb: f32, up: Vec3, flat: f32) -> Cone {
        let round = up == Vec3::ZERO || flat >= 0.999;
        let (up, flat) = if round { (Vec3::ZERO, 1.0) } else { (up, flat.max(0.05)) };
        let stretch = 1.0 / flat - 1.0;
        let ends = (a, b);
        // The round cone holds the flattened one, so its box does too.
        let min = (a - Vec3::splat(ra)).min(b - Vec3::splat(rb));
        let max = (a + Vec3::splat(ra)).max(b + Vec3::splat(rb));
        let (a, b) = (a + up * (stretch * a.dot(up)), b + up * (stretch * b.dot(up)));
        let ba = b - a;
        let l2 = ba.dot(ba);
        let rr = ra - rb;
        let a2 = l2 - rr * rr;
        Cone {
            a,
            b,
            ba,
            ra,
            rb,
            l2,
            il2: if l2 > 0.0 { 1.0 / l2 } else { 0.0 },
            rr,
            a2,
            spheres_only: l2 < 1e-12 || a2 <= 1e-12,
            min,
            max,
            up,
            stretch,
            flat,
            ends,
            keel: None,
        }
    }

    /// The same cone with the fill under it, its box grown to hold that.
    fn with_keel(mut self, keel: Option<Keel>) -> Cone {
        self.keel = keel;
        if let Some(keel) = &keel {
            for corner in keel.corners() {
                self.min = self.min.min(corner);
                self.max = self.max.max(corner);
            }
        }
        self
    }

    /// The same cone with neither end thinner than `floor`, in width or in depth: a flattened one
    /// rounds up where it would be thinner.
    fn thickened(&self, floor: f32) -> Cone {
        let (ra, rb) = (self.ra.max(floor), self.rb.max(floor));
        let flat = self.flat.max(floor / ra.max(rb)).min(1.0);
        Cone::flattened(self.ends.0, self.ends.1, ra, rb, self.up, flat).with_keel(self.keel)
    }

    /// Half the cone's least depth: what a lattice has to resolve.
    fn thinnest(&self) -> f32 {
        self.ra.min(self.rb) * self.flat
    }

    /// Signed distance: exact for a round cone. For a flattened one it is exact along `up` and short
    /// by up to `flat` across it, with the surface itself in the right place.
    #[inline]
    fn distance(&self, p: Vec3) -> f32 {
        let d = if self.stretch == 0.0 {
            self.round_distance(p)
        } else {
            self.round_distance(p + self.up * (self.stretch * p.dot(self.up))) * self.flat
        };
        match &self.keel {
            Some(keel) => d.min(keel.distance(p)),
            None => d,
        }
    }

    /// Exact signed distance to the hull, after Inigo Quilez's region-split
    /// formulation.
    #[inline]
    fn round_distance(&self, p: Vec3) -> f32 {
        let pa = p - self.a;
        if self.spheres_only {
            return (pa.length() - self.ra).min((p - self.b).length() - self.rb);
        }
        let y = pa.dot(self.ba);
        let z = y - self.l2;
        let x = pa * self.l2 - self.ba * y;
        let x2 = x.dot(x);
        let y2 = y * y * self.l2;
        let z2 = z * z * self.l2;
        let k = sign(self.rr) * self.rr * self.rr * x2;
        if sign(z) * self.a2 * z2 > k {
            return (x2 + z2).sqrt() * self.il2 - self.rb;
        }
        if sign(y) * self.a2 * y2 < k {
            return (x2 + y2).sqrt() * self.il2 - self.ra;
        }
        ((x2 * self.a2 * self.il2).sqrt() + y * self.rr) * self.il2 - self.ra
    }
}

/// Flattens the part of the tube that pokes past its end plane: inside the
/// cut's cylinder, the field is `max(field, distance past the plane)`.
#[derive(Clone, Copy, Debug)]
struct Cut {
    center: Vec3,
    axis: Vec3,
    lateral: f32,
    reach: f32,
    min: Vec3,
    max: Vec3,
}

impl Cut {
    /// The same cut for a tube whose end is no thinner than `floor`.
    fn thickened(&self, floor: f32) -> Cut {
        let (lateral, reach) = (self.lateral.max(floor * 1.05), self.reach.max(floor * 1.05));
        let (a, b) = (self.center - self.axis * reach, self.center + self.axis * reach);
        let pad = Vec3::splat(lateral);
        Cut { lateral, reach, min: a.min(b) - pad, max: a.max(b) + pad, ..*self }
    }

    #[inline]
    fn apply(&self, p: Vec3, d: f32) -> f32 {
        let rel = p - self.center;
        let axial = rel.dot(self.axis);
        if axial < -self.reach || axial > self.reach {
            return d;
        }
        if (rel - self.axis * axial).length_squared() > self.lateral * self.lateral {
            return d;
        }
        d.max(axial)
    }
}

/// An inside distance is never stored smaller than the lattice can read the
/// sign of (see `sculpt_volume::sdf`).
#[inline]
fn readable(d: f32) -> f32 {
    if d < 0.0 && d > -sdf::MIN_MAGNITUDE { -sdf::MIN_MAGNITUDE } else { d }
}

/// The union of any number of tubes, in whatever space their stations were
/// given in.
#[derive(Clone, Debug)]
pub struct TubeField {
    cones: Vec<Cone>,
    cuts: Vec<Cut>,
    bounds: Aabb,
    min_radius: f32,
    max_radius: f32,
}

impl Default for TubeField {
    fn default() -> TubeField {
        TubeField::new()
    }
}

/// The mesh of a field, and the lattice it was sampled on.
pub struct Voxelized {
    /// Closed, wound counter-clockwise seen from outside, with a normal per
    /// vertex taken from the field.
    pub surface: Surface,
    /// `grid.cell` is the cell actually used: larger than asked for when the
    /// sample budget stepped in.
    pub grid: Grid,
}

impl TubeField {
    pub fn new() -> TubeField {
        TubeField { cones: Vec::new(), cuts: Vec::new(), bounds: Aabb::EMPTY, min_radius: f32::MAX, max_radius: 0.0 }
    }

    pub fn is_empty(&self) -> bool {
        self.cones.is_empty()
    }

    /// A box round every tube added.
    pub fn bounds(&self) -> Aabb {
        self.bounds
    }

    /// Smallest station radius added: what a lattice has to resolve.
    pub fn min_radius(&self) -> f32 {
        self.min_radius
    }

    pub fn cone_count(&self) -> usize {
        self.cones.len()
    }

    /// Adds one tube copy. Its stations are taken as they are.
    pub fn add_tube(&mut self, samples: &[Sample], caps: CapStyle) {
        let n = samples.len();
        if n < 2 {
            return;
        }
        for (i, pair) in samples.windows(2).enumerate() {
            let up = (pair[0].up + pair[1].up).normalize_or_zero();
            let flat = if up == Vec3::ZERO { 1.0 } else { 0.5 * (pair[0].flat + pair[1].flat) };
            let inner = [i > 0, i + 2 < n];
            let cone = Cone::flattened(pair[0].center, pair[1].center, pair[0].radius, pair[1].radius, up, flat)
                .with_keel(Keel::between(&pair[0], &pair[1], inner));
            self.bounds = Aabb::union(self.bounds, Aabb::new(cone.min, cone.max));
            self.min_radius = self.min_radius.min(cone.thinnest());
            self.max_radius = self.max_radius.max(cone.ra).max(cone.rb);
            self.cones.push(cone);
        }
        if caps == CapStyle::Flat {
            self.add_cut(samples, 0, -samples[0].tangent_out, true);
            self.add_cut(samples, n - 1, samples[n - 1].tangent_in, false);
        }
    }

    /// The cut at one end. Its cylinder has to reach everything past the end
    /// plane (the end station's own sphere, and any sphere just behind it big
    /// enough to poke through, as on a tube that thins toward a flat end) but
    /// no further, or it would cut a tube that curls back past its own end.
    fn add_cut(&mut self, samples: &[Sample], end: usize, outward: Vec3, forward: bool) {
        let center = samples[end].center;
        let mut lateral = samples[end].radius;
        let mut reach = samples[end].radius;
        let walk: Box<dyn Iterator<Item = &Sample>> =
            if forward { Box::new(samples.iter()) } else { Box::new(samples.iter().rev()) };
        for sample in walk {
            let d = sample.center - center;
            let behind = -d.dot(outward);
            let r = sample.radius;
            if behind > r {
                break;
            }
            let side = (d + outward * behind).length();
            lateral = lateral.max(side + r);
            reach = reach.max(r - behind);
        }
        lateral *= 1.05;
        reach *= 1.05;

        // A conservative box: the cylinder from `reach` behind the plane
        // (where the flat face's own distance still matters inside the tube)
        // to `reach` past it.
        let (a, b) = (center - outward * reach, center + outward * reach);
        let pad = Vec3::splat(lateral);
        self.cuts.push(Cut { center, axis: outward, lateral, reach, min: a.min(b) - pad, max: a.max(b) + pad });
    }

    /// The same tubes with no part thinner than `floor`.
    ///
    /// A lattice cannot hold a tube much thinner than its own spacing: the
    /// samples that fall inside come and go along it, and it meshes as a row
    /// of loose droplets. Fattened to one cell it stays one piece, so a
    /// tapered tip rounds off instead of breaking up.
    pub fn thickened(&self, floor: f32) -> TubeField {
        if !(self.min_radius < floor) {
            return self.clone();
        }
        let cones: Vec<Cone> = self.cones.iter().map(|c| c.thickened(floor)).collect();
        let bounds = cones.iter().fold(Aabb::EMPTY, |b, c| Aabb::union(b, Aabb::new(c.min, c.max)));
        TubeField {
            cones,
            cuts: self.cuts.iter().map(|c| c.thickened(floor)).collect(),
            bounds,
            min_radius: floor,
            max_radius: self.max_radius.max(floor),
        }
    }

    /// Signed distance at `p`: exact outside and on the surface, a bound
    /// inside. Tests every cone; for many queries build a [`Lookup`].
    pub fn distance(&self, p: Vec3) -> f32 {
        let d = self.cones.iter().map(|cone| cone.distance(p)).fold(FAR, f32::min);
        if d >= FAR {
            return FAR;
        }
        self.cuts.iter().fold(d, |d, cut| cut.apply(p, d))
    }

    /// Buckets every cone into a uniform lattice of cells, so a query only
    /// tests the cones near its point. `band` is how far from the tubes the
    /// answers still have to be exact; farther out they may be [`FAR`].
    pub fn lookup(&self, band: f32) -> Lookup<'_> {
        let band = band.max(1e-5);
        if self.is_empty() {
            return Lookup { field: self, origin: Vec3::ZERO, inv_cell: 0.0, n: [0; 3], starts: vec![0], items: Vec::new() };
        }
        let region = self.bounds.expanded(band);
        let size = region.size();
        // Never much finer than the tubes are thick: a cone would then be
        // listed in thousands of cells.
        let mut cell = band.max(self.min_radius * 2.0).max(self.max_radius * 0.5).max(1e-4);
        let counts = |cell: f32| (size / cell).ceil().max(Vec3::ONE).as_dvec3();
        for _ in 0..8 {
            let c = counts(cell);
            let total = c.x * c.y * c.z;
            if total <= MAX_LOOKUP_CELLS {
                break;
            }
            cell *= (total / MAX_LOOKUP_CELLS).cbrt() as f32 * 1.01;
        }
        let c = counts(cell);
        let n = [c.x as usize, c.y as usize, c.z as usize];
        let inv_cell = 1.0 / cell;
        let origin = region.min;

        let cell_range = |cone: &Cone| {
            let lo = ((cone.min - Vec3::splat(band) - origin) * inv_cell).floor().max(Vec3::ZERO);
            let hi = ((cone.max + Vec3::splat(band) - origin) * inv_cell).floor().max(Vec3::ZERO);
            let clamp = |v: Vec3| [0, 1, 2].map(|a| (v[a] as usize).clamp(0, n[a] - 1));
            (clamp(lo), clamp(hi))
        };
        let each_cell = |cone: &Cone, visit: &mut dyn FnMut(usize)| {
            let (lo, hi) = cell_range(cone);
            for k in lo[2]..=hi[2] {
                for j in lo[1]..=hi[1] {
                    for i in lo[0]..=hi[0] {
                        visit(i + n[0] * (j + n[1] * k));
                    }
                }
            }
        };

        let total = n[0] * n[1] * n[2];
        let mut starts = vec![0u32; total + 1];
        for cone in &self.cones {
            each_cell(cone, &mut |cell| starts[cell + 1] += 1);
        }
        for cell in 0..total {
            starts[cell + 1] += starts[cell];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; starts[total] as usize];
        for (index, cone) in self.cones.iter().enumerate() {
            each_cell(cone, &mut |cell| {
                items[next[cell] as usize] = index as u32;
                next[cell] += 1;
            });
        }
        Lookup { field: self, origin, inv_cell, n, starts, items }
    }

    /// The field sampled onto `grid`, laid out as `sculpt_volume::sdf` wants
    /// it: negative inside, real distances at every sample within about two
    /// cells of the surface (every corner of every cell the surface passes
    /// through), and [`FAR`] where no tube reaches.
    ///
    /// Each cone writes only the samples in its own box: a tube is thin, so
    /// this costs its volume, not the lattice's. One slab of the lattice to a
    /// task.
    pub fn rasterize(&self, grid: &Grid) -> Vec<f32> {
        let [nx, ny, nz] = grid.n;
        let band = Vec3::splat(grid.cell * 2.0);
        let range = |min: Vec3, max: Vec3| [0, 1, 2].map(|a| grid.span(a, min[a], max[a]));
        let cone_ranges: Vec<[(usize, usize); 3]> = self.cones.iter().map(|c| range(c.min - band, c.max + band)).collect();
        let cut_ranges: Vec<[(usize, usize); 3]> = self.cuts.iter().map(|c| range(c.min, c.max)).collect();
        let cone_slabs = Slabs::build(nz, &cone_ranges);
        let cut_slabs = Slabs::build(nz, &cut_ranges);
        let xs: Vec<f32> = (0..nx).map(|i| grid.coord(0, i)).collect();
        let ys: Vec<f32> = (0..ny).map(|j| grid.coord(1, j)).collect();

        let mut values = vec![FAR; grid.sample_count()];
        values.par_chunks_mut(nx * ny).enumerate().for_each(|(k, slab)| {
            let z = grid.coord(2, k);
            for &c in cone_slabs.of(k) {
                let cone = &self.cones[c as usize];
                let [(i0, i1), (j0, j1), _] = cone_ranges[c as usize];
                for j in j0..j1 {
                    let y = ys[j];
                    for (value, &x) in slab[j * nx + i0..j * nx + i1].iter_mut().zip(&xs[i0..i1]) {
                        let d = cone.distance(Vec3::new(x, y, z));
                        if d < *value {
                            *value = readable(d);
                        }
                    }
                }
            }
            for &c in cut_slabs.of(k) {
                let cut = &self.cuts[c as usize];
                let [(i0, i1), (j0, j1), _] = cut_ranges[c as usize];
                for j in j0..j1 {
                    let y = ys[j];
                    for (value, &x) in slab[j * nx + i0..j * nx + i1].iter_mut().zip(&xs[i0..i1]) {
                        // A sample no cone reached is outside whatever the
                        // cut says.
                        if *value < FAR {
                            *value = readable(cut.apply(Vec3::new(x, y, z), *value));
                        }
                    }
                }
            }
        });
        values
    }

    /// The lattice [`Self::voxelize`] would sample on: spacing `cell`, or
    /// larger if that would take more than `max_samples` samples.
    pub fn grid_for(&self, cell: f32, mirror: [bool; 3], max_samples: usize) -> Result<Grid, String> {
        if self.is_empty() {
            return Err("There is no tube to build.".into());
        }
        let (min, max) = (self.bounds.min.as_dvec3(), self.bounds.max.as_dvec3());
        // Samples along each axis exactly as `Grid::covering` and
        // `Grid::mirrored` will count them.
        let counts = |cell: f64| {
            [0, 1, 2].map(|a| {
                let first = (min[a] / cell - 0.5).floor() - grid::PAD as f64;
                let last = (max[a] / cell - 0.5).ceil() + grid::PAD as f64;
                if mirror[a] { 2.0 * (-first).max(last + 1.0).max(1.0) } else { last - first + 1.0 }
            })
        };
        // Far from the origin a lattice finer than the coordinates
        // themselves is refused; coarsen rather than fail.
        let reach = min.abs().max(max.abs()).max_element();
        let mut cell = (cell as f64).max(1e-7).max(reach / 4.0e6);
        let longest = (grid::MAX_SAMPLES - 8) as f64;
        // However small the budget, a lattice has to have some samples.
        let budget = max_samples.max(MIN_VOXEL_SAMPLES) as f64;
        for _ in 0..12 {
            let n = counts(cell);
            let total = n[0] * n[1] * n[2];
            let worst = n[0].max(n[1]).max(n[2]);
            if total > budget {
                cell *= (total / budget).cbrt() * 1.02;
            } else if worst > longest {
                cell *= worst / longest * 1.02;
            } else {
                break;
            }
        }
        Ok(Grid::covering(&self.bounds, cell as f32)?.mirrored(mirror))
    }

    /// Meshes the field on a lattice of spacing `cell`. A lattice that would
    /// be too large is coarsened to fit `max_samples`; the result says what
    /// was used.
    ///
    /// `mirror` names the coordinate planes the tubes were mirrored across.
    /// Across those the lattice is laid out symmetrically and the sampled
    /// field is made exactly symmetric (each mirrored pair of samples takes
    /// the smaller of the two) before it is meshed. Evaluating the two
    /// copies' cones separately is only symmetric to rounding, and a sample
    /// that lands on the surface can then read +0 on one side and a hair
    /// below zero on the other: one cell meshed two different ways.
    ///
    /// Parts of a tube thinner than a cell are meshed one cell thick (see
    /// [`Self::thickened`]).
    pub fn voxelize(&self, cell: f32, mirror: [bool; 3], max_samples: usize) -> Result<Voxelized, String> {
        let grid = self.grid_for(cell, mirror, max_samples)?;
        let thick;
        let field = if self.min_radius < grid.cell {
            thick = self.thickened(grid.cell);
            &thick
        } else {
            self
        };
        let mut values = field.rasterize(&grid);
        for axis in (0..3).filter(|&a| mirror[a]) {
            sdf::symmetrize(&mut values, &grid, axis)?;
        }
        let surface = sdf::surface_from_sdf(&values, &grid, mirror)
            .map_err(|_| "The tube came out empty: it is thinner than the grid it was built on.".to_string())?;
        Ok(Voxelized { surface, grid })
    }
}

/// For each slab of a lattice, the items whose box reaches it.
struct Slabs {
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Slabs {
    fn build(slabs: usize, ranges: &[[(usize, usize); 3]]) -> Slabs {
        let mut starts = vec![0u32; slabs + 1];
        for range in ranges {
            for k in range[2].0..range[2].1 {
                starts[k + 1] += 1;
            }
        }
        for k in 0..slabs {
            starts[k + 1] += starts[k];
        }
        let mut next = starts.clone();
        let mut items = vec![0u32; starts[slabs] as usize];
        for (index, range) in ranges.iter().enumerate() {
            for k in range[2].0..range[2].1 {
                items[next[k] as usize] = index as u32;
                next[k] += 1;
            }
        }
        Slabs { starts, items }
    }

    fn of(&self, slab: usize) -> &[u32] {
        &self.items[self.starts[slab] as usize..self.starts[slab + 1] as usize]
    }
}

/// A [`TubeField`] with its cones bucketed for point queries. Only read once
/// built, so any number of threads can query it at once.
pub struct Lookup<'a> {
    field: &'a TubeField,
    origin: Vec3,
    inv_cell: f32,
    n: [usize; 3],
    starts: Vec<u32>,
    items: Vec<u32>,
}

impl Lookup<'_> {
    /// Signed distance at `p`: exact outside and on the surface, a bound
    /// inside. Positions more than the band from every cone come back as
    /// [`FAR`].
    pub fn evaluate(&self, p: Vec3) -> f32 {
        let c = (p - self.origin) * self.inv_cell;
        if !(c.x >= 0.0 && c.y >= 0.0 && c.z >= 0.0) {
            return FAR;
        }
        let (i, j, k) = (c.x as usize, c.y as usize, c.z as usize);
        if i >= self.n[0] || j >= self.n[1] || k >= self.n[2] {
            return FAR;
        }
        let cell = i + self.n[0] * (j + self.n[1] * k);
        let cones = &self.items[self.starts[cell] as usize..self.starts[cell + 1] as usize];
        let d = cones.iter().map(|&c| self.field.cones[c as usize].distance(p)).fold(FAR, f32::min);
        if d >= FAR {
            return FAR;
        }
        self.field.cuts.iter().fold(d, |d, cut| cut.apply(p, d))
    }

    /// The field's gradient at `p` by central differences over `probe`, not
    /// normalised (its length is about `2 * probe`). `None` beyond the band.
    pub fn gradient(&self, p: Vec3, probe: f32) -> Option<Vec3> {
        let along = |axis: Vec3| self.evaluate(p + axis * probe) - self.evaluate(p - axis * probe);
        let g = Vec3::new(along(Vec3::X), along(Vec3::Y), along(Vec3::Z));
        let length = g.length();
        (length > 1e-12 && length < 1e5).then_some(g)
    }
}
