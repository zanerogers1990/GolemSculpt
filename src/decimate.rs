//! Decimate: the selected object in fewer triangles, down to a count asked
//! for, keeping its shape as well as that many triangles can.
//!
//! The work is `sculpt_geom::decimate` (quadric edge collapse; see there for
//! how, and for what it promises of the surface). This file is the app's side
//! of it: a sculpt mesh in, a sculpt mesh out, what the mask and the hidden
//! parts mean to it, and the words for the status line.
//!
//! It is for a model that has to fit a triangle budget: a sculpt of a few
//! million triangles brought down to the 200k to 500k a game asset is
//! allowed, with the triangles kept where the surface bends and spent
//! sparingly where it is flat.
//!
//! What it does with what the user set up:
//!
//!   - **The mask protects**, as it does from every brush: a vertex that is
//!     fully masked is neither moved nor removed, so masked surface keeps
//!     every triangle it had. (The same threshold as dynamic topology, which
//!     leaves masked surface alone too.) So the count asked for may not be
//!     reached, and the line says so.
//!   - **A softer mask keeps more detail**, as much more as it is strong
//!     ([`detail_weight`]): the triangles are not kept, but folding them
//!     costs more, so they are folded later and more of them are left. At
//!     half strength about three times as many to the area as unmasked
//!     surface of the same shape, nearly ten times just short of full. The
//!     count is the same; it is spent elsewhere less. Each new vertex takes
//!     its mask from the two it was made of.
//!   - **Hidden parts are decimated with the rest and stay hidden.** Hiding
//!     is a matter of what is seen; the budget, and the file an export
//!     writes, are of the whole model. A triangle of the result is one of
//!     the triangles the model had, so what was hidden still is.
//!   - **Face groups stay**, by the same token: each triangle of the result
//!     is in the group of the triangle it was.
//!   - **The quads of a quad remesh go**: what comes out is triangles.
//!   - **A mirror is kept exactly** ([`Source::mirrored`], mirror.rs): the
//!     part on the positive side of each mirror plane is decimated and
//!     mirrored back, so the sides match point for point, and where they
//!     were not alike each keeps its own shape. Radial symmetry is not kept
//!     exactly; the app asks first (`app/decimate.rs`).
//!   - **Any count at once** ([`prepared`]): decimated once down to the
//!     slider's lowest, with every fold written down, the result for any
//!     count above that is had in a moment, the very result Decimate makes
//!     for it. That is the preview.
//!
//! A closed surface comes out closed. The line says so, by the same count of
//! open and crowded edges Check for Printing makes (`printable.rs`), taken
//! before and after.

mod mirror;

use std::ops::RangeInclusive;

use glam::Vec3;
use sculpt_geom::{Decimated, Keep, LengthUnit, Progressive};

use crate::mesh::Mesh;
use crate::printable::{self, Cure, EdgeCensus, grouped};

/// A vertex masked at least this much is left where it is, and is not
/// removed: the mask at which dynamic topology and every brush leave a
/// vertex alone.
pub const FROZEN_MASK: f32 = 0.999;

/// How much a vertex's surface counts towards what a fold costs, masked
/// `mask` (below [`FROZEN_MASK`]; at it the vertex is not touched at all):
/// 1 unmasked, a thousand times just short of fully masked. Where the
/// surface bends, the triangles left to an area go with the cube root of
/// it (`sculpt_geom::Keep::weights`), so a mask's strength is how many times
/// over its surface keeps its detail on a scale of powers of ten: about
/// three times at half strength, nearly ten just short of full.
pub fn detail_weight(mask: f32) -> f32 {
    if mask > 0.0 { 1000f32.powf(mask.min(1.0)) } else { 1.0 }
}

/// Fewest and most triangles the app asks for. The low end is a far
/// background prop; the high end is past any budget a game gives one model.
pub const TARGET_RANGE: RangeInclusive<f32> = 1_000.0..=4_000_000.0;

/// What a decimation reads, copied out of a sculpt mesh so that the work can
/// be done on a thread of its own while the app carries on.
pub struct Source {
    positions: Vec<Vec3>,
    indices: Vec<u32>,
    mask: Vec<f32>,
    /// One flag for each triangle; empty when nothing is hidden.
    hidden: Vec<bool>,
    /// The group of each triangle; empty when the mesh has no groups.
    groups: Vec<u16>,
    /// The mirror planes (X, Y, Z, through the object's own centre) the
    /// result is to be exactly symmetric across, and the Match Tolerance.
    mirror: [bool; 3],
    tolerance_scale: f32,
}

impl Source {
    pub fn of(mesh: &Mesh) -> Source {
        Source {
            positions: mesh.positions.clone(),
            indices: mesh.indices.clone(),
            mask: mesh.mask.clone(),
            hidden: mesh.hidden_triangles().map_or_else(Vec::new, <[bool]>::to_vec),
            groups: mesh.group_ids().to_vec(),
            mirror: [false; 3],
            tolerance_scale: 1.0,
        }
    }

    /// To come out exactly symmetric across the planes `axes` (X, Y, Z), its
    /// sides mirror images point for point, as an object with a Local mirror
    /// on is sculpted. `tolerance_scale` is the Symmetry card's Match
    /// Tolerance: how near a plane a point is taken as on it, and how far
    /// apart the sides may be and still count as the same.
    pub fn mirrored(self, axes: [bool; 3], tolerance_scale: f32) -> Source {
        Source { mirror: axes, tolerance_scale, ..self }
    }

    pub fn triangle_count(&self) -> usize {
        self.indices.len() / 3
    }
}

