//! Brush falloff: how strongly a brush acts at each distance from its centre.
//!
//! Every brush has a soft edge of its own. Focal Shift moves where that edge
//! falls without changing its shape, and a hand-drawn curve (ZBrush's brush
//! Curve) can replace it outright. A stroke bakes whichever applies into a
//! [`Falloff`] when it begins, so weighing a vertex is a few multiplies or a
//! table lookup and never a spline solve: a dab on a dense mesh weighs a
//! million of them.

use glam::Vec2;

/// How far Focal Shift can push the bias away from its neutral 0.5. At the
/// ends the smoothstep brushes reach half weight at 5% / 95% of the radius.
pub const MAX_FOCAL_BIAS: f32 = 0.45;

/// Spans in a baked curve (it holds one more sample than this).
pub const TABLE_SIZE: usize = 64;

/// Focal shift (-1 hard .. 0 none .. +1 soft) as the Schlick bias term
/// `1/b - 2`; 0 is the identity.
pub fn focal_k(focal_shift: f32) -> f32 {
    let b = 0.5 + MAX_FOCAL_BIAS * focal_shift.clamp(-1.0, 1.0);
    1.0 / b - 2.0
}

/// Focal shift on a normalised distance (0 centre, 1 edge): Schlick's bias,
/// which keeps both ends where they are and sends 0.5 to the bias. Negative
/// shift pulls distances toward the centre, so the brush stays near full
/// weight further out (harder); positive pushes them outward (softer,
/// pointier). Rational rather than `pow` so it is exact at the ends, cheap,
/// and with `k` = 0 gives back `u` bit for bit.
#[inline]
pub fn shifted(u: f32, k: f32) -> f32 {
    u / (k * (1.0 - u) + 1.0)
}

/// The shapes a hand-drawn curve can start from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FalloffPreset {
    Smooth,
    Sharp,
    Plateau,
    Linear,
}

impl FalloffPreset {
    pub const ALL: [FalloffPreset; 4] =
        [FalloffPreset::Smooth, FalloffPreset::Sharp, FalloffPreset::Plateau, FalloffPreset::Linear];

    pub fn name(self) -> &'static str {
        match self {
            FalloffPreset::Smooth => "Smooth",
            FalloffPreset::Sharp => "Sharp",
            FalloffPreset::Plateau => "Plateau",
            FalloffPreset::Linear => "Linear",
        }
    }
}

/// A hand-drawn falloff. Control points are (distance, weight): distance 0 at
/// the brush centre and 1 at its edge, weight 0..1. The first point always
/// sits at distance 0 and the last at distance 1; the ones between can go
/// anywhere.
///
/// Joined with a monotone cubic (Fritsch-Carlson), so the curve passes
/// through every point and never overshoots between two of them. A plain
/// spline rings below 0 or above 1 next to a sharp step, and a negative
/// weight would sculpt backwards.
#[derive(Clone, Debug, PartialEq)]
pub struct FalloffCurve {
    pub points: Vec<Vec2>,
}

impl FalloffCurve {
    pub fn preset(preset: FalloffPreset) -> FalloffCurve {
        let points: &[(f32, f32)] = match preset {
            // A smoothstep, what most brushes use when left alone.
            FalloffPreset::Smooth => &[(0.0, 1.0), (0.5, 0.5), (1.0, 0.0)],
            FalloffPreset::Sharp => &[(0.0, 1.0), (0.2, 0.45), (0.55, 0.1), (1.0, 0.0)],
            FalloffPreset::Plateau => &[(0.0, 1.0), (0.6, 1.0), (0.85, 0.35), (1.0, 0.0)],
            FalloffPreset::Linear => &[(0.0, 1.0), (1.0, 0.0)],
        };
        FalloffCurve { points: points.iter().map(|&(x, y)| Vec2::new(x, y)).collect() }
    }

