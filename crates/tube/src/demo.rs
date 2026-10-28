//! Scripted muscle strokes, for pictures and timings: `--demo-tool muscle:[round][-merged|-selected[-edge]]`.
//!
//! `MUSCLE_STROKES` is what to draw, in fractions of the viewport from its top-left corner: points
//! joined by `>`, strokes parted by `;`, and a stroke that starts with `~` drawn floating.
//! `MUSCLE_SET` is settings as `name=value,...`, the radius as a fraction of the object's half size.
//! `MUSCLE_MIRROR` draws under X symmetry, `MUSCLE_WOBBLE` shakes the hand by so many pixels, and
//! `MUSCLE_BARE` leaves the handles out of the picture. Each run prints what it drew, what one edit
//! costs, and a fingerprint that the same strokes on the same model must repeat.

use std::time::Instant;

use glam::{Vec2, Vec3};
use sculpt_toolkit::Ctx;

use crate::tool::TubeTool;
use crate::types::TubeSettings;

fn milliseconds(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

impl TubeTool {
    pub(crate) fn demo_muscle(&mut self, cx: &mut Ctx, what: &str) {
        let scene = cx.scene();
        let bounds = scene.bounds(scene.active());
        if bounds.is_empty() {
            return;
        }
        let reach = bounds.size().max_element() * 0.5;
        let mut settings = TubeSettings { radius: reach * 0.08, ..TubeSettings::default() }.muscle();
        if what.contains("round") {
            (settings.flatten, settings.depth) = (1.0, 0.25);
        }
        self.bare = std::env::var_os("MUSCLE_BARE").is_some();
        for item in std::env::var("MUSCLE_SET").unwrap_or_default().split(',') {
            let Some((name, value)) = item.split_once('=') else { continue };
            let Ok(value) = value.trim().parse::<f32>() else { continue };
            match name.trim() {
                "radius" => settings.radius = reach * value,
                "swell" => settings.swell = value,
                "belly" => settings.belly = value,
                "flatten" => settings.flatten = value,
                "depth" => settings.depth = value,
                "taper" => settings.taper = value,
                "smoothness" => settings.smoothness = value,
                "blend" => settings.blend = value,
                other => eprintln!("muscle: no setting called {other:?}"),
            }
        }
        self.defaults = settings;
        if std::env::var_os("MUSCLE_MIRROR").is_some() {
            cx.set_mirror([true, false, false]);
        }

        let size = cx.view.size;
        let strokes = std::env::var("MUSCLE_STROKES").unwrap_or_else(|_| "0.38,0.4>0.5,0.5>0.6,0.66".to_owned());
        for stroke in strokes.split(';') {
            let stroke = stroke.trim();
            let (free, stroke) = stroke.strip_prefix('~').map_or((false, stroke), |rest| (true, rest));
            let way: Vec<Vec2> = stroke
                .split('>')
                .filter_map(|p| {
                    let (x, y) = p.split_once(',')?;
                    Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?) * size)
                })
                .collect();
            let mut path: Vec<Vec2> = Vec::new();
            for pair in way.windows(2) {
                path.extend((0..24).map(|step| pair[0].lerp(pair[1], step as f32 / 24.0)));
            }
            path.extend(way.last());
            // A shaky hand: `MUSCLE_WOBBLE` pixels of it, the same every run.
            let wobble: f32 = std::env::var("MUSCLE_WOBBLE").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0);
            for i in 1..path.len().saturating_sub(1) {
                let along = (path[i + 1] - path[i - 1]).normalize_or_zero();
                let shake = (i as f32 * 0.9).sin() + 0.6 * (i as f32 * 2.3 + 1.0).sin();
                path[i] += Vec2::new(-along.y, along.x) * (wobble * shake);
            }
            self.set_on_surface(!free);
            let drawn = self.draw_stroke(cx, &path, free);
            let points = self.tubes.last().map_or(0, |t| t.state.points.len());
            let on = self.tubes.last().map_or(0, |t| t.state.points.iter().filter(|p| p.is_on_surface()).count());
            println!("muscle: stroke {stroke:?} {}: {points} points, {on} on the surface", if drawn { "drawn" } else { "NOT drawn" });
        }

        self.refresh(cx, false);
        let start = Instant::now();
        self.refresh(cx, true);
        println!("muscle: {} built and meshed in {:.1} ms", self.tubes.len(), milliseconds(start));
        self.time_an_edit(cx, reach);
        // The same strokes on the same model must print the same number, run after run.
        let mut print: u64 = 0xcbf29ce484222325;
        let mut mix = |v: Vec3| {
            for bits in [v.x.to_bits(), v.y.to_bits(), v.z.to_bits()] {
                print = (print ^ bits as u64).wrapping_mul(0x100000001b3);
            }
        };
        for tube in &self.tubes {
            tube.state.points.iter().for_each(|p| mix(p.position));
            tube.copies.iter().flat_map(|c| &c.samples).for_each(|s| mix(s.center));
            tube.preview.data().positions.iter().for_each(|p| mix(*p));
        }
        println!("muscle: fingerprint {print:016x}");

        if what.contains("merged") {
            let start = Instant::now();
            self.merge_all(cx);
            self.wait_for_merge(cx);
            println!("muscle: merged in {:.0} ms: {}", milliseconds(start), self.status);
        } else if what.contains("selected") {
            if let Some(tube) = self.tubes.len().checked_sub(1) {
                self.select_tube(tube, Some(self.point_count(tube) / 2));
            }
            // `selected-edge`: one edge of that point dragged out, as far again as the muscle is wide.
            self.refresh(cx, false);
            if let Some(ring) = self.ring_handles(&cx.view).first().copied().filter(|_| what.contains("edge")) {
                let to = ring.at + (ring.at - ring.centre) * 2.0;
                self.press(cx, ring.at, false, false);
                (1..=8).for_each(|step| self.drag_to(cx, ring.at.lerp(to, step as f32 / 8.0)));
                self.release(cx);
                println!("muscle: an edge dragged out: {}", self.status);
            }
        } else {
            (self.active, self.selected_point) = (None, None);
        }
        self.refresh(cx, false);
        self.flush(cx);
    }

    /// What one step of dragging a point of the last muscle costs: its centre line laid again and its
    /// preview meshed again.
    fn time_an_edit(&mut self, cx: &Ctx, reach: f32) {
        let Some(tube) = self.tubes.len().checked_sub(1) else { return };
        let points = self.tubes[tube].state.points.clone();
        let middle = points.len() / 2;
        let (mut total, mut worst) = (0.0, 0.0f64);
        const STEPS: usize = 30;
        for step in 0..STEPS {
            let mut moved = points[middle];
            moved.position += Vec3::splat(reach * 1e-4 * (step + 1) as f32);
            self.tubes[tube].state.points[middle] = moved;
            let start = Instant::now();
            self.refresh(cx, true);
            let took = milliseconds(start);
            total += took;
            worst = worst.max(took);
        }
        self.tubes[tube].state.points = points;
        self.refresh(cx, true);
        let live = &self.tubes[tube];
        println!(
            "muscle: an edit rebuilds in {:.1} ms on average, {:.1} ms at worst ({} stations, {} preview triangles)",
            total / STEPS as f64,
            worst,
            live.copies.first().map_or(0, |c| c.samples.len()),
            live.preview.data().indices.len() / 3,
        );
    }
}
