//! The limits everything on a lattice shares, and the question asked before
//! any of it is allocated: can this job be afforded?
//!
//! One set of numbers, so that the app cannot ask for something the kernel
//! then refuses in other words, or the other way round. A remesh, a boolean
//! and a tube merge all read them from here, and so does the app.
//!
//! A job costs memory three ways, and only the first was ever counted:
//!
//!   - By the triangles it makes ([`MAX_TRIANGLES`]). A remesh lowers its
//!     resolution to fit, or the app says no; a union is refused beforehand;
//!     a subtract or an intersect cannot know until it has looked, and is
//!     stopped during extraction.
//!   - By the surface it reads. Every place a mesh crosses a lattice line is
//!     kept, sixteen bytes each, for every solid, whatever becomes of it: a
//!     subtract that keeps a sliver of a huge target still scans the whole
//!     target first.
//!   - By the lattice itself. Each solid keeps four bytes for every lattice
//!     line along each axis, and extraction eight for every row of the
//!     lattice it works on. These follow the box the lattice fills rather
//!     than the surface, so they are small for one model at any resolution
//!     and enormous for a union of objects standing far apart, or for a
//!     model far from a mirror plane it is to be made symmetric across: that
//!     lattice has to reach from the model to its mirror image.
//!
//! [`Job::check`] adds up the second and third from the bounds and the
//! triangles alone, and refuses anything over [`MAX_BYTES`] in words that say
//! what to do about it, before a byte of it is asked for.
//!
//! What is added up is the most the job holds at any one moment, not what
//! is left standing when its scans are done: see [`PEAK_PER_KEPT`]. The
//! meshes themselves are not in it, neither those that go in nor the one
//! that comes out.
//!
//! A refusal offers the highest resolution that would have done, and that
//! number is found by asking the very question the caller will ask again
//! ([`plan`], [`highest_passing`]): what is offered is never refused in its
//! turn.

use glam::Vec3;
use rayon::prelude::*;
use sculpt_geom::Aabb;

use crate::grid::{Grid, MAX_SAMPLES, PAD};

/// Fewest and most cells along the model's longest side. Requests outside
/// the range are brought into it rather than refused.
pub const MIN_RESOLUTION: u32 = 4;
pub const MAX_RESOLUTION: u32 = 4096;

/// Most triangles a remesh or a boolean makes. This, not the resolution, is
/// the real ceiling on the result: resolution counts cells along the longest
/// side only, so one number gives wildly different triangle counts by shape
/// (a slender worm came out at 681k where a compact model reached 10M).
///
/// The app's Remesh has allowed 80 million since it was written, and the
/// kernel's own figure was half that, so a boolean was refused at a size a
/// remesh of the same object was not. One figure now, the larger, so that
/// nothing that worked is refused.
pub const MAX_TRIANGLES: usize = 80_000_000;

/// Most memory one job may hold at once for its crossings and its per-line
/// and per-row tables (see the module remarks), not counting the mesh that
/// comes out, which [`MAX_TRIANGLES`] bounds.
///
/// A fixed figure rather than a share of the machine's memory, so that the
/// same file behaves alike everywhere. At the 1024 the app's sliders stop
/// at, everything the triangle limits allow on a model that fills its
/// lattice is within it: a union of eight solids at twice the triangle
/// budget is reckoned at 3.6 GiB, nearly all of it crossings, and an
/// ordinary model at a small fraction of that. What it stops is the lattice
/// that fills with empty space, and the surface too large to read.
///
/// At the kernel's own highest resolution (4096, four times where the
/// app's sliders stop) a remesh at the triangle budget is 2.2 GiB. A union
/// of eight solids at twice that budget there is over the limit and
/// refused; it was let through when the figure counted only what a scan
/// keeps, and held half as much again as the limit while it ran.
pub const MAX_BYTES: u64 = 4 << 30;

/// One `u32` per lattice line, in each solid's scan.
const BYTES_PER_LINE: u64 = 4;
/// A start and a count, each a `u32`, per row of cells in extraction.
const BYTES_PER_ROW: u64 = 8;
/// A start kept and a count while it is being made, each a `u32`, per row
/// of samples of each solid whose inside has to be put to the vote of the
/// axes (`scan/vote.rs`). Whether it will be is only known once the solid
/// has been read, so every solid is reckoned as if it were.
const VOTE_BYTES_PER_ROW: u64 = 8;
/// One crossing, as a scan keeps it.
const BYTES_PER_CROSSING: f64 = 16.0;

