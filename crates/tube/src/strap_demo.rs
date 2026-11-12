//! Scripted straps, for pictures and timings: `--demo-tool strap:[wrap][-merged|-object|-selected]`.
//!
//! `STRAP_STROKES` is what to draw, in fractions of the viewport from its top-left corner: points
//! joined by `>`, strokes parted by `;`. `STRAP_WRAP` is a line to wrap a belt along, two points
//! joined by `>`, used when the demo's name has `wrap` in it. `STRAP_SET` is settings as
//! `name=value,...`, the width (`radius`, half of it) and `thickness` as fractions of the object's
//! half size. `STRAP_MIRROR` draws under X symmetry and `STRAP_BARE` leaves the handles out of the
//! picture. Each run prints what it drew and what one edit costs.

use std::time::Instant;

use glam::{Vec2, Vec3};
use sculpt_toolkit::Ctx;

use crate::tool::TubeTool;
use crate::types::{Tip, TubeSettings};

fn milliseconds(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Points in fractions of the viewport, `x,y>x,y>...`, in pixels.
fn way(text: &str, size: Vec2) -> Vec<Vec2> {
    text.split('>')
        .filter_map(|p| {
            let (x, y) = p.split_once(',')?;
            Some(Vec2::new(x.trim().parse().ok()?, y.trim().parse().ok()?) * size)
        })
        .collect()
}

impl TubeTool {
    pub(crate) fn demo_strap(&mut self, cx: &mut Ctx, what: &str) {
        let scene = cx.scene();
        let bounds = scene.bounds(scene.active());
        if bounds.is_empty() {
            return;
        }
        let reach = bounds.size().max_element() * 0.5;
        let mut settings = TubeSettings { radius: reach * 0.06, ..TubeSettings::default() }.strap();
        self.bare = std::env::var_os("STRAP_BARE").is_some();
        for item in std::env::var("STRAP_SET").unwrap_or_default().split(',') {
            let Some((name, value)) = item.split_once('=') else { continue };
            let value = value.trim();
            match (name.trim(), value.parse::<f32>()) {
                ("radius", Ok(v)) => settings.radius = reach * v,
                ("thickness", Ok(v)) => settings.thickness = reach * v,
                ("edge", Ok(v)) => settings.edge = v,
                ("gap", Ok(v)) => settings.gap = v,
                ("tight", Ok(v)) => settings.tight = v,
                ("smoothness", Ok(v)) => settings.smoothness = v,
                ("tip", _) => {
                    settings.tip = match value {
                        "round" => Tip::Round,
                        "point" => Tip::Point,
                        _ => Tip::Square,
                    }
                }
                (other, _) => eprintln!("strap: no setting called {other:?}"),
            }
        }
        if !what.contains("wrap") && !std::env::var("STRAP_SET").unwrap_or_default().contains("thickness") {
            settings.thickness = settings.radius * 0.3;
        }
        self.defaults = settings;
        self.defaults_sized_for_demo();
        if std::env::var_os("STRAP_MIRROR").is_some() {
            cx.set_mirror([true, false, false]);
        }

        let size = cx.view.size;
        let start = Instant::now();
        if what.contains("wrap") {
            let line = std::env::var("STRAP_WRAP").unwrap_or_else(|_| "0.2,0.52>0.8,0.5".to_owned());
            let ends = way(&line, size);
            if let [from, to] = ends[..] {
                let made = self.draw_wrap_line(cx, from, to);
                println!("strap: wrap {line:?} {}: {}", if made { "laid" } else { "NOT laid" }, self.status);
            }
        } else {
            let strokes = std::env::var("STRAP_STROKES").unwrap_or_else(|_| "0.35,0.3>0.5,0.45>0.62,0.7".to_owned());
            for stroke in strokes.split(';') {
                let way = way(stroke.trim(), size);
                let mut path: Vec<Vec2> = Vec::new();
                for pair in way.windows(2) {
                    path.extend((0..24).map(|step| pair[0].lerp(pair[1], step as f32 / 24.0)));
                }
                path.extend(way.last());
                let drawn = self.draw_stroke(cx, &path, false);
                let points = self.tubes.last().map_or(0, |t| t.state.points.len());
                let on = self.tubes.last().map_or(0, |t| t.state.points.iter().filter(|p| p.is_on_surface()).count());
                println!("strap: stroke {stroke:?} {}: {points} points, {on} on the surface", if drawn { "drawn" } else { "NOT drawn" });
            }
        }
        self.refresh(cx, true);
        println!("strap: {} laid and meshed in {:.1} ms", self.tubes.len(), milliseconds(start));
        // How the middle of the last strap lies: its width, and how square its way across is to the surface.
        if let Some(laid) = self.tubes.last().and_then(|t| t.copies.first()).and_then(|c| c.laid.as_ref()) {
            let k = laid.centre.len() / 2;
            if k < laid.centre.len() {
                println!(
                    "strap: middle {:.4} wide, {:.4} thick, way across . way up = {:.3}",
                    2.0 * laid.half_width[k],
                    laid.thickness,
                    laid.across[k].dot(laid.up[k]),
                );
            }
        }
        self.time_a_strap_edit(cx, reach);

        if what.contains("merged") {
            let start = Instant::now();
            self.merge_all(cx);
            self.wait_for_merge(cx);
            println!("strap: merged in {:.0} ms: {}", milliseconds(start), self.status);
        } else if what.contains("object") {
            self.keep_separate(cx);
            println!("strap: kept: {}", self.status);
        } else if what.contains("selected") {
            if let Some(tube) = self.tubes.len().checked_sub(1) {
                self.select_tube(tube, None);
            }
        } else {
            (self.active, self.selected_point) = (None, None);
        }
        self.refresh(cx, false);
        self.flush(cx);
    }

    /// The demo sizes the strap itself: the tool's own first sizing is not to undo it.
    fn defaults_sized_for_demo(&mut self) {
        self.mark_defaults_sized();
    }

    /// What one step of dragging a point of the last strap costs: laid again and meshed again.
    fn time_a_strap_edit(&mut self, cx: &Ctx, reach: f32) {
        let Some(tube) = self.tubes.len().checked_sub(1) else { return };
        let points = self.tubes[tube].state.points.clone();
        let wrap = self.tubes[tube].state.wrap;
        let middle = points.len() / 2;
        let (mut total, mut worst) = (0.0, 0.0f64);
        const STEPS: usize = 20;
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
        self.tubes[tube].state.wrap = wrap;
        self.refresh(cx, true);
        let live = &self.tubes[tube];
        println!(
            "strap: an edit rebuilds in {:.1} ms on average, {:.1} ms at worst ({} stations, {} preview triangles)",
            total / STEPS as f64,
            worst,
            live.copies.first().map_or(0, |c| c.samples.len()),
            live.preview.data().indices.len() / 3,
        );
    }
}