/// What a decimation did, for the status line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Report {
    pub before: usize,
    pub after: usize,
    /// The count that was asked for.
    pub target: usize,
    /// Fully masked vertices, which were left as they were.
    pub frozen: usize,
    /// Triangles that were hidden before, and how many still are.
    pub hidden_before: usize,
    pub hidden_after: usize,
    /// How closed the surface was, and is.
    pub closed_before: EdgeCensus,
    pub closed_after: EdgeCensus,
    /// The mirror planes the result was to be symmetric across, X, Y, Z.
    pub mirror: [bool; 3],
    /// Where the sides were a little off: how many points of the other sides
    /// kept their own side's shape, and how far apart the sides were at the
    /// most (world units).
    pub own_shape: usize,
    pub apart: f32,
    /// Points of the other sides that could not be moved onto their own
    /// side's shape without turning a triangle over, and how far off it they
    /// are at the most.
    pub held_back: usize,
    pub held_apart: f32,
    /// When the sides were two different shapes, how far apart they were at
    /// the most: the model was decimated whole, and is not mirrored.
    pub unmirrored: Option<f32>,
}

impl Report {
    /// Whether the count asked for was reached.
    pub fn reached(&self) -> bool {
        self.after <= self.target
    }
}

/// Decimates to about `target` triangles and builds the sculpt mesh of the
/// result. `progress` is called with how far along it is, from 0 to 1, and
/// returns whether to carry on; `None` if it said no. `Err` when there is
/// nothing to do, in words for the status line.
pub fn decimated(
    source: &Source,
    target: usize,
    progress: &mut dyn FnMut(f32) -> bool,
) -> Result<Option<(Mesh, Report)>, String> {
    let before = source.triangle_count();
    if before == 0 {
        return Err("Nothing to decimate.".into());
    }
    if target >= before {
        return Err(format!(
            "It has {} triangles already, which is no more than the {} asked for. Lower Target Triangles to decimate.",
            grouped(before),
            grouped(target)
        ));
    }
    let share = Plan::share(source);
    let Some(plan) = Plan::of(source, &mut |done| progress(done * share))? else { return Ok(None) };
    let done = {
        let (positions, indices, keep) = plan.part(source);
        let mut progress = |done: f32| progress(share + done * (1.0 - share));
        match sculpt_geom::decimate_keeping(positions, indices, &keep, target / plan.copies(), &mut progress) {
            Some(done) => done,
            None => return Ok(None),
        }
    };
    Ok(Some(plan.finish(source, done, target)))
}

/// A decimation set up: the part of the model the engine works on (the
/// whole, or with a mirror on, the part on the positive side of its planes)
/// and what it holds to.
struct Plan {
    cut: Option<mirror::Cut>,
    /// Fully masked vertices of the part, which are neither moved nor
    /// removed; empty when there are none.
    locked: Vec<bool>,
    /// How much each vertex's surface counts (`detail_weight`); empty when
    /// nothing is masked short of fully.
    weights: Vec<f32>,
    closed_before: EdgeCensus,
    /// With a mirror on, when the sides were two different shapes: how far
    /// apart they were at the most. The model is decimated whole.
    unmirrored: Option<f32>,
}

impl Plan {
    /// What share of a decimation's progress setting it up is: comparing the
    /// sides and cutting, with a mirror on; nothing to speak of without.
    fn share(source: &Source) -> f32 {
        if source.mirror.contains(&true) { 0.2 } else { 0.0 }
    }

    /// `None` if `progress` said to stop.
    fn of(source: &Source, progress: &mut dyn FnMut(f32) -> bool) -> Result<Option<Plan>, String> {
        let (mut cut, mut unmirrored) = (None, None);
        if source.mirror.contains(&true) {
            let tolerance = sculpt_symmetry::ops::tolerance(&source.positions, &source.indices, source.tolerance_scale);
            match mirror::cut(&source.positions, &source.indices, &source.mask, source.mirror, tolerance, progress)? {
                None => return Ok(None),
                Some(mirror::Sides::Cut(made)) => cut = Some(made),
                // Sides that are two different shapes are not made mirror images: whole, as it is.
                Some(mirror::Sides::Unlike(apart)) => unmirrored = Some(apart),
            }
        }
        let protect = cut.as_ref().map_or(&source.mask[..], |cut| &cut.protect[..]);
        let locked: Vec<bool> = protect.iter().map(|&m| m >= FROZEN_MASK).collect();
        let locked = if locked.contains(&true) { locked } else { Vec::new() };
        let soft = protect.iter().any(|&m| m > 0.0 && m < FROZEN_MASK);
        let weights = if soft { protect.iter().map(|&m| detail_weight(m)).collect() } else { Vec::new() };
        let closed_before = sculpt_geom::edge_census(source.positions.len(), &source.indices);
        Ok(Some(Plan { cut, locked, weights, closed_before, unmirrored }))
    }

    /// How many times over the part makes the whole.
    fn copies(&self) -> usize {
        self.cut.as_ref().map_or(1, mirror::Cut::copies)
    }