/// How much a job holds at its worst for every byte of crossings its scans
/// keep. Measured with a counting allocator (`tests/peak_memory.rs`, which
/// holds this figure to what it finds): the most held at once, less the
/// lattice tables counted beside the crossings and the mesh handed back,
/// over the bytes of crossings the budget expected to be kept.
///
///   - 1.0 while a closed solid is being read, 1.3 for one with a hole. Its
///     crossings are found in pieces and joined an axis at a time, so a
///     third of them is held twice. (It was 2.1 to 2.3, and 3.6 with a
///     hole, when all three axes were joined at once out of pieces with
///     room to spare and a mesh with holes was read a second time with the
///     first reading still held.)
///   - 1.7 during the extraction of a closed model: the scans as kept, and
///     the tables of cells the surface is built through.
///   - 1.9 for a mesh torn or wound inside out, whose inside is voted on.
///   - 2.1 for a bowl, half a sphere capped across its rim: the lid is read
///     with the mesh, a third more surface than the budget was shown.
///
/// A lid larger than the mesh it closes (a shallow dish) is beyond it, and
/// is the one thing here that is not known before the mesh is read.
const PEAK_PER_KEPT: f64 = 2.5;

/// How the kernel speaks of what it is working on. The app has the object's
/// name, and puts it in place of `named` at the start of a message.
#[derive(Clone, Copy, Debug)]
pub struct Subject {
    /// At the head of a sentence: "The model".
    pub named: &'static str,
    /// Within one: "this model".
    pub this: &'static str,
}

/// What a remesh works on.
pub const MODEL: Subject = Subject { named: "The model", this: "this model" };
/// The object a boolean keeps, whose longest side the resolution counts along.
pub const FIRST_OBJECT: Subject = Subject { named: "The first object", this: "these objects" };

/// What is about to be put on a lattice.
#[derive(Clone, Copy, Debug)]
pub struct Job {
    /// What the lattice has to cover.
    pub bounds: Aabb,
    /// Spacing of its samples.
    pub cell: f32,
    /// The coordinate planes the result is to be symmetric across.
    pub mirror: [bool; 3],
    /// Meshes scanned onto it.
    pub solids: usize,
    /// How many times, all told, their surfaces cross its lines: the sum of
    /// [`crossings`] over them.
    pub crossings: f64,
}

/// Why a job cannot be afforded.
#[derive(Clone, Debug, PartialEq)]
pub enum Refusal {
    /// Not a matter of size: the bounds or the cell make no lattice at all.
    /// In [`Grid`]'s own words.
    Grid(String),
    /// It would fit but for the mirror: the meshes lie far to one side of
    /// these planes, and the lattice has to reach to their mirror image.
    OffCentre {
        axes: [bool; 3],
        /// How far from the plane their middle is, in their own longest sides.
        times: f32,
        bytes: u64,
    },
    /// Too much, mirror or no mirror.
    TooBig { bytes: u64 },
}

/// A refusal, with the way out that a number can give.
#[derive(Clone, Debug, PartialEq)]
pub struct Refused {
    pub why: Refusal,
    /// The highest resolution at which the same job could be afforded, when
    /// there is one below the resolution asked for.
    pub fits: Option<u32>,
}

/// What a job comes to.
struct Cost {
    /// First sample and sample count on each axis: round the meshes alone,
    /// and widened across the mirror planes, which is the lattice used.
    narrow: [(i64, i64); 3],
    wide: [(i64, i64); 3],
    bytes: u64,
}

fn counts(reach: &[(i64, i64); 3]) -> [u64; 3] {
    reach.map(|(_, count)| count.max(0) as u64)
}

/// Lattice lines along all three axes: the three faces of the lattice's box.
fn lines([nx, ny, nz]: [u64; 3]) -> u64 {
    nx.saturating_mul(ny).saturating_add(ny.saturating_mul(nz)).saturating_add(nz.saturating_mul(nx))
}

/// The memory one scan's per-line tables take on `grid`.
pub(crate) fn scan_table_bytes(grid: &Grid) -> u64 {
    lines(grid.n.map(|n| n as u64)).saturating_mul(BYTES_PER_LINE)
}

/// The memory extraction's per-row tables take on `grid`.
pub(crate) fn row_table_bytes(grid: &Grid) -> u64 {
    (grid.n[1] as u64).saturating_mul(grid.n[2] as u64).saturating_mul(BYTES_PER_ROW)
}

impl Cost {
    fn affordable(&self) -> bool {
        counts(&self.wide).iter().all(|&n| n <= MAX_SAMPLES as u64) && self.bytes <= MAX_BYTES
    }

    fn grid(&self, cell: f32) -> Grid {
        Grid { cell, lo: self.wide.map(|(first, _)| first as i32), n: counts(&self.wide).map(|n| n as usize) }
    }
}

impl Job {
    fn cost(&self) -> Result<Cost, Refusal> {
        let narrow = Grid::reach(&self.bounds, self.cell, PAD).map_err(Refusal::Grid)?;
        let mut wide = narrow;
        for a in (0..3).filter(|&a| self.mirror[a]) {
            wide[a] = Grid::mirrored_reach(narrow[a]);
        }
        let [_, wy, wz] = counts(&wide);
        // A count that is not a number, or is beyond counting, is beyond affording.
        let countable = self.crossings.is_finite();
        let held = self.crossings.max(0.0) * BYTES_PER_CROSSING * PEAK_PER_KEPT;
        // A float too large for the integer becomes the largest there is.
        let surface = if countable { held as u64 } else { u64::MAX };
        let rows = wy.saturating_mul(wz);
        let scan = BYTES_PER_LINE.saturating_mul(lines(counts(&wide)));
        let per_solid = scan.saturating_add(rows.saturating_mul(VOTE_BYTES_PER_ROW));
        let tables = (self.solids as u64).saturating_mul(per_solid);
        let extraction = rows.saturating_mul(BYTES_PER_ROW);
        Ok(Cost { narrow, wide, bytes: tables.saturating_add(extraction).saturating_add(surface) })
    }

