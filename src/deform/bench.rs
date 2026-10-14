//! Headless timing of the deformers: `sculpt --bench-deform [triangles...]`
//! (1, 5 and 10 million unless told otherwise). For each deformer, on an
//! upright cylinder of that many triangles: one preview frame as the app
//! runs it while a slider or a handle is dragged, which is the pass over the
//! mesh, the normals of what moved and taking the ranges to send to the
//! graphics card. Each frame sets a slightly different value, as a drag
//! does, and the best of `FRAMES` is reported (the machine is shared, and the
//! best is the one least disturbed). Every deformation is cancelled before
//! the next, so each is timed on the untouched cylinder.

use std::time::Instant;

use glam::{DQuat, DVec3, Mat4};

use super::run::{Frame, Target};
use super::{Anchor, Bounds, Deformer, Kind, Lattice, Params};
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

const FRAMES: usize = 6;

/// The deformer set as a drag at `step` (0, 1, 2...) would have it.
pub fn dragged(kind: Kind, step: usize) -> Params {
    let k = 1.0 + step as f32 * 0.05;
    let mut lattice = Lattice::default();
    let top = lattice.index([1, 2, 1]);
    lattice.moved[top] = [0.1 * k, 0.2, 0.0];
    Params {
        kind,
        angle: 60.0 * k,
        tip: [0.2 * k, -0.3, 0.4],
        scales: [1.0, 0.5 / k],
        flatten: 0.3 * k,
        inflate: 0.02 * k,
        stretch: 1.3 * k,
        // Inflate and Lattice over the middle half, so their pass skips the rest.
        range: if matches!(kind, Kind::Inflate | Kind::Lattice) { [0.25, 0.75] } else { [0.0, 1.0] },
        anchor: Anchor::Start,
        lattice,
        ..Params::default()
    }
}

/// One preview frame: the pass, then what the renderer asks of the mesh.
/// Gives back the whole frame and the pass alone, in milliseconds.
fn frame(target: &mut Target, mesh: &mut Mesh, params: &Params, bounds: &Bounds) -> (f64, f64) {
    let start = Instant::now();
    let deformer = Deformer::new(params, bounds);
    target.apply(mesh, deformer.as_ref(), params.axis);
    let pass = start.elapsed().as_secs_f64() * 1000.0;
    mesh.flush_normals();
    let _ = mesh.take_dirty();
    (start.elapsed().as_secs_f64() * 1000.0, pass)
}

/// The best frame time of each deformer on `mesh`, and the best time of its
/// pass alone (the rest is the normals and the ranges to send), in milliseconds.
pub fn time_all(mesh: &mut Mesh) -> Vec<(Kind, f64, f64)> {
    let frame_of = Frame { origin: DVec3::ZERO, rotation: DQuat::IDENTITY };
    Kind::ALL
        .into_iter()
        .map(|kind| {
            let mut target = Target::begin(mesh, Mat4::IDENTITY, &frame_of);
            let bounds = target.bounds;
            let times: Vec<(f64, f64)> =
                (0..FRAMES).map(|step| frame(&mut target, mesh, &dragged(kind, step), &bounds)).collect();
            target.cancel(mesh);
            let _ = mesh.take_dirty();
            let best = |pick: fn(&(f64, f64)) -> f64| times.iter().map(pick).fold(f64::MAX, f64::min);
            (kind, best(|t| t.0), best(|t| t.1))
        })
        .collect()
}

pub fn run(sizes: &[usize]) {
    println!("Deformer preview frame, best of {FRAMES} (ms); {} threads", rayon::current_num_threads());
    let names: Vec<String> = Kind::ALL.iter().map(|k| format!("{:>9}", k.name())).collect();
    println!("{:>12} {}", "triangles", names.join(""));
    for &size in sizes {
        let mut mesh = primitives::build(Primitive::Cylinder, size, 0.5);
        let times = time_all(&mut mesh);
        let cells: Vec<String> = times.iter().map(|(_, ms, _)| format!("{ms:>9.1}")).collect();
        println!("{:>12} {}", mesh.triangle_count(), cells.join(""));
        let cells: Vec<String> = times.iter().map(|(_, _, ms)| format!("{ms:>9.1}")).collect();
        println!("{:>12} {}", "(pass alone)", cells.join(""));
    }
}
