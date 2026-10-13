//! Dynamic topology's brush side: when to refine, where, and how fine. The
//! mesh side (the splitting, and keeping everything derived from the
//! triangles consistent) is mesh/dyntopo.rs.
//!
//! Detail is tied to the BRUSH RADIUS (Blender's "Brush Detail"): a small
//! brush adds fine triangles where it works and a big one leaves the density
//! alone, so zooming in and shrinking the brush is how a sculptor asks for
//! more detail, and nothing balloons outside the stroke. Only the brushes
//! that deposit or carve refine (Standard, Clay, Crease, Inflate, Flatten,
//! Layer). Move and Pose hold the vertices they took hold of at the press
//! until the release, so the mesh must not change under them; Snake Hook is
//! left out as in the Unity app; and Smooth and mask painting have nothing to
//! gain from more triangles.

use glam::Vec3;

use super::Brush;
use crate::mesh::Mesh;

/// The longest edge dynamic topology leaves under the brush, as a fraction
/// of the brush radius: the range of the Detail slider and where it starts.
pub const MIN_DETAIL: f32 = 0.03;
pub const MAX_DETAIL: f32 = 0.5;
pub const DEFAULT_DETAIL: f32 = 0.15;

/// Dynamic topology stops adding triangles past this: memory, and the frame
/// rate of every other brush and tool on the object, scale with it.
pub const MAX_TRIANGLES: usize = 4_000_000;

/// Cost ceiling for one frame's refining. Longest edges go first, so a frame
/// that hits it has done the coarsest (most visible) part, and the rest
/// lands over the next few frames.
const MAX_SPLITS_PER_FRAME: usize = 3000;

/// The refined region reaches a little past the brush, so the rim of the
/// footprint, which the falloff still moves, is not left coarse against a
/// fine centre.
const RADIUS_MARGIN: f32 = 1.15;

/// Below this much travel (in brush radii) since the last refine, with
/// nothing left over from one that ran out of budget, there is nothing new
/// to refine.
const MIN_TRAVEL: f32 = 0.1;

/// The smallest edge dynamic topology will ever make, as a fraction of the
/// model's size, so a tiny brush on a big model cannot make triangles no one
/// can see.
const MIN_DETAIL_PER_MODEL_SIZE: f32 = 1e-4;

/// The most places along the path since the last refine that one refine
/// covers.
const MAX_PATH_SITES: usize = 16;

/// Whether strokes of `brush` add detail while dynamic topology is on.
pub fn refines(brush: Brush) -> bool {
    matches!(brush, Brush::Standard | Brush::Clay | Brush::Crease | Brush::Inflate | Brush::Flatten | Brush::Layer)
}

/// Where the stroke under way has refined so far.
struct Path {
    /// The brush centre at the last refine, or `None` before the first and
    /// after the pointer has been off the model.
    last: Option<Vec3>,
    /// The edge length that refine worked to.
    edge: f32,
    /// It stopped at the frame's budget with edges still to split.
    unfinished: bool,
    /// Where the brush had got to when a frame's budget was already spent:
    /// the stretch from `last` to here has not been refined at all yet.
    waiting: Option<Vec3>,
    splits_this_frame: usize,
    model_size: f32,
    /// How far off the path the stroke's dabs can land, in brush radii
    /// (the brush's Scatter): the refined region is that much wider.
    spread: f32,
}

/// Dynamic topology as the brush panel sets it, and as a stroke drives it.
pub struct DynTopo {
    pub enabled: bool,
    /// The longest edge left under the brush, as a fraction of its radius.
    pub detail: f32,
    /// The last refine stopped at the triangle limit; the panel says so.
    pub at_limit: bool,
    /// The most triangles a mesh is refined to: [`MAX_TRIANGLES`].
    limit: usize,
    /// The stroke under way, if its brush refines.
    path: Option<Path>,
    sites: Vec<Vec3>,
    images: Vec<Vec3>,
}

impl Default for DynTopo {
    fn default() -> Self {
        Self {
            enabled: false,
            detail: DEFAULT_DETAIL,
            at_limit: false,
            limit: MAX_TRIANGLES,
            path: None,
            sites: Vec::new(),
            images: Vec::new(),
        }
    }
}

impl DynTopo {
    /// A triangle limit low enough for a test to reach.
    #[cfg(test)]
    pub fn limit_triangles(&mut self, limit: usize) {
        self.limit = limit;
    }

    /// Whether a stroke of `brush` beginning now refines as it goes; `mask`
    /// is whether it paints the mask instead of sculpting.
    fn takes(&self, brush: Brush, mask: bool) -> bool {
        self.enabled && !mask && refines(brush)
    }