    /// Keeps the points valid after an edit: in order of distance, the ends
    /// pinned to 0 and 1, every weight within 0..1.
    pub fn normalize(&mut self) {
        for p in &mut self.points {
            *p = p.clamp(Vec2::ZERO, Vec2::ONE);
        }
        self.points.sort_by(|a, b| a.x.total_cmp(&b.x));
        if self.points.is_empty() {
            self.points.push(Vec2::new(0.0, 1.0));
        }
        if self.points.len() == 1 {
            self.points.push(Vec2::new(1.0, 0.0));
        }
        self.points[0].x = 0.0;
        let last = self.points.len() - 1;
        self.points[last].x = 1.0;
    }

    /// The weight at `distance` (0 centre, 1 edge).
    pub fn evaluate(&self, distance: f32) -> f32 {
        let p = &self.points;
        let n = p.len();
        if n == 0 {
            return 1.0 - distance;
        }
        if distance <= p[0].x {
            return p[0].y;
        }
        if distance >= p[n - 1].x {
            return p[n - 1].y;
        }
        let mut k = 0;
        while k + 2 < n && distance > p[k + 1].x {
            k += 1;
        }
        let (a, b) = (p[k], p[k + 1]);
        let h = b.x - a.x;
        if h <= 1e-6 {
            return b.y;
        }
        let t = (distance - a.x) / h;
        let (m0, m1) = (self.tangent(k), self.tangent(k + 1));
        let (t2, t3) = (t * t, t * t * t);
        let y = (2.0 * t3 - 3.0 * t2 + 1.0) * a.y
            + (t3 - 2.0 * t2 + t) * h * m0
            + (-2.0 * t3 + 3.0 * t2) * b.y
            + (t3 - t2) * h * m1;
        y.clamp(0.0, 1.0)
    }

    /// Fritsch-Carlson tangents: zero at a local extreme or on a flat run,
    /// otherwise the harmonic mean of the two neighbouring slopes, which is
    /// what keeps each span from overshooting.
    fn tangent(&self, i: usize) -> f32 {
        let p = &self.points;
        let n = p.len();
        let slope = |j: usize| (p[j + 1].y - p[j].y) / (p[j + 1].x - p[j].x).max(1e-6);
        if n < 2 {
            return 0.0;
        }
        if i == 0 {
            return slope(0);
        }
        if i == n - 1 {
            return slope(n - 2);
        }
        let (d0, d1) = (slope(i - 1), slope(i));
        if d0 * d1 <= 0.0 { 0.0 } else { 2.0 / (1.0 / d0 + 1.0 / d1) }
    }

    fn bake(&self) -> [f32; TABLE_SIZE + 1] {
        std::array::from_fn(|i| self.evaluate(i as f32 / TABLE_SIZE as f32))
    }
}

/// What a stroke weighs vertices with: its brush's focal shift, and the
/// brush's hand-drawn curve baked to a table if it has one. Small and `Copy`,
/// so every dab carries its own and nothing is shared between threads.
#[derive(Clone, Copy)]
pub struct Falloff {
    k: f32,
    table: Option<[f32; TABLE_SIZE + 1]>,
}

impl Falloff {
    /// Every brush's own falloff, unshifted. Mask painting uses this: it has
    /// its Hardness control instead.
    pub const BUILT_IN: Falloff = Falloff { k: 0.0, table: None };

    pub fn new(focal_shift: f32, curve: Option<&FalloffCurve>) -> Falloff {
        Falloff { k: focal_k(focal_shift), table: curve.map(FalloffCurve::bake) }
    }

    /// The weight at normalised distance `u` (0 centre, 1 edge; the caller
    /// has already turned away anything beyond the edge). `built_in` is the
    /// brush's own falloff, given 1 at the centre and 0 at the edge; a
    /// hand-drawn curve replaces it. Focal shift applies to either.
    ///
    /// With no curve and no shift this is `built_in(1.0 - u)` to the last
    /// bit, which is what the brushes computed before curves existed.
    #[inline]
    pub fn at(&self, u: f32, built_in: impl FnOnce(f32) -> f32) -> f32 {
        let u = shifted(u, self.k);
        match &self.table {
            None => built_in(1.0 - u),
            Some(table) => {
                let x = u.clamp(0.0, 1.0) * TABLE_SIZE as f32;
                let i = (x as usize).clamp(0, TABLE_SIZE - 1);
                table[i] + (table[i + 1] - table[i]) * (x - i as f32)
            }
        }
    }
}
