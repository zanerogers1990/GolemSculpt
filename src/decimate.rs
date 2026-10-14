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
//!     leaves masked surface alone too.) The soft edge of a mask is free, and
//!     each new vertex there takes its mask from the two it was made of. So
//!     the count asked for may not be reached, and the line says so.
//!   - **Hidden parts are decimated with the rest and stay hidden.** Hiding
//!     is a matter of what is seen; the budget, and the file an export
//!     writes, are of the whole model. A triangle of the result is one of
//!     the triangles the model had, so what was hidden still is.
//!   - **Face groups stay**, by the same token: each triangle of the result
//!     is in the group of the triangle it was.
//!   - **The quads of a quad remesh go**: what comes out is triangles.
//!   - **Symmetry is not kept exactly.** The two sides are thinned in
//!     whatever order their costs come. The app asks before decimating an
//!     object with a Local symmetry on (`app/decimate.rs`).
//!
//! A closed surface comes out closed. The line says so, by the same count of
//! open and crowded edges Check for Printing makes (`printable.rs`), taken
//! before and after.

use std::ops::RangeInclusive;

use glam::Vec3;

use crate::mesh::Mesh;
use crate::printable::{self, Cure, EdgeCensus, grouped};

/// A vertex masked at least this much is left where it is, and is not
/// removed: the mask at which dynamic topology and every brush leave a
/// vertex alone.
pub const FROZEN_MASK: f32 = 0.999;

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
}

impl Source {
    pub fn of(mesh: &Mesh) -> Source {
        Source {
            positions: mesh.positions.clone(),
            indices: mesh.indices.clone(),
            mask: mesh.mask.clone(),
            hidden: mesh.hidden_triangles().map_or_else(Vec::new, <[bool]>::to_vec),
            groups: mesh.group_ids().to_vec(),
        }
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
    let frozen: Vec<bool> = source.mask.iter().map(|&m| m >= FROZEN_MASK).collect();
    let any_frozen = frozen.contains(&true);
    let locked: &[bool] = if any_frozen { &frozen } else { &[] };
    let closed_before = sculpt_geom::edge_census(source.positions.len(), &source.indices);
    let Some(done) = sculpt_geom::decimate(&source.positions, &source.indices, Some(&source.mask), locked, target, progress)
    else {
        return Ok(None);
    };
    let hidden: Vec<bool> = if source.hidden.is_empty() {
        Vec::new()
    } else {
        done.triangle_source.iter().map(|&t| source.hidden.get(t as usize).copied().unwrap_or(false)).collect()
    };
    let report = Report {
        before,
        after: done.triangle_count(),
        target,
        frozen: done.locked,
        hidden_before: source.hidden.iter().filter(|&&h| h).count(),
        hidden_after: hidden.iter().filter(|&&h| h).count(),
        closed_before,
        closed_after: sculpt_geom::edge_census(done.positions.len(), &done.indices),
    };
    // Each triangle of the result in the group of the triangle it was.
    let groups: Vec<u16> =
        done.triangle_source.iter().map(|&t| source.groups.get(t as usize).copied().unwrap_or(0)).collect();
    let mesh = if source.groups.is_empty() {
        Mesh::new_with_hidden(done.positions, done.indices, done.values, &hidden)
    } else {
        let (mut mesh, numbering) = Mesh::new_numbered(done.positions, done.indices, done.values, &hidden);
        mesh.set_group_ids(crate::mesh::groups::renumbered(&groups, &numbering.triangle));
        mesh
    };
    Ok(Some((mesh, report)))
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
    (text + " Undo (Z) brings the triangles back.", closed)
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
                 (Z) brings the triangles back.",
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
        assert!(text.ends_with(" A Remesh (R) usually closes it. Undo (Z) brings the triangles back."), "{text}");
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
        };
        assert_eq!(
            line("Ring", &report, 0.004).0,
            "Decimated \"Ring\" from 5,000 to 16 triangles in 4 ms. 4 were asked for: any fewer would tear or fold \
             the surface. It was a closed surface and still is. Undo (Z) brings the triangles back."
        );
        // And for the one thing that must never happen.
        let torn = Report { closed_after: EdgeCensus { open: 3, crowded: 0, holes: Some(1) }, target: 16, ..report };
        let (text, ok) = line("Ring", &torn, 0.004);
        assert!(!ok && text.contains(" It was a closed surface and is not now: it has 3 open edges (1 hole), so"), "{text}");
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
    }
}