    /// What the job is reckoned to hold at its worst: the figure [`Self::check`]
    /// holds against [`MAX_BYTES`]. `None` for a job with no lattice at all.
    pub fn bytes(&self) -> Option<u64> {
        self.cost().ok().map(|cost| cost.bytes)
    }

    /// The lattice for the job, covering the bounds and its own mirror image
    /// across the mirror planes, or why the job cannot be afforded. Nothing
    /// is allocated either way.
    pub fn check(&self) -> Result<Grid, Refusal> {
        let cost = self.cost()?;
        if cost.affordable() {
            return Ok(cost.grid(self.cell));
        }
        // Affordable with the mirror off? Then the mirror is the reason, and
        // the planes to blame are those the lattice grows by half or more
        // across: the meshes are mostly or wholly to one side of them.
        let plain = Job { mirror: [false; 3], ..*self }.cost()?;
        let (narrow, wide) = (counts(&cost.narrow), counts(&cost.wide));
        let half_again = |a: usize| wide[a].saturating_mul(2) >= narrow[a].saturating_mul(3);
        let axes = [0, 1, 2].map(|a| self.mirror[a] && half_again(a));
        if plain.affordable() && axes.contains(&true) {
            let middle = (self.bounds.min + self.bounds.max) * 0.5;
            let longest = self.bounds.size().max_element().max(f32::MIN_POSITIVE);
            let times = (0..3).filter(|&a| axes[a]).map(|a| middle[a].abs() / longest).fold(0.0, f32::max);
            return Err(Refusal::OffCentre { axes, times, bytes: cost.bytes });
        }
        Err(Refusal::TooBig { bytes: cost.bytes })
    }
}

/// Checks the job `job_at` makes of `resolution`, and returns its lattice.
/// A refusal carries the highest resolution below it that would have done.
///
/// `job_at` is the job as the caller would put it at any resolution, worked
/// out afresh each time, crossings and all. Scaling one job's crossings by
/// the square of the resolution instead is near enough for a surface inside
/// the lattice and wrong for one that reaches beyond it, which is read only
/// where the lattice is: the number offered was then refused in its turn.
pub fn plan(resolution: u32, job_at: impl Fn(u32) -> Job) -> Result<Grid, Refused> {
    job_at(resolution).check().map_err(|why| {
        let fits = highest_passing(resolution.saturating_sub(1), |lower| job_at(lower).check().is_ok());
        Refused { why, fits }
    })
}

/// The highest resolution, no higher than `at_most`, that `passes`: for
/// offering in place of one that did not. It is looked for as if whatever
/// passes at one resolution passes at every lower one, but nothing is taken
/// on trust: the resolution returned is one that was tried, and passed.
pub fn highest_passing(at_most: u32, passes: impl Fn(u32) -> bool) -> Option<u32> {
    // Written as a chain, not with `min` (crates/README.md rule 9).
    let top = if at_most > MAX_RESOLUTION {
        MAX_RESOLUTION
    } else if at_most < MIN_RESOLUTION {
        return None;
    } else {
        at_most
    };
    if !passes(MIN_RESOLUTION) {
        return None;
    }
    if passes(top) {
        return Some(top);
    }
    // `fits` passed and `above` did not; close the gap between them.
    let (mut fits, mut above) = (MIN_RESOLUTION, top);
    while above - fits > 1 {
        let middle = fits + (above - fits) / 2;
        if passes(middle) {
            fits = middle;
        } else {
            above = middle;
        }
    }
    Some(fits)
}

/// The way out for one object that is far from, or behind, its own mirror
/// plane, besides turning the mirror off: the app's Set Origin to Mesh (in
/// its Objects card) puts the object's centre, and with it the plane, in the
/// middle of the object's mesh. After that the lattice need reach no further
/// than the object itself.
pub const RECENTRE: &str = "move its centre onto it with Set Origin to Mesh";

/// "X", "X and Y", "X, Y and Z".
fn axis_names(axes: [bool; 3]) -> String {
    let names: Vec<&str> = (0..3).filter(|&a| axes[a]).map(|a| ["X", "Y", "Z"][a]).collect();
    match names.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => names.concat(),
    }
}

/// The mirrors of those axes, as the thing to turn off: "X mirror", "X and Y mirrors".
pub(crate) fn mirrors_named(axes: [bool; 3]) -> String {
    let plural = if axes.iter().filter(|&&on| on).count() == 1 { "" } else { "s" };
    format!("{} mirror{plural}", axis_names(axes))
}