    /// Whether a stroke of `brush` beginning now could change the triangles
    /// of a mesh that has `triangles` of them. False at the triangle limit,
    /// where [`Mesh::refine`] turns back before it touches anything. What is
    /// only worth doing before a stroke that may grow the mesh asks this.
    pub fn may_grow(&self, brush: Brush, mask: bool, triangles: usize) -> bool {
        self.takes(brush, mask) && triangles + 2 <= self.limit
    }

    /// A stroke begins. `brush` is the one it uses (Smooth when Shift makes
    /// it so), `model_size` the largest side of the model and `spread` how
    /// far off its path the stroke's dabs can land, in brush radii.
    pub(super) fn begin(&mut self, brush: Brush, mask: bool, model_size: f32, spread: f32) {
        self.path = self.takes(brush, mask).then(|| Path {
            last: None,
            edge: 0.0,
            unfinished: false,
            waiting: None,
            splits_this_frame: 0,
            model_size,
            spread: spread.max(0.0),
        });
        if !self.enabled {
            self.at_limit = false;
        }
    }

    /// The pointer left the model: when it comes back the stroke carries on
    /// from there, not along a line through the air from where it left.
    pub(super) fn pause(&mut self) {
        if let Some(path) = &mut self.path {
            (path.last, path.waiting) = (None, None);
        }
    }

    /// Refines the surface under the brush at `at`, and everywhere it has
    /// travelled since the last refine, before the dabs for that stretch are
    /// placed, so that they land on the new vertices. `radius` is the brush
    /// radius on the model: the ring's, the largest a dab of the stroke can
    /// be, whatever the pen's pressure is making of it (a stroke that tapers
    /// to a point is refined along its whole length as finely as its thick
    /// end, and no finer). `copies` appends every symmetric copy of a point
    /// (the point itself among them) to a list: each copy of the brush
    /// refines the way its dabs will land.
    pub(super) fn refine(&mut self, mesh: &mut Mesh, at: Vec3, radius: f32, copies: &dyn Fn(Vec3, &mut Vec<Vec3>)) {
        let Some(path) = &mut self.path else { return };
        let reach = radius * (RADIUS_MARGIN + path.spread);
        let edge =
            (radius * self.detail.clamp(MIN_DETAIL, MAX_DETAIL)).max(path.model_size * MIN_DETAIL_PER_MODEL_SIZE);
        if let Some(last) = path.last {
            let rested = at.distance_squared(last) < (radius * MIN_TRAVEL).powi(2);
            if rested && !path.unfinished && edge == path.edge {
                return;
            }
        }
        let budget = MAX_SPLITS_PER_FRAME.saturating_sub(path.splits_this_frame);
        if budget == 0 {
            // This frame has done its share; the stretch waits for the next.
            path.unfinished = true;
            path.waiting = Some(at);
            return;
        }

        // The path since the last refine, a site every three quarters of the
        // reach, so a fast stroke is covered end to end rather than only
        // where each sample happened to land.
        self.sites.clear();
        if let Some(last) = path.last {
            let step = (reach * 0.75).max(1e-6);
            let steps = ((last.distance(at) / step).ceil() as usize).min(MAX_PATH_SITES);
            for i in 1..steps {
                self.sites.push(last.lerp(at, i as f32 / steps as f32));
            }
        }
        self.sites.push(at);
        // A dab on a mirror plane is its own image: one site does.
        let same = reach * reach * 1e-4;
        for k in 0..self.sites.len() {
            self.images.clear();
            copies(self.sites[k], &mut self.images);
            for (n, &image) in self.images.iter().enumerate() {
                let apart = |other: &Vec3| image.distance_squared(*other) > same;
                if apart(&self.sites[k]) && self.images[..n].iter().all(apart) {
                    self.sites.push(image);
                }
            }
        }

        let stats = mesh.refine(&self.sites, reach, edge, budget, self.limit);
        self.at_limit = stats.hit_limit;
        path.last = Some(at);
        path.waiting = None;
        path.edge = edge;
        path.unfinished = stats.hit_budget;
        path.splits_this_frame += stats.splits;
    }

    /// Once a frame, after the frame's pointer samples: carries on with a
    /// refine that ran out of budget if the pointer has rested since (a
    /// moving one carries on by itself), and opens the next frame's budget.
    pub(super) fn frame(&mut self, mesh: &mut Mesh, radius: f32, copies: &dyn Fn(Vec3, &mut Vec<Vec3>)) {
        let Some(path) = &mut self.path else { return };
        let resume = path.unfinished && path.splits_this_frame == 0;
        let at = path.waiting.or(path.last);
        path.splits_this_frame = 0;
        if let (true, Some(at)) = (resume, at) {
            self.refine(mesh, at, radius, copies);
            if let Some(path) = &mut self.path {
                path.splits_this_frame = 0;
            }
        }
    }
}