    /// The part's points and triangles, and what the engine is to keep.
    fn part<'a>(&'a self, source: &'a Source) -> (&'a [Vec3], &'a [u32], Keep<'a>) {
        let keep = Keep { locked: &self.locked, weights: &self.weights, ..Keep::default() };
        match &self.cut {
            Some(cut) => {
                let keep = Keep { values: Some(&cut.mask), seams: source.mirror, extra: &cut.extra, ..keep };
                (&cut.positions, &cut.indices, keep)
            }
            None => (&source.positions, &source.indices, Keep { values: Some(&source.mask), ..keep }),
        }
    }

    /// The sculpt mesh of `done`, the part decimated (mirrored back, with a
    /// mirror on), asked for `target` triangles, and what was done.
    fn finish(&self, source: &Source, done: Decimated, target: usize) -> (Mesh, Report) {
        let frozen = done.locked;
        let (positions, indices, mask, triangle_source, own_shape, apart, held) = match &self.cut {
            None => (done.positions, done.indices, done.values, done.triangle_source, 0, 0.0, (0, 0.0)),
            Some(cut) => {
                let sources = !source.hidden.is_empty() || !source.groups.is_empty();
                let whole = mirror::mirrored_back(cut, &done, sources);
                let held = (whole.held_back, whole.held_apart);
                (whole.positions, whole.indices, Some(whole.mask), whole.triangle_source, whole.own_shape, whole.apart, held)
            }
        };
        let hidden: Vec<bool> = if source.hidden.is_empty() {
            Vec::new()
        } else {
            triangle_source.iter().map(|&t| source.hidden.get(t as usize).copied().unwrap_or(false)).collect()
        };
        let report = Report {
            before: source.triangle_count(),
            after: indices.len() / 3,
            target,
            frozen,
            hidden_before: source.hidden.iter().filter(|&&h| h).count(),
            hidden_after: hidden.iter().filter(|&&h| h).count(),
            closed_before: self.closed_before,
            closed_after: sculpt_geom::edge_census(positions.len(), &indices),
            mirror: if self.cut.is_some() || self.unmirrored.is_some() { source.mirror } else { [false; 3] },
            own_shape,
            apart,
            held_back: held.0,
            held_apart: held.1,
            unmirrored: self.unmirrored,
        };
        // Each triangle of the result in the group of the triangle it was.
        let mesh = if source.groups.is_empty() {
            Mesh::new_with_hidden(positions, indices, mask, &hidden)
        } else {
            let groups: Vec<u16> =
                triangle_source.iter().map(|&t| source.groups.get(t as usize).copied().unwrap_or(0)).collect();
            let (mut mesh, numbering) = Mesh::new_numbered(positions, indices, mask, &hidden);
            mesh.set_group_ids(crate::mesh::groups::renumbered(&groups, &numbering.triangle));
            mesh
        };
        (mesh, report)
    }
}

/// A decimation worked out once down to the lowest count the Target
/// Triangles slider goes to, with every fold written down, so that the
/// result for any count above that is had in a moment: what the preview
/// shows while the slider is dragged. [`Prepared::at`] gives, to the bit,
/// what [`decimated`] does for the count.
pub struct Prepared {
    source: Source,
    plan: Plan,
    written: Progressive,
}

/// Prepares `source` for decimating to any count (see [`Prepared`]).
/// `progress` as for [`decimated`]; `None` if it said to stop, `Err` when
/// there is nothing to decimate.
pub fn prepared(source: Source, progress: &mut dyn FnMut(f32) -> bool) -> Result<Option<Prepared>, String> {
    if source.triangle_count() == 0 {
        return Err("Nothing to decimate.".into());
    }
    let share = Plan::share(&source);
    let Some(plan) = Plan::of(&source, &mut |done| progress(done * share))? else { return Ok(None) };
    let floor = (*TARGET_RANGE.start() as usize / plan.copies()).max(1);
    let written = {
        let (positions, indices, keep) = plan.part(&source);
        sculpt_geom::progressive(positions, indices, &keep, floor, &mut |done| progress(share + done * (1.0 - share)))
    };
    Ok(written.map(|written| Prepared { source, plan, written }))
}

impl Prepared {
    /// The model decimated to about `target` triangles, and what was done.
    pub fn at(&self, target: usize) -> (Mesh, Report) {
        let done = self.written.at(target / self.plan.copies());
        self.plan.finish(&self.source, done, target)
    }

    /// Triangles the model has.
    #[cfg(test)]
    pub fn triangles_before(&self) -> usize {
        self.source.triangle_count()
    }

    /// Whether a mirror is on and the sides were too unlike to be made
    /// mirror images, so the model is decimated whole.
    pub fn unmirrored(&self) -> bool {
        self.plan.unmirrored.is_some()
    }

    /// The fewest triangles it can be taken down to.
    pub fn fewest(&self) -> usize {
        self.written.fewest() * self.plan.copies()
    }

    /// Roughly what it holds, in bytes.
    #[cfg(test)]
    pub fn bytes(&self) -> usize {
        self.written.bytes() + self.source.positions.len() * 16 + self.source.indices.len() * 4
    }
}

/// A length of time for the status line: "640 ms", "2.3 s".
fn took(seconds: f64) -> String {
    if seconds < 1.0 { format!("{:.0} ms", seconds * 1000.0) } else { format!("{seconds:.1} s") }
}