impl Refused {
    /// The refusal in words for whoever asked, saying what to do about it.
    pub fn message(&self, subject: &Subject) -> String {
        let lower = self.fits.map(|fits| format!("lower the resolution to {fits} or less"));
        match &self.why {
            Refusal::Grid(error) => error.clone(),
            Refusal::OffCentre { axes, times, .. } => {
                let (names, mirrors) = (axis_names(*axes), mirrors_named(*axes));
                let planes = if axes.iter().filter(|&&on| on).count() == 1 { "plane" } else { "planes" };
                let place = if *times >= 1.5 {
                    format!("is about {times:.0} times its own size away from its {names} mirror {planes}")
                } else {
                    format!("lies to one side of its {names} mirror {planes}")
                };
                let (how_big, lower) = match lower {
                    Some(lower) => ("too big at this resolution", format!(", {lower}")),
                    None => ("too big at any resolution", String::new()),
                };
                format!(
                    "{} {place}. Rebuilt mirrored, the grid has to reach from it to its mirror image, which is \
                     {how_big}. Turn the {mirrors} off for this{lower}, or {RECENTRE}.",
                    subject.named
                )
            }
            Refusal::TooBig { .. } => {
                let this = subject.this;
                match self.fits {
                    Some(fits) => format!("That resolution needs too much memory for {this}. Try {fits} or lower."),
                    None => format!("Rebuilding {this} needs too much memory at any resolution."),
                }
            }
        }
    }
}

