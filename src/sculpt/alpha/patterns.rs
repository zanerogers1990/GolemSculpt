//! The alphas that come with the app, each drawn by a function of where on
//! the picture a sample is (0..1 across, 0..1 down).
//!
//! All but Square repeat exactly from one edge to the opposite one, so they
//! can be tiled across the brush with no seam.

use std::f32::consts::{PI, TAU};

use super::super::kernels::{quintic, smooth};

/// The built-in alphas, by name, in the order the panel shows them.
pub const ALL: [(&str, fn(f32, f32) -> f32); 7] = [
    ("Square", square),
    ("Rake", rake),
    ("Noise", noise),
    ("Bumps", bumps),
    ("Scales", scales),
    ("Cracks", cracks),
    ("Weave", weave),
];

/// A number from 0 up to 1 for a grid cell, the same every run on every
/// machine.
fn hash(x: i32, y: i32, seed: u32) -> f32 {
    let mut h = (x as u32).wrapping_mul(0x9E37_79B1)
        ^ (y as u32).wrapping_mul(0x85EB_CA77)
        ^ seed.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^= h >> 12;
    h = h.wrapping_mul(0x297A_2D39);
    h ^= h >> 15;
    (h >> 8) as f32 / (1u32 << 24) as f32
}

/// A distance along a repeating axis, taken the short way round.
fn wrapped(d: f32) -> f32 {
    d - d.round()
}

/// A flat-topped square that fits inside the brush ring, for stamping hard
/// shapes.
fn square(x: f32, y: f32) -> f32 {
    let edge = (x - 0.5).abs().max((y - 0.5).abs()) * 2.0;
    smooth((0.68 - edge) / 0.08)
}

/// Ridges running up the picture, like the teeth of a rake.
fn rake(x: f32, _y: f32) -> f32 {
    const TEETH: f32 = 6.0;
    quintic(0.5 - 0.5 * (TAU * x * TEETH).cos())
}

/// Smooth random values on a grid `period` cells a side that repeats.
fn value_noise(x: f32, y: f32, period: i32, seed: u32) -> f32 {
    let (x, y) = (x * period as f32, y * period as f32);
    let (x0, y0) = (x.floor(), y.floor());
    let (tx, ty) = (quintic(x - x0), quintic(y - y0));
    let at = |dx: i32, dy: i32| hash((x0 as i32 + dx).rem_euclid(period), (y0 as i32 + dy).rem_euclid(period), seed);
    let top = at(0, 0) + (at(1, 0) - at(0, 0)) * tx;
    let bottom = at(0, 1) + (at(1, 1) - at(0, 1)) * tx;
    top + (bottom - top) * ty
}

/// Rough, cloudy texture: four layers of noise, each twice as fine.
fn noise(x: f32, y: f32) -> f32 {
    let (mut sum, mut amp, mut norm) = (0.0, 0.5, 0.0);
    for octave in 0..4 {
        sum += amp * value_noise(x, y, 4 << octave, octave);
        norm += amp;
        amp *= 0.5;
    }
    // The layers average out toward the middle; spread them back over 0..1.
    ((sum / norm - 0.5) * 2.2 + 0.5).clamp(0.0, 1.0)
}

/// Calls `f` with the centre of every jittered grid point near (`px`, `py`),
/// on a grid `cells` a side that repeats, and a number of the point's own.
fn each_point(px: f32, py: f32, cells: i32, seed: u32, mut f: impl FnMut(f32, f32, f32)) {
    let (cx, cy) = (px.floor() as i32, py.floor() as i32);
    for gy in cy - 1..=cy + 1 {
        for gx in cx - 1..=cx + 1 {
            let (wx, wy) = (gx.rem_euclid(cells), gy.rem_euclid(cells));
            let centre_x = gx as f32 + 0.2 + 0.6 * hash(wx, wy, seed);
            let centre_y = gy as f32 + 0.2 + 0.6 * hash(wx, wy, seed + 1);
            f(centre_x, centre_y, hash(wx, wy, seed + 2));
        }
    }
}

/// Scattered round bumps of different sizes: warts, rivets, or pores when
/// inverted.
fn bumps(x: f32, y: f32) -> f32 {
    const CELLS: i32 = 5;
    let (px, py) = (x * CELLS as f32, y * CELLS as f32);
    let mut best = 0.0f32;
    each_point(px, py, CELLS, 11, |cx, cy, own| {
        let radius = 0.26 + 0.2 * own;
        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        best = best.max(smooth(1.0 - d / radius));
    });
    best
}

/// Overlapping scales in rows, each rising toward its rounded lower end and
/// lying over the row below.
fn scales(x: f32, y: f32) -> f32 {
    // Measured in scale widths: a scale is 1 wide and 1 long (a square top
    // half and a round lower half), and rows are half a width apart.
    const COLUMNS: f32 = 3.0;
    let (px, py) = (x * COLUMNS, y * COLUMNS);
    let row = (py * 2.0).floor() as i32;
    // The scale of row `r` that this point is under, as (how far down it,
    // how far inside its outline).
    let scale = |r: i32| {
        let shift = if r.rem_euclid(2) == 1 { 0.5 } else { 0.0 };
        let dx = wrapped(px - shift - 0.5);
        let dy = py - r as f32 * 0.5;
        let inside = if dy <= 0.5 { 0.5 - dx.abs() } else { 0.5 - (dx * dx + (dy - 0.5).powi(2)).sqrt() };
        (dy, inside)
    };
    let height = |dy: f32| dy;
    // This row's own scale, with a shallow groove between it and the next.
    let (dy, inside) = scale(row);
    let under = height(dy) * (0.55 + 0.45 * smooth(inside / 0.1));
    // The round end of the scale above lies over it.
    let (dy, inside) = scale(row - 1);
    under + (height(dy) - under) * smooth(inside / 0.05)
}

/// Flat plates with cracks between them: dried mud, old paint, thick hide.
fn cracks(x: f32, y: f32) -> f32 {
    const CELLS: i32 = 5;
    let (px, py) = (x * CELLS as f32, y * CELLS as f32);
    let (mut nearest, mut second) = (f32::MAX, f32::MAX);
    each_point(px, py, CELLS, 23, |cx, cy, _| {
        let d = ((px - cx).powi(2) + (py - cy).powi(2)).sqrt();
        if d < nearest {
            (nearest, second) = (d, nearest);
        } else if d < second {
            second = d;
        }
    });
    smooth((second - nearest) / 0.25)
}

/// Woven threads going over and under each other.
fn weave(x: f32, y: f32) -> f32 {
    const THREADS: f32 = 6.0;
    let (px, py) = (x * THREADS, y * THREADS);
    let (column, row) = (px.floor(), py.floor());
    // A thread's round back, across it.
    let across = |t: f32| (PI * t).sin().max(0.0).sqrt();
    // How high it rides, along it: over one crossing and under the next.
    let rides = |along: f32, lane: f32| 0.5 + 0.5 * (PI * (along - 0.5 + lane)).cos();
    let weft = across(py - row) * (0.25 + 0.75 * rides(px, row));
    let warp = across(px - column) * (0.25 + 0.75 * rides(py, column + 1.0));
    weft.max(warp)
}