/// The status line after `name` was decimated, and whether it is good news:
/// it is not when the result is not a closed surface.
pub fn line(name: &str, report: &Report, seconds: f64) -> (String, bool) {
    let (before, after) = (grouped(report.before), grouped(report.after));
    let mut text = format!("Decimated \"{name}\" from {before} to {after} triangles in {}.", took(seconds));
    match (report.reached(), report.frozen > 0) {
        (true, false) => {}
        (true, true) => text += " Fully masked surface was left as it was.",
        (false, true) => {
            text += &format!(
                " {} were asked for: fully masked surface is left as it is, and the rest has given all it can.",
                grouped(report.target)
            );
        }
        (false, false) => {
            text += &format!(
                " {} were asked for: any fewer would tear or fold the surface.",
                grouped(report.target)
            );
        }
    }
    if report.hidden_before > 0 {
        text += " Hidden parts were decimated with the rest and are still hidden.";
    }
    if report.mirror.contains(&true) {
        let planes: Vec<&str> = ["X", "Y", "Z"].into_iter().zip(report.mirror).filter(|(_, on)| *on).map(|(name, _)| name).collect();
        let planes = match planes.split_last() {
            Some((last, [])) => last.to_string(),
            Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
            None => String::new(),
        };
        text += &if let Some(apart) = report.unmirrored {
            let apart = sculpt_geom::units::format(apart, LengthUnit::Millimetres, true);
            format!(
                " With Mirror {planes} on, its sides are not mirror images of each other (up to {apart} apart), so it \
                 was decimated as it is: each side keeps its own shape, and their points do not pair up."
            )
        } else if report.own_shape == 0 {
            format!(" Its sides mirror each other across {planes}, point for point.")
        } else {
            let apart = sculpt_geom::units::format(report.apart, LengthUnit::Millimetres, true);
            format!(
                " Its sides mirror each other across {planes}, triangle for triangle; where they were not alike (up to \
                 {apart} apart), each kept its own shape."
            )
        };
        if report.held_back > 0 && report.unmirrored.is_none() {
            let apart = sculpt_geom::units::format(report.held_apart, LengthUnit::Millimetres, true);
            text += &format!(
                " At {} points the mirrored triangles could not follow their own side without folding over: those are \
                 up to {apart} from it.",
                grouped(report.held_back)
            );
        }
    }
    let closed = report.closed_after.is_closed();
    text += &match (report.closed_before.is_closed(), closed) {
        (true, true) => " It was a closed surface and still is.".to_owned(),
        // Decimation is built and tested never to do this; this is the check that it has not.
        (true, false) => {
            let faults = printable::faults(&report.closed_after).unwrap_or_default();
            format!(" It was a closed surface and is not now: it has {faults}, so it may not print.")
        }
        (false, true) => " It is a closed surface now.".to_owned(),
        (false, false) => {
            let warning = printable::warning(&[(name, report.closed_after)], Cure::Remesh).unwrap_or_default();
            format!(" It was not a closed surface before and is not now: {warning}")
        }
    };
    (text + " Undo (Ctrl+Z) brings the triangles back.", closed)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;
    use crate::mesh::primitives::{self, Primitive};

    const CLOSED: EdgeCensus = EdgeCensus { open: 0, crowded: 0, holes: Some(0) };

    /// A ball with lumps on it, as the app builds a sphere.
    fn lumpy(triangles: usize) -> Mesh {
        let ball = primitives::build(Primitive::Sphere, triangles, 0.5);
        let lumps = |d: Vec3| (d.y * 7.0 + 1.0).sin() * (d.z * 6.0 + 0.5).sin() + 0.5 * (d.x * 9.0 + d.y * 5.0).sin();
        let positions = ball.positions.iter().map(|p| *p * (1.0 + 0.12 * lumps(p.normalize()))).collect();
        Mesh::new(positions, ball.indices.clone(), None)
    }

    fn volume(mesh: &Mesh) -> f64 {
        mesh.indices
            .chunks_exact(3)
            .map(|t| {
                let p = [0, 1, 2].map(|c| mesh.positions[t[c] as usize].as_dvec3());
                p[0].dot(p[1].cross(p[2])) / 6.0
            })
            .sum()
    }

    fn run(mesh: &Mesh, target: usize) -> (Mesh, Report) {
        decimated(&Source::of(mesh), target, &mut |_| true).expect("something to do").expect("not cancelled")
    }

    /// The smallest of the app's benchmark sizes, to a quarter: what comes
    /// out is a sculpt mesh of the count asked for, closed, the same shape.
    #[test]
    fn a_200k_model_comes_down_to_a_quarter_closed_and_the_same_shape() {
        let model = lumpy(200_000);
        let before = model.triangle_count();
        assert!((190_000..=210_000).contains(&before), "{before}");
        let (mesh, report) = run(&model, 50_000);
        assert_eq!((report.before, report.after, report.target), (before, mesh.triangle_count(), 50_000));
        assert!(report.reached() && mesh.triangle_count() >= 49_999, "{}", mesh.triangle_count());
        assert_eq!((report.closed_before, report.closed_after), (CLOSED, CLOSED));
        assert!(printable::census(&mesh).is_closed());
        assert!((volume(&mesh) / volume(&model) - 1.0).abs() < 1e-3, "{} against {}", volume(&mesh), volume(&model));
        let (a, b) = (model.bounds(), mesh.bounds());
        assert!((a.min - b.min).abs().max_element() < 2e-3 && (a.max - b.max).abs().max_element() < 2e-3);
        // A sculpt mesh like any other: every vertex lit, nothing masked or hidden, no quads.
        assert!(mesh.normals.iter().all(|n| (n.length() - 1.0).abs() < 1e-3));
        assert!(mesh.mask.iter().all(|&m| m == 0.0) && !mesh.any_hidden() && mesh.polys.is_none());
        assert_eq!((report.frozen, report.hidden_before, report.hidden_after), (0, 0, 0));
        let (text, ok) = line("Sphere", &report, 0.25);
        assert_eq!(
            text,
            format!(
                "Decimated \"Sphere\" from {} to {} triangles in 250 ms. It was a closed surface and still is. Undo \
                 (Ctrl+Z) brings the triangles back.",
                grouped(before),
                grouped(mesh.triangle_count())
            )
        );
        assert!(ok);
    }

    #[test]
    fn fully_masked_surface_keeps_every_triangle_and_the_soft_edge_is_carried() {
        let mut model = lumpy(40_000);
        // Masked above the middle, with a soft edge a tenth of the ball wide.
        for v in 0..model.vertex_count() {
            model.mask[v] = (model.positions[v].y * 10.0).clamp(0.0, 1.0);
        }
        let frozen = |mesh: &Mesh, t: &[u32]| t.iter().all(|&v| mesh.mask[v as usize] >= FROZEN_MASK);
        let frozen_triangles = |mesh: &Mesh| mesh.indices.chunks_exact(3).filter(|t| frozen(mesh, t)).count();
        let frozen_places = |mesh: &Mesh| -> HashSet<[u32; 3]> {
            let masked = mesh.positions.iter().zip(&mesh.mask).filter(|(_, m)| **m >= FROZEN_MASK);
            masked.map(|(p, _)| p.to_array().map(f32::to_bits)).collect()
        };
        let (kept_triangles, kept_places) = (frozen_triangles(&model), frozen_places(&model));
        assert!(kept_triangles > 15_000);

        let (mesh, report) = run(&model, 25_000);
        assert!(report.reached() && report.frozen == kept_places.len());
        assert!(printable::census(&mesh).is_closed());
        // Every masked triangle is still there (the engine's own test checks them one by one), and
        // a few more are wholly masked now: ones at the mask's edge whose free corner was folded
        // into a masked one.
        let now = frozen_triangles(&mesh);
        assert!(now >= kept_triangles && now < kept_triangles + kept_triangles / 50, "{now} against {kept_triangles}");
        assert_eq!(frozen_places(&mesh), kept_places, "and every masked point, where it was to the bit");
        // The mask is what the surface had there: 1 above, 0 below, the ramp between.
        for (p, &m) in mesh.positions.iter().zip(&mesh.mask) {
            assert!((m - (p.y * 10.0).clamp(0.0, 1.0)).abs() < 0.2, "mask {m} at {p}");
        }
        assert!(line("Ball", &report, 0.1).0.contains(" Fully masked surface was left as it was. It was a closed"));

        // Asked for fewer than the masked part alone has: it stops short, and says why.
        let (mesh, report) = run(&model, 2_000);
        assert!(!report.reached() && mesh.triangle_count() >= kept_triangles && mesh.triangle_count() < 25_000);
        assert!(printable::census(&mesh).is_closed());
        let text = line("Ball", &report, 0.1).0;
        assert!(text.contains(" 2,000 were asked for: fully masked surface is left as it is, and the rest has"), "{text}");
    }

    #[test]
    fn hidden_parts_are_decimated_with_the_rest_and_stay_hidden() {
        let mut model = lumpy(40_000);
        let low: Vec<u32> = (0..model.triangle_count() as u32)
            .filter(|&t| model.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| model.positions[v as usize].y < -0.1))
            .collect();
        assert!(model.hide(&low).is_some());
        let share = low.len() as f64 / model.triangle_count() as f64;

        let (mesh, report) = run(&model, 10_000);
        assert!(report.reached() && printable::census(&mesh).is_closed());
        assert_eq!((report.hidden_before, report.hidden_after), (low.len(), mesh.hidden_triangle_count()));
        // About the same share of the model is hidden, and it is the same part: the bottom.
        let now = mesh.hidden_triangle_count() as f64 / mesh.triangle_count() as f64;
        assert!((now - share).abs() < 0.05, "{now} of it hidden, where {share} was");
        for t in 0..mesh.triangle_count() as u32 {
            let ys = mesh.indices[t as usize * 3..t as usize * 3 + 3].iter().map(|&v| mesh.positions[v as usize].y);
            let (lowest, highest) = ys.fold((f32::MAX, f32::MIN), |(l, h), y| (l.min(y), h.max(y)));
            if mesh.is_triangle_hidden(t) {
                assert!(lowest < -0.05, "a hidden triangle up at {lowest}");
            } else {
                assert!(highest > -0.15, "a showing triangle down at {highest}");
            }
        }
        assert!(line("Ball", &report, 0.1).0.contains(" Hidden parts were decimated with the rest and are still hidden."));
    }

    /// Groups painted by where a face is: the top cap one group, a band
    /// round the middle another, the rest in none.
    fn group_by_place(mesh: &Mesh, t: u32) -> u16 {
        let corners = &mesh.indices[t as usize * 3..t as usize * 3 + 3];
        let y = corners.iter().map(|&v| mesh.positions[v as usize].y).sum::<f32>() / 3.0;
        if y > 0.2 {
            3
        } else if y.abs() < 0.08 {
            7
        } else {
            0
        }
    }

    #[test]
    fn face_groups_are_decimated_with_the_rest_and_stay_where_they_were() {
        let mut model = lumpy(40_000);
        let ids: Vec<u16> = (0..model.triangle_count() as u32).map(|t| group_by_place(&model, t)).collect();
        model.set_group_ids(ids);
        let hide: Vec<u32> = (0..model.triangle_count() as u32).filter(|&t| group_by_place(&model, t) == 7).collect();
        assert!(model.hide(&hide).is_some());
        let (mesh, report) = run(&model, 8_000);
        assert!(report.reached() && printable::census(&mesh).is_closed());
        assert!(mesh.has_groups() && mesh.group_count() == model.group_count(), "{}", mesh.group_count());
        // A decimated face lies where the face it was did, give or take the
        // collapsed edges round it: nearly every one is still in the group
        // its place says, and the hidden band is still the hidden band.
        let wrong = (0..mesh.triangle_count() as u32).filter(|&t| mesh.group_of(t) != group_by_place(&mesh, t)).count();
        assert!(wrong * 50 < mesh.triangle_count(), "{wrong} of {} faces in another group", mesh.triangle_count());
        for t in 0..mesh.triangle_count() as u32 {
            assert_eq!(mesh.is_triangle_hidden(t), mesh.group_of(t) == 7, "hidden and grouped faces go together");
        }
        // Without groups, still none.
        let (plain, _) = run(&lumpy(20_000), 4_000);
        assert!(!plain.has_groups());
    }

    #[test]
    fn a_model_with_holes_keeps_its_holes_and_the_line_says_it_is_not_closed() {
        let ball = lumpy(20_000);
        let kept = ball.indices.chunks_exact(3).filter(|t| t.iter().any(|&v| ball.positions[v as usize].y.abs() < 0.4));
        let holed = Mesh::new(ball.positions.clone(), kept.flatten().copied().collect(), None);
        let before = printable::census(&holed);
        assert_eq!(before.holes, Some(2));
        let (mesh, report) = run(&holed, 4_000);
        assert_eq!(printable::census(&mesh).holes, Some(2), "two holes still, no more and no fewer");
        assert_eq!((report.closed_before, report.closed_after.holes), (before, Some(2)));
        let (text, ok) = line("Ball", &report, 1.26);
        assert!(!ok);
        assert!(text.contains(" in 1.3 s. It was not a closed surface before and is not now: \"Ball\" has "), "{text}");
        assert!(text.ends_with(" A Remesh (R) usually closes it. Undo (Ctrl+Z) brings the triangles back."), "{text}");
    }

    #[test]
    fn nothing_to_do_is_said_and_a_cancel_leaves_nothing() {
        let model = lumpy(2_000);
        let source = Source::of(&model);
        let count = model.triangle_count();
        let refused = decimated(&source, count, &mut |_| true).err().unwrap();
        assert!(refused.starts_with(&format!("It has {} triangles already", grouped(count))), "{refused}");
        assert!(decimated(&source, count + 5, &mut |_| true).is_err());
        let empty = Source::of(&Mesh::new(Vec::new(), Vec::new(), None));
        assert_eq!(decimated(&empty, 100, &mut |_| true).err().unwrap(), "Nothing to decimate.");
        assert!(decimated(&source, 500, &mut |_| false).unwrap().is_none(), "cancelled before it began");
        // The words for a count that could not be reached with no mask in the way.
        let report = Report {
            before: 5_000,
            after: 16,
            target: 4,
            frozen: 0,
            hidden_before: 0,
            hidden_after: 0,
            closed_before: CLOSED,
            closed_after: CLOSED,
            mirror: [false; 3],
            own_shape: 0,
            apart: 0.0,
            held_back: 0,
            held_apart: 0.0,
            unmirrored: None,
        };
        assert_eq!(
            line("Ring", &report, 0.004).0,
            "Decimated \"Ring\" from 5,000 to 16 triangles in 4 ms. 4 were asked for: any fewer would tear or fold \
             the surface. It was a closed surface and still is. Undo (Ctrl+Z) brings the triangles back."
        );
        // And for the one thing that must never happen.
        let torn = Report { closed_after: EdgeCensus { open: 3, crowded: 0, holes: Some(1) }, target: 16, ..report };
        let (text, ok) = line("Ring", &torn, 0.004);
        assert!(!ok && text.contains(" It was a closed surface and is not now: it has 3 open edges (1 hole), so"), "{text}");
    }

    /// A ball with lumps that are their own mirror image across X = 0 and Z = 0.
    fn even_lumps(triangles: usize) -> Mesh {
        let ball = primitives::build(Primitive::Sphere, triangles, 0.5);
        let lumps = |d: Vec3| (d.y * 7.0 + 1.0).sin() * (d.z * d.z * 9.0).cos() + 0.5 * (d.x * d.x * 8.0 + d.y * 5.0).cos();
        let positions = ball.positions.iter().map(|p| *p * (1.0 + 0.12 * lumps(p.normalize()))).collect();
        Mesh::new(positions, ball.indices.clone(), None)
    }

    fn mirrored_run(mesh: &Mesh, axes: [bool; 3], target: usize) -> (Mesh, Report) {
        let source = Source::of(mesh).mirrored(axes, 1.0);
        decimated(&source, target, &mut |_| true).expect("something to do").expect("not cancelled")
    }

    /// Every point off the planes has its mirror image across each of them,
    /// to the bit; and the triangles pair up the same way.
    fn assert_exact_mirror(mesh: &Mesh, axes: [bool; 3]) {
        // A point on a plane is its own mirror image: -0 is 0.
        let key = |p: Vec3| (p + Vec3::ZERO).to_array().map(f32::to_bits);
        let points: HashSet<[u32; 3]> = mesh.positions.iter().map(|&p| key(p)).collect();
        let triangles: HashSet<[[u32; 3]; 3]> = mesh
            .indices
            .chunks_exact(3)
            .map(|t| {
                let mut corners = [0, 1, 2].map(|k| key(mesh.positions[t[k] as usize]));
                corners.sort_unstable();
                corners
            })
            .collect();
        for axis in (0..3).filter(|&axis| axes[axis]) {
            let flip = |mut p: Vec3| {
                p[axis] = -p[axis];
                p
            };
            for &p in &mesh.positions {
                assert!(points.contains(&key(flip(p))), "{p} has no mirror image across axis {axis}");
            }
            for t in mesh.indices.chunks_exact(3) {
                let mut corners = [0, 1, 2].map(|k| key(flip(mesh.positions[t[k] as usize])));
                corners.sort_unstable();
                assert!(triangles.contains(&corners), "a triangle with no mirror image across axis {axis}");
            }
        }
    }

    #[test]
    fn with_a_mirror_on_the_sides_come_out_mirror_images_point_for_point() {
        let model = even_lumps(60_000);
        for (axes, words) in [([true, false, false], "across X, point for point."), ([true, false, true], "across X and Z, point for point.")] {
            let (mesh, report) = mirrored_run(&model, axes, 12_000);
            assert!(report.reached() && mesh.triangle_count() > 11_000, "{}", mesh.triangle_count());
            assert!(printable::census(&mesh).is_closed(), "{axes:?}: {:?}", printable::census(&mesh));
            assert_exact_mirror(&mesh, axes);
            assert!((volume(&mesh) / volume(&model) - 1.0).abs() < 2e-3, "{axes:?}: {} against {}", volume(&mesh), volume(&model));
            assert_eq!((report.mirror, report.own_shape), (axes, 0));
            let text = line("Head", &report, 0.5).0;
            assert!(text.contains(&format!(" Its sides mirror each other {words} It was a closed surface")), "{text}");
        }
    }

    /// `even_lumps` with a bump on the -X side only, a fifth of the ball
    /// wide, standing `height` off the surface at its top; and where it is.
    fn bumped(height: f32) -> (Mesh, Vec3) {
        let model = even_lumps(60_000);
        let bump = Vec3::new(-0.45, 0.1, 0.15).normalize() * 0.5;
        let positions = model
            .positions
            .iter()
            .map(|&p| {
                let near = (1.0 - p.distance(bump) / 0.1).max(0.0);
                p + p.normalize() * height * near * near
            })
            .collect();
        (Mesh::new(positions, model.indices.clone(), None), bump)
    }

    /// How far `p` is from the surface of `mesh`.
    fn off(mesh: &Mesh, p: Vec3) -> f32 {
        let (t, _) = mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, p, f32::INFINITY, |_| false).unwrap();
        let [a, b, c] = [0, 1, 2].map(|k| mesh.positions[mesh.indices[t as usize * 3 + k] as usize]);
        crate::mesh::bvh::nearest::closest_on_triangle(p, a, b, c).distance(p)
    }

    /// Of the points of `mesh` over the middle of `bump`, how far they are
    /// on average from the surface of `bumped`, and of `plain`.
    fn over_the_bump(mesh: &Mesh, bump: Vec3, bumped: &Mesh, plain: &Mesh) -> (f32, f32) {
        let over: Vec<Vec3> = mesh.positions.iter().copied().filter(|p| p.normalize().distance(bump.normalize()) < 0.05).collect();
        assert!(over.len() >= 3, "{} points over the bump", over.len());
        let mean = |surface: &Mesh| over.iter().map(|&p| off(surface, p)).sum::<f32>() / over.len() as f32;
        (mean(bumped), mean(plain))
    }

    #[test]
    fn where_the_sides_are_a_little_off_each_keeps_its_own_shape_on_mirrored_triangles() {
        let tolerance = {
            let model = even_lumps(60_000);
            sculpt_symmetry::ops::tolerance(&model.positions, &model.indices, 1.0)
        };
        // Two and a half Match Tolerances high: a little off, not another shape.
        let (model, bump) = bumped(2.5 * tolerance);
        let (mesh, report) = mirrored_run(&model, [true, false, false], 12_000);
        assert!(printable::census(&mesh).is_closed());
        assert_eq!(report.unmirrored, None);
        assert!(report.own_shape > 5 && report.apart > tolerance, "{} points kept their own shape, {} apart", report.own_shape, report.apart);
        // The bump is still there: the points over it lie on the bumped surface, not the plain one.
        let (on_bump, on_plain) = over_the_bump(&mesh, bump, &model, &even_lumps(60_000));
        assert!(on_bump * 3.0 < on_plain, "{on_bump} off the bump and {on_plain} off the plain ball");
        // Away from it, still mirror images to the bit.
        let key = |p: Vec3| (p + Vec3::ZERO).to_array().map(f32::to_bits);
        let points: HashSet<[u32; 3]> = mesh.positions.iter().map(|&p| key(p)).collect();
        let away = |p: Vec3| p.normalize().distance(bump.normalize()) > 0.4;
        let far = mesh.positions.iter().filter(|p| away(**p) && away(Vec3::new(-p.x, p.y, p.z)));
        assert!(far.clone().count() > 1000 && far.clone().all(|p| points.contains(&key(Vec3::new(-p.x, p.y, p.z)))));
        let text = line("Head", &report, 0.5).0;
        assert!(text.contains(" Its sides mirror each other across X, triangle for triangle; where they were not alike (up to "), "{text}");
        assert!(text.contains(" apart), each kept its own shape. It was a closed surface"), "{text}");

        // Further off, short of two shapes: still mirrored, the bump still
        // followed, and the line says where it could not be.
        let (model, bump) = bumped(6.0 * tolerance);
        let (mesh, report) = mirrored_run(&model, [true, false, false], 12_000);
        assert!(printable::census(&mesh).is_closed() && report.unmirrored.is_none());
        let (on_bump, on_plain) = over_the_bump(&mesh, bump, &model, &even_lumps(60_000));
        assert!(on_bump * 3.0 < on_plain, "{on_bump} off the bump and {on_plain} off the plain ball");
        let text = line("Head", &report, 0.5).0;
        println!("{text}");
        assert_eq!(report.held_back > 0, text.contains(" could not follow their own side without folding over: those are up to "), "{text}");
    }

    #[test]
    fn sides_that_are_two_different_shapes_are_decimated_as_they_are() {
        // A bump a third of the radius high on one side only: the door of
        // docs/remesh-symmetry-fix.md. The mirror for the brushes must not
        // make one side over into the other.
        let (model, bump) = bumped(0.16);
        let (mesh, report) = mirrored_run(&model, [true, false, false], 12_000);
        assert!(report.reached() && printable::census(&mesh).is_closed());
        assert!(report.unmirrored.is_some_and(|apart| apart > 0.03), "{:?}", report.unmirrored);
        assert_eq!((report.mirror, report.own_shape), ([true, false, false], 0));
        let (on_bump, on_plain) = over_the_bump(&mesh, bump, &model, &even_lumps(60_000));
        assert!(on_bump * 10.0 < on_plain, "{on_bump} off the bump and {on_plain} off the plain ball");
        assert!((volume(&mesh) / volume(&model) - 1.0).abs() < 2e-3, "{} against {}", volume(&mesh), volume(&model));
        let text = line("Door", &report, 0.5).0;
        assert!(
            text.contains(" With Mirror X on, its sides are not mirror images of each other (up to ")
                && text.contains(" apart), so it was decimated as it is: each side keeps its own shape, and their points do not pair up. It was a closed"),
            "{text}"
        );
        // The preview says so before anything is kept.
        let ready = prepared(Source::of(&model).mirrored([true, false, false], 1.0), &mut |_| true).unwrap().unwrap();
        assert!(ready.unmirrored());
    }

    #[test]
    fn with_a_mirror_on_a_side_masked_on_its_own_keeps_its_detail_and_its_mask() {
        let mut model = even_lumps(40_000);
        // The -X side's top is masked, the +X side's is not.
        for v in 0..model.vertex_count() {
            let p = model.positions[v];
            model.mask[v] = if p.x < -0.05 && p.y > 0.2 { 1.0 } else { 0.0 };
        }
        let (mesh, report) = mirrored_run(&model, [true, false, false], 8_000);
        assert!(printable::census(&mesh).is_closed());
        assert!(report.frozen > 0);
        // Both tops keep their detail; the rest pays for it.
        let count = |test: &dyn Fn(Vec3) -> bool| mesh.indices.chunks_exact(3).filter(|t| test(mesh.positions[t[0] as usize])).count();
        let (masked_top, mirrored_top) = (count(&|p| p.x < -0.1 && p.y > 0.25), count(&|p| p.x > 0.1 && p.y > 0.25));
        let bottom = count(&|p| p.x.abs() > 0.1 && p.y < -0.25);
        assert!(mirrored_top * 10 > masked_top * 9 && masked_top > bottom * 3, "{masked_top} {mirrored_top} {bottom}");
        // The mask is each side's own.
        for (p, &m) in mesh.positions.iter().zip(&mesh.mask) {
            if p.y > 0.25 && p.x.abs() > 0.1 {
                assert_eq!(m > 0.5, p.x < 0.0, "mask {m} at {p}");
            }
        }
    }

    #[test]
    fn a_softer_mask_keeps_more_detail_as_it_is_stronger() {
        let share = |strength: f32| {
            let mut masked = lumpy(40_000);
            for v in 0..masked.vertex_count() {
                masked.mask[v] = if masked.positions[v].y > 0.0 { strength } else { 0.0 };
            }
            let (mesh, report) = run(&masked, 6_000);
            assert!(report.reached() && report.frozen == 0);
            let top = mesh.indices.chunks_exact(3).filter(|t| t.iter().all(|&v| mesh.positions[v as usize].y > 0.05)).count();
            let bottom = mesh.indices.chunks_exact(3).filter(|t| t.iter().all(|&v| mesh.positions[v as usize].y < -0.05)).count();
            top as f64 / bottom as f64
        };
        let (none, half, most) = (share(0.0), share(0.5), share(0.95));
        println!("top over bottom: unmasked {none:.2}, half {half:.2}, 95% {most:.2}");
        assert!(half > none * 2.0 && most > half * 2.0, "{none} {half} {most}");
        assert!((detail_weight(0.0), detail_weight(1.0)) == (1.0, 1000.0) && (detail_weight(0.5) - 31.62).abs() < 0.01);
    }

    #[test]
    fn prepared_once_any_count_is_the_decimation_of_that_count_to_the_bit() {
        let mut model = even_lumps(30_000);
        for v in 0..model.vertex_count() {
            model.mask[v] = (model.positions[v].z * 3.0).clamp(0.0, 1.0);
        }
        let hide: Vec<u32> = (0..model.triangle_count() as u32)
            .filter(|&t| model.indices[t as usize * 3..t as usize * 3 + 3].iter().all(|&v| model.positions[v as usize].y < -0.3))
            .collect();
        assert!(model.hide(&hide).is_some());
        for axes in [[false; 3], [true, false, false]] {
            let source = || Source::of(&model).mirrored(axes, 1.0);
            let prepared = prepared(source(), &mut |_| true).unwrap().unwrap();
            assert_eq!(prepared.triangles_before(), model.triangle_count());
            // As few as the fully masked part allows.
            assert!(prepared.fewest() < model.triangle_count() / 4, "{}", prepared.fewest());
            for target in [25_000, 9_000, 3_000, 1_000] {
                let (mesh, report) = prepared.at(target);
                let (direct, direct_report) = decimated(&source(), target, &mut |_| true).unwrap().unwrap();
                assert_eq!(report, direct_report, "{axes:?} at {target}");
                assert_eq!(mesh.indices, direct.indices);
                let bits = |m: &Mesh| m.positions.iter().map(|p| p.to_array().map(f32::to_bits)).collect::<Vec<_>>();
                assert_eq!(bits(&mesh), bits(&direct));
                assert_eq!(mesh.mask, direct.mask);
                assert_eq!(mesh.hidden_triangle_count(), direct.hidden_triangle_count());
                assert!(mesh.hidden_triangle_count() > 0);
            }
        }
    }

    /// Run with `--release -- --ignored --nocapture`: what Decimate costs at
    /// the app's benchmark sizes, the sculpt mesh of the result included
    /// (its spatial index, adjacency and normals), which is what the user
    /// waits for.
    #[test]
    #[ignore]
    fn timing_of_a_decimate_at_200k_and_1m_triangles() {
        for (triangles, targets) in [(200_000, [100_000, 50_000]), (1_000_000, [500_000, 200_000])] {
            let model = lumpy(triangles);
            let was = volume(&model);
            for target in targets {
                let clock = std::time::Instant::now();
                let source = Source::of(&model);
                let (mesh, report) = decimated(&source, target, &mut |_| true).unwrap().unwrap();
                let seconds = clock.elapsed().as_secs_f64();
                println!("{}", line("Model", &report, seconds).0);
                assert!(report.reached() && printable::census(&mesh).is_closed());
                assert!((volume(&mesh) / was - 1.0).abs() < 1e-3);
            }
        }
        // With a mirror on (X), and prepared for the preview, then a count shown.
        for triangles in [200_000, 1_000_000] {
            let model = even_lumps(triangles);
            let target = triangles / 5;
            let clock = std::time::Instant::now();
            let (mesh, report) = mirrored_run(&model, [true, false, false], target);
            println!("mirrored, {}: {:.0} ms", line("Model", &report, 0.0).0, clock.elapsed().as_secs_f64() * 1e3);
            assert!(report.reached() && printable::census(&mesh).is_closed());
            let clock = std::time::Instant::now();
            let ready = prepared(Source::of(&model), &mut |_| true).unwrap().unwrap();
            let took = clock.elapsed().as_secs_f64();
            let clock = std::time::Instant::now();
            let (shown, _) = ready.at(target);
            println!(
                "prepared {} triangles in {:.0} ms ({} MB held); {} shown in {:.0} ms",
                model.triangle_count(),
                took * 1e3,
                ready.bytes() >> 20,
                shown.triangle_count(),
                clock.elapsed().as_secs_f64() * 1e3
            );
        }
    }
}