/// How many times a mesh's surface crosses the lines of a lattice of spacing
/// `cell` that covers `bounds` and, across the `mirror` planes, its mirror
/// image, near enough: what its scan will have to keep.
///
/// Lines along an axis cross a triangle once for every lattice point under
/// its shadow on the plane across that axis, so the count is the shadow's
/// area in cells, summed over the three axes: between one and 1.73 times the
/// surface area, by which way the surface faces. A triangle reaching beyond
/// the lattice sideways is only crossed by the lines there are, so its shadow
/// counts no further than the lattice goes (a cutter ten times the size of
/// what it cuts costs what the part inside costs).
///
/// The mesh must have been through `check_mesh`.
pub fn crossings(positions: &[Vec3], indices: &[u32], bounds: &Aabb, mirror: [bool; 3], cell: f32) -> f64 {
    let margin = (PAD + 1) as f64 * cell as f64;
    let (mut low, mut high) = (bounds.min.as_dvec3() - margin, bounds.max.as_dvec3() + margin);
    // As far to the other side of a mirror plane as to this one.
    for axis in (0..3).filter(|&axis| mirror[axis]) {
        let reach = low[axis].abs().max(high[axis].abs());
        (low[axis], high[axis]) = (-reach, reach);
    }
    let shadows: f64 = indices
        .par_chunks_exact(3)
        .map(|t| {
            let [a, b, c] = [t[0], t[1], t[2]].map(|i| positions[i as usize].as_dvec3());
            let across = (b - a).cross(c - a) * 0.5;
            let (least, most) = (a.min(b).min(c), a.max(b).max(c));
            let inside = |axis: usize| (most[axis].min(high[axis]) - least[axis].max(low[axis])).max(0.0);
            (0..3).map(|axis| across[axis].abs().min(inside((axis + 1) % 3) * inside((axis + 2) % 3))).sum::<f64>()
        })
        .sum();
    shadows / (cell as f64 * cell as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scan::Scan;
    use crate::testing::{box_mesh, icosphere};

    fn unit_box() -> Aabb {
        Aabb::new(Vec3::splat(-0.5), Vec3::splat(0.5))
    }

    fn job(bounds: Aabb, resolution: u32, mirror: [bool; 3]) -> Job {
        let cell = bounds.size().max_element() / resolution as f32;
        Job { bounds, cell, mirror, solids: 1, crossings: 0.0 }
    }

    /// [`plan`] for a job with no surface to speak of, on bounds a unit long.
    fn planned(bounds: Aabb, solids: usize, resolution: u32, mirror: [bool; 3]) -> Result<Grid, Refused> {
        plan(resolution, |resolution| Job { solids, cell: 1.0 / resolution as f32, ..job(bounds, 1, mirror) })
    }

    /// Whether the same job is let through at another resolution.
    fn passes(bounds: Aabb, solids: usize, resolution: u32, mirror: [bool; 3]) -> bool {
        Job { solids, cell: 1.0 / resolution as f32, ..job(bounds, 1, mirror) }.check().is_ok()
    }

    /// The other limits' own worst case on a model that fills its lattice,
    /// up to the 1024 the app's sliders stop at: a union of eight solids at
    /// twice the triangle budget, mirrored on every axis. And at the highest
    /// resolution there is, a remesh at the triangle budget.
    #[test]
    fn everything_the_other_limits_allow_on_a_centred_model_is_affordable() {
        // A union is refused beforehand past three triangles per unit of area
        // in cells; crossings are at most 1.74 per unit.
        let most_crossings = 2.0 * MAX_TRIANGLES as f64 / 3.0 * 1.74;
        for resolution in [4, 128, 1024] {
            for mirror in [[false; 3], [true, false, false], [true; 3]] {
                let heavy = Job { solids: 8, crossings: most_crossings, ..job(unit_box(), resolution, mirror) };
                let grid = heavy.check().unwrap_or_else(|why| panic!("{resolution}, {mirror:?}: {why:?}"));
                // The lattice of a centred model is its own mirror image as it stands.
                assert_eq!(grid, Grid::covering(&unit_box(), heavy.cell).unwrap().mirrored(mirror));
            }
        }
        let gib = |job: Job| job.bytes().unwrap() as f64 / (1u64 << 30) as f64;
        let at_the_slider = Job { solids: 8, crossings: most_crossings, ..job(unit_box(), 1024, [true; 3]) };
        assert!((3.5..3.7).contains(&gib(at_the_slider)), "{} GiB", gib(at_the_slider));
        // Without a surface to read, the tables of the app's highest setting are nothing much.
        let tables = Job { solids: 8, ..job(unit_box(), 1024, [true; 3]) };
        assert!(tables.bytes().unwrap() < 256 << 20, "{} bytes", tables.bytes().unwrap());

        // The kernel's own highest resolution: one model at the triangle budget is rebuilt, mirrored or not.
        let remesh = Job { crossings: most_crossings / 2.0, ..job(unit_box(), MAX_RESOLUTION, [true; 3]) };
        assert!(remesh.check().is_ok());
        assert!((2.1..2.3).contains(&gib(remesh)), "{} GiB", gib(remesh));
        // The union above is not: it would hold more than the limit while it ran, and did.
        let union = Job { solids: 8, crossings: most_crossings, ..job(unit_box(), MAX_RESOLUTION, [true; 3]) };
        assert!(matches!(union.check(), Err(Refusal::TooBig { .. })), "{} GiB", gib(union));
    }

    /// What is held against the limit is the most a job holds at once, which
    /// is more than its scans keep: every crossing counts for more than the
    /// sixteen bytes it is stored in, and every solid for the tables a vote
    /// of the axes would need, since any of them may turn out torn.
    #[test]
    fn the_figure_is_the_most_held_not_what_is_kept() {
        let grid = Grid::covering(&unit_box(), 1.0 / 100.0).unwrap();
        let [nx, ny, nz] = grid.n.map(|n| n as u64);
        let empty = Job { solids: 3, ..job(unit_box(), 100, [false; 3]) };
        let tables = 3 * (4 * (nx * ny + ny * nz + nz * nx) + 8 * ny * nz) + 8 * ny * nz;
        assert_eq!(empty.bytes(), Some(tables));
        assert_eq!(tables, 3 * (scan_table_bytes(&grid) + 8 * ny * nz) + row_table_bytes(&grid));
        let read = Job { crossings: 1e6, ..empty };
        assert_eq!(read.bytes(), Some(tables + 40_000_000));
        assert_eq!(Job { bounds: Aabb::EMPTY, ..read }.bytes(), None);
    }

    /// A model that only touches its mirror planes, as half a figure does:
    /// the lattice doubles on each axis and is still nothing much.
    #[test]
    fn a_model_beside_its_mirror_planes_is_affordable() {
        let octant = Aabb::new(Vec3::ZERO, Vec3::ONE);
        let beside = job(octant, 1024, [true, false, false]);
        assert_eq!(beside.check().unwrap(), Grid::covering(&octant, beside.cell).unwrap().mirrored(beside.mirror));
        assert!(job(octant, 1024, [true; 3]).check().is_ok());
    }

    /// A model the size of a rivet, a long way off two of its mirror planes
    /// at the app's highest resolution. Its lattice has 53,254 x 53,254 lines
    /// along z, four bytes each: 11.3 GB for the one table, which used to be
    /// asked for. It is refused from the numbers alone.
    #[test]
    fn a_model_far_from_two_mirror_planes_is_refused_before_anything_is_allocated() {
        let rivet = Aabb::new(Vec3::new(25.0, 25.0, -0.5), Vec3::new(26.0, 26.0, 0.5));
        let mirrored = job(rivet, 1024, [true, true, false]);
        let cost = mirrored.cost().unwrap();
        assert_eq!(counts(&cost.wide), [53_254, 53_254, 1_030]);
        assert_eq!(counts(&cost.narrow), [1_030; 3]);
        assert!(cost.bytes > 11 << 30, "{} bytes", cost.bytes);
        assert_eq!(mirrored.bytes(), Some(cost.bytes));

        let refused = planned(rivet, 1, 1024, [true, true, false]).unwrap_err();
        let Refusal::OffCentre { axes, times, .. } = refused.why else { panic!("{:?}", refused.why) };
        assert_eq!(axes, [true, true, false]);
        assert!((times - 25.5).abs() < 0.01, "{times}");
        // The resolution offered is one that passes, and the next one up does not.
        let fits = refused.fits.expect("a lower resolution does it");
        assert!(passes(rivet, 1, fits, [true, true, false]));
        assert!(!passes(rivet, 1, fits + 1, [true, true, false]));
        assert!((500..700).contains(&fits), "{fits}");
        assert_eq!(
            refused.message(&MODEL),
            format!(
                "The model is about 26 times its own size away from its X and Y mirror planes. Rebuilt mirrored, \
                 the grid has to reach from it to its mirror image, which is too big at this resolution. Turn the \
                 X and Y mirrors off for this, lower the resolution to {fits} or less, or {RECENTRE}."
            )
        );
        // With the mirrors off it is an ordinary model.
        assert!(job(rivet, 1024, [false; 3]).check().is_ok());
    }

    /// Off one plane only, the lattice is long and thin: no table is large,
    /// until it is longer than a lattice may be.
    #[test]
    fn a_model_far_from_one_mirror_plane_is_affordable_until_its_lattice_is_too_long() {
        let aside = |x: f32| Aabb::new(Vec3::new(x, -0.5, -0.5), Vec3::new(x + 1.0, 0.5, 0.5));
        let near = job(aside(20.0), 1024, [true, false, false]);
        let grid = near.check().unwrap();
        assert_eq!(grid, Grid::covering(&aside(20.0), near.cell).unwrap().mirrored(near.mirror));
        assert_eq!(grid.n, [43_014, 1_030, 1_030]);
        assert!(near.cost().unwrap().bytes < 512 << 20);

        let refused = planned(aside(40.0), 1, 1024, [true, false, false]).unwrap_err();
        assert!(matches!(refused.why, Refusal::OffCentre { axes: [true, false, false], .. }), "{:?}", refused.why);
        let fits = refused.fits.unwrap();
        assert_eq!(
            refused.message(&FIRST_OBJECT),
            format!(
                "The first object is about 40 times its own size away from its X mirror plane. Rebuilt mirrored, \
                 the grid has to reach from it to its mirror image, which is too big at this resolution. Turn the \
                 X mirror off for this, lower the resolution to {fits} or less, or move its centre onto it with Set \
                 Origin to Mesh."
            )
        );
        // So far off that no resolution helps: the mirror is left to turn off, or the centre to move.
        let refused = planned(aside(20_000.0), 1, 64, [true, false, false]).unwrap_err();
        assert_eq!(refused.fits, None);
        let message = refused.message(&MODEL);
        let end = format!("which is too big at any resolution. Turn the X mirror off for this, or {RECENTRE}.");
        assert!(message.ends_with(&end), "{message}");
    }

    /// Two objects a long way apart on two axes, to be united: with no mirror
    /// anywhere the lattice between them is as large. 51,206 x 51,206 lines
    /// along z for each of the two solids was 21 GB.
    #[test]
    fn a_lattice_spread_over_empty_space_is_refused() {
        let spread = Aabb::new(Vec3::splat(-0.5), Vec3::new(49.5, 49.5, 0.5));
        let refused = planned(spread, 2, 1024, [false; 3]).unwrap_err();
        let Refusal::TooBig { bytes } = refused.why else { panic!("{:?}", refused.why) };
        assert!(bytes > 20 << 30, "{bytes} bytes");
        let fits = refused.fits.unwrap();
        assert_eq!(
            refused.message(&FIRST_OBJECT),
            format!("That resolution needs too much memory for these objects. Try {fits} or lower.")
        );
        assert!(passes(spread, 2, fits, [false; 3]) && !passes(spread, 2, fits + 1, [false; 3]));
    }

    /// The surface read counts as well as the lattice: forty bytes held, at
    /// the worst, for each crossing kept in sixteen.
    #[test]
    fn a_surface_too_large_to_read_is_refused() {
        let affordable = (MAX_BYTES / 40) as f64;
        let within = Job { crossings: affordable * 0.9, ..job(unit_box(), 1024, [false; 3]) };
        assert!(within.check().is_ok());
        // A surface inside the lattice: its crossings go with the square of the resolution.
        let beyond = |resolution: u32| {
            let crossings = affordable * 4.0 * (resolution as f64 / 1024.0).powi(2);
            Job { crossings, ..job(unit_box(), resolution, [false; 3]) }
        };
        let refused = plan(1024, beyond).unwrap_err();
        assert!(matches!(refused.why, Refusal::TooBig { .. }));
        // A little under half the resolution, then.
        let fits = refused.fits.unwrap();
        assert!((480..512).contains(&fits), "{fits}");
        assert!(beyond(fits).check().is_ok() && beyond(fits + 1).check().is_err());
        let said = format!("That resolution needs too much memory for this model. Try {fits} or lower.");
        assert_eq!(refused.message(&MODEL), said);
    }

    /// The resolution offered is one that was tried and passed, however the
    /// job's cost goes with resolution. Here it does not fall with the square
    /// of it, as a cutter larger than the lattice does not.
    #[test]
    fn the_resolution_offered_is_one_that_passes() {
        for (floor, per_line) in [(0.0, 300.0), (1e7, 250.0), (9e7, 40.0), (1.07e8, 0.01), (2e8, 1.0)] {
            // So many crossings whatever the resolution, and so many more for every line across the lattice.
            let job_at = |resolution: u32| {
                let lines = (resolution as f64 + 6.0).powi(2);
                Job { crossings: floor + per_line * lines, ..job(unit_box(), resolution, [false; 3]) }
            };
            for resolution in [MAX_RESOLUTION, 3000, 2019, 1024, 100, 5, 4] {
                let Err(refused) = plan(resolution, job_at) else { continue };
                match refused.fits {
                    Some(fits) => {
                        assert!(fits < resolution && job_at(fits).check().is_ok(), "{floor}, {per_line}: {fits}");
                        assert!(plan(fits, job_at).is_ok());
                        assert!(fits + 1 == resolution || job_at(fits + 1).check().is_err());
                    }
                    None => assert!(resolution == MIN_RESOLUTION || job_at(MIN_RESOLUTION).check().is_err()),
                }
            }
        }
        // Passing here and there only: what comes back is still one that passed, or nothing.
        let odd = highest_passing(1000, |resolution| resolution % 2 == 0 && resolution < 700);
        assert!(odd.is_some_and(|fits| fits % 2 == 0 && fits < 700), "{odd:?}");
        assert_eq!(highest_passing(1000, |resolution| resolution > 500), None);
        assert_eq!(highest_passing(1000, |_| true), Some(1000));
    }

    /// The sums themselves cannot overflow, whatever is asked. The lattice
    /// here is one that is let through with nothing on it, so each of these
    /// is refused for its own figure and no other: a sum that wrapped round
    /// would come to little, and pass.
    #[test]
    fn absurd_jobs_are_refused_not_overflowed() {
        let plain = job(unit_box(), 64, [true; 3]);
        assert!(plain.check().is_ok());
        assert!(Job { solids: 8, ..plain }.check().is_ok());
        for crossings in [1e300, f64::MAX, f64::INFINITY, f64::NAN, u64::MAX as f64, u64::MAX as f64 / 39.0] {
            let absurd = Job { crossings, ..plain };
            assert!(matches!(absurd.check(), Err(Refusal::TooBig { .. })), "{crossings} crossings");
            assert_eq!(absurd.bytes(), Some(u64::MAX), "{crossings} crossings");
        }
        // Tables for more solids than there are bytes. And tables and a surface that can each be counted, each
        // a little over half of what a u64 holds: it is their sum that cannot.
        let per_solid = plain.bytes().unwrap() - Job { solids: 0, ..plain }.bytes().unwrap();
        let half = (u64::MAX / 2 / per_solid) as usize + 1;
        for (solids, crossings) in [(usize::MAX, 0.0), (usize::MAX / 2, 0.0), (half, u64::MAX as f64 / 79.0)] {
            let absurd = Job { solids, crossings, ..plain };
            assert!(Job { solids: 1, ..absurd }.bytes().unwrap() < u64::MAX, "the surface alone can be counted");
            assert!(Job { crossings: 0.0, ..absurd }.bytes().unwrap() > u64::MAX / 2, "{solids} solids");
            assert!(matches!(absurd.check(), Err(Refusal::TooBig { .. })), "{solids} solids, {crossings} crossings");
            assert_eq!(absurd.bytes(), Some(u64::MAX), "{solids} solids, {crossings} crossings");
        }
        // Fewer than none is none.
        assert_eq!(Job { crossings: -1.0, ..plain }.bytes(), plain.bytes());
        assert_eq!(Job { crossings: f64::NEG_INFINITY, ..plain }.bytes(), Some(u64::MAX));
        // And a lattice too long to count on is refused whatever is on it.
        let huge = Aabb::new(Vec3::splat(-4000.0), Vec3::splat(4000.0));
        let endless = Job { cell: 0.001, ..job(huge, 4, [true; 3]) };
        assert!(matches!(endless.check(), Err(Refusal::TooBig { .. })));
        // Nonsense is Grid's to refuse, in its words.
        let flat = job(unit_box(), 64, [false; 3]);
        assert_eq!(Job { cell: 0.0, ..flat }.check(), Err(Refusal::Grid("The cell size must be more than zero.".into())));
        assert!(matches!(Job { bounds: Aabb::EMPTY, ..flat }.check(), Err(Refusal::Grid(_))));
        let message = Refused { why: Refusal::Grid("The model has no size.".into()), fits: None }.message(&MODEL);
        assert_eq!(message, "The model has no size.");
    }

    /// The lowest resolutions there are (crates/README.md rule 9).
    #[test]
    fn the_resolution_that_fits_is_never_below_the_lowest() {
        let far = Aabb::new(Vec3::new(40.0, -0.5, -0.5), Vec3::new(41.0, 0.5, 0.5));
        let mirror = [true, false, false];
        // What a refusal at `resolution` would offer: the highest below it that passes.
        let lower = |resolution: u32| resolution.saturating_sub(1);
        let offered = |resolution: u32| highest_passing(lower(resolution), |lower| passes(far, 1, lower, mirror));
        for resolution in [0, 1, 2, 3, 4] {
            assert_eq!(offered(resolution), None, "asked for {resolution}");
        }
        assert_eq!(offered(5), Some(4));
        assert_eq!(offered(6), Some(5));
        // Asked for more than there is, the answer is still within the range.
        let fits = offered(u32::MAX).unwrap();
        assert!((MIN_RESOLUTION..MAX_RESOLUTION).contains(&fits), "{fits}");
        // And when everything passes, the most there is, or as much as was asked.
        let highest = [(0, None), (3, None), (4, Some(4)), (5, Some(5)), (4096, Some(4096)), (4097, Some(4096))];
        for (at_most, highest) in highest {
            assert_eq!(highest_passing(at_most, |_| true), highest, "at most {at_most}");
        }
        assert_eq!(highest_passing(u32::MAX, |_| true), Some(MAX_RESOLUTION));
        assert_eq!(highest_passing(u32::MAX, |_| false), None);
    }

    #[test]
    fn mirror_planes_are_named_as_a_person_would() {
        assert_eq!(axis_names([true, false, false]), "X");
        assert_eq!(axis_names([false, true, true]), "Y and Z");
        assert_eq!(axis_names([true, true, true]), "X, Y and Z");
        assert_eq!(axis_names([false; 3]), "");
        // A thin model a little way off its plane is not "0 times its own size" from it.
        let why = Refusal::OffCentre { axes: [false, false, true], times: 0.3, bytes: 0 };
        let message = Refused { why, fits: Some(100) }.message(&MODEL);
        assert!(message.starts_with("The model lies to one side of its Z mirror plane. "), "{message}");
    }

    /// The estimate against what a scan really keeps.
    #[test]
    fn crossings_are_counted_as_a_scan_will_find_them() {
        let sphere = icosphere(5, 0.5, Vec3::new(0.01, 0.02, -0.01));
        let cube = box_mesh(Vec3::splat(-0.5), Vec3::splat(0.5));
        for (mesh, name) in [(&sphere, "sphere"), (&cube, "cube")] {
            let bounds = mesh.bounds();
            let cell = 1.0 / 97.0;
            let grid = Grid::covering(&bounds, cell).unwrap();
            let found = Scan::new(&mesh.positions, &mesh.indices, &grid).unwrap().crossing_count() as f64;
            let expected = crossings(&mesh.positions, &mesh.indices, &bounds, [false; 3], cell);
            assert!((expected / found - 1.0).abs() < 0.03, "{name}: expected {expected}, found {found}");
        }
        // A cube faces the axes squarely, one crossing per cell of area; a sphere averages 1.5.
        let per_area = |mesh: &sculpt_geom::TriMesh, area: f64| {
            crossings(&mesh.positions, &mesh.indices, &mesh.bounds(), [false; 3], 0.01) * 1e-4 / area
        };
        assert!((per_area(&cube, 6.0) - 1.0).abs() < 1e-3);
        assert!((per_area(&sphere, std::f64::consts::PI) - 1.5).abs() < 0.02);
    }

    /// A cutter far larger than what it cuts is read only where the lattice
    /// is: its cost is the lattice's cross-section, not its own size.
    #[test]
    fn a_mesh_larger_than_the_lattice_counts_only_where_the_lattice_is() {
        let cutter = box_mesh(Vec3::splat(-500.0), Vec3::splat(500.0));
        let small = unit_box();
        let cell = 1.0 / 100.0;
        let counted = crossings(&cutter.positions, &cutter.indices, &small, [false; 3], cell);
        let whole = crossings(&cutter.positions, &cutter.indices, &cutter.bounds(), [false; 3], cell);
        assert!((whole - 6e10).abs() < 1e6, "{whole}");
        // Two faces across each axis over the lattice's 106 x 106 lines; each face is two triangles whose
        // boxes both cover the lattice, so it is counted twice. Too many by a little, never too few.
        assert!(counted < 12.1 * 106.0 * 106.0, "{counted}");
        let grid = Grid::covering(&small, cell).unwrap();
        let found = Scan::new(&cutter.positions, &cutter.indices, &grid).unwrap().crossing_count() as f64;
        assert!(found <= counted, "found {found}, counted {counted}");
        // And one wholly beside the lattice costs nothing.
        let beside = box_mesh(Vec3::splat(5.0), Vec3::splat(6.0));
        assert_eq!(crossings(&beside.positions, &beside.indices, &small, [false; 3], cell), 0.0);

        // A cube beyond the end of the lattice, in line with it: the lines along x run on through its two
        // faces across them, as a scan's do, and the other four are out of every line's way. Mirrored
        // across x the lattice reaches that far itself, and all six faces are read.
        let beyond = Aabb::new(Vec3::new(2.0, -0.5, -0.5), Vec3::new(3.0, 0.5, 0.5));
        let twin = box_mesh(Vec3::new(-3.0, -0.5, -0.5), Vec3::new(-2.0, 0.5, 0.5));
        let in_line = crossings(&twin.positions, &twin.indices, &beyond, [false; 3], cell);
        assert!((in_line / 2e4 - 1.0).abs() < 1e-3, "{in_line}");
        let mirrored = crossings(&twin.positions, &twin.indices, &beyond, [true, false, false], cell);
        assert!((mirrored / 6e4 - 1.0).abs() < 1e-3, "{mirrored}");
    }
}
