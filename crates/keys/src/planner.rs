//! Automatic key placement, the fit check behind manual placement, and
//! building a plan into both halves at once.

use std::sync::Arc;

use glam::Vec2;
use sculpt_geom::{TriMesh, units};

use crate::builder::{self, Built};
use crate::joint::{KeyJoint, KeyPart};
use crate::joint_map::{JointMap, NOT_ENOUGH};
use crate::profile::{KeyProfile, KeyShape};
use crate::types::{KeyDimensions, KeyFeature, KeyFrame};

/// Which half of a cut gets the pegs.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum KeySide {
    /// The holes go into whichever half has more material behind the cut.
    #[default]
    Auto,
    /// Pegs on the first part (A).
    PegOnFirst,
    /// Pegs on the second part (B).
    PegOnSecond,
}

/// Everything the key code is asked for, in world units (`sculpt_geom::units`
/// converts). A size, height or count of zero means "choose for me".
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct KeySettings {
    pub shape: KeyShape,
    pub size: f32,
    /// A rectangle's length, as a multiple of its width.
    pub aspect: f32,
    pub corner_radius: f32,
    pub height: f32,
    pub clearance: f32,
    pub extra_depth: f32,
    pub tip_chamfer: f32,
    pub entry_chamfer: f32,
    pub taper_degrees: f32,
    /// Least material left round a hole, sideways and below its floor.
    pub min_wall: f32,
    pub count: usize,
    /// Turns every key in its own plane, from the orientation automatic
    /// placement picks.
    pub rotation_degrees: f32,
    pub side: KeySide,
}

impl Default for KeySettings {
    /// FDM defaults for a 0.4 mm nozzle: 0.15 mm clearance a side is a snug
    /// hand fit.
    fn default() -> KeySettings {
        KeySettings {
            shape: KeyShape::Circle,
            size: 0.0,
            aspect: 1.6,
            corner_radius: mm(0.6),
            height: 0.0,
            clearance: mm(0.15),
            extra_depth: mm(0.4),
            tip_chamfer: mm(0.4),
            entry_chamfer: mm(0.3),
            taper_degrees: 0.0,
            min_wall: mm(1.2),
            count: 0,
            rotation_degrees: 0.0,
            side: KeySide::Auto,
        }
    }
}

impl KeySettings {
    pub fn dimensions(&self, height: f32) -> KeyDimensions {
        KeyDimensions {
            height,
            clearance: self.clearance,
            extra_depth: self.extra_depth,
            tip_chamfer: self.tip_chamfer,
            entry_chamfer: self.entry_chamfer,
            taper_degrees: self.taper_degrees,
        }
    }
}

/// One key on a joint, as placed: in the joint's own plane (see
/// `KeyJoint::to_plane`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlannedKey {
    pub position: Vec2,
    /// Degrees, relative to the joint's principal direction.
    pub angle: f32,
    pub valid: bool,
    /// Why it will not be built, when it is not valid.
    pub problem: Option<String>,
}

impl PlannedKey {
    pub fn new(position: Vec2, angle: f32) -> PlannedKey {
        PlannedKey { position, angle, valid: true, problem: None }
    }
}

/// The keys for one joint, and everything they were sized from.
#[derive(Clone, Debug)]
pub struct JointPlan {
    pub joint: KeyJoint,
    pub map: JointMap,
    /// True: pegs on part A, holes sunk into B.
    pub peg_on_a: bool,
    pub size: f32,
    pub height: f32,
    pub dimensions: KeyDimensions,
    pub profile: Option<Arc<KeyProfile>>,
    pub keys: Vec<PlannedKey>,
    /// What the planner had to settle for, if anything: worth showing the
    /// user.
    pub note: Option<String>,
    /// The clearance map last asked for: which part the hole was in, how deep
    /// it needed, and the map.
    clear: Option<(bool, f32, Arc<Vec<f32>>)>,
}

impl JointPlan {
    fn new(joint: KeyJoint, map: JointMap) -> JointPlan {
        JointPlan {
            joint,
            map,
            peg_on_a: true,
            size: 0.0,
            height: 0.0,
            dimensions: KeyDimensions::default(),
            profile: None,
            keys: Vec::new(),
            note: None,
            clear: None,
        }
    }

    /// The profile, when it can be built.
    pub fn valid_profile(&self) -> Option<&Arc<KeyProfile>> {
        self.profile.as_ref().filter(|p| p.is_valid())
    }

    /// Clearance map for this plan's hole depth, kept until the depth or the
    /// side changes.
    pub fn clear(&mut self, wall: f32) -> Arc<Vec<f32>> {
        let need = self.dimensions.hole_depth() + wall;
        match &self.clear {
            Some((side, depth, map)) if *side == self.peg_on_a && (depth - need).abs() <= 1e-7 => map.clone(),
            _ => {
                let map = Arc::new(self.map.clearance(self.peg_on_a, need));
                self.clear = Some((self.peg_on_a, need, map.clone()));
                map
            }
        }
    }

    /// Where a key stands: on the face, with its axis pointing from the peg's
    /// part into the hole's.
    pub fn frame(&self, key: &PlannedKey) -> KeyFrame {
        let origin = self.joint.from_plane(key.position, self.map.surface_height(key.position));
        let axis = if self.peg_on_a { self.joint.axis } else { -self.joint.axis };
        KeyFrame::new(origin, axis, self.joint.u, key.angle)
    }

    /// A key's outline at `offset`, in the joint plane.
    pub fn outline(&self, key: &PlannedKey, offset: f32) -> Vec<Vec2> {
        let Some(profile) = &self.profile else { return Vec::new() };
        let frame = self.frame(key);
        profile.outline(offset).iter().map(|&p| self.joint.to_plane(frame.point(p, 0.0))).collect()
    }

    /// The hole's mouth outline, in the joint plane: the footprint every fit
    /// test is about.
    pub fn mouth(&self, key: &PlannedKey) -> Vec<Vec2> {
        self.outline(key, self.dimensions.mouth_offset())
    }

    /// The key whose hole mouth covers a point of the joint plane, the one
    /// added last first.
    pub fn key_at(&self, plane: Vec2) -> Option<usize> {
        (0..self.keys.len()).rev().find(|&k| sculpt_cut::polygon::contains(&self.mouth(&self.keys[k]), plane))
    }
}

/// All the joints between two parts and their keys.
#[derive(Clone, Debug)]
pub struct KeyPlan {
    pub a: Arc<KeyPart>,
    pub b: Arc<KeyPart>,
    pub joints: Vec<JointPlan>,
}

impl KeyPlan {
    /// Keys that will be built.
    pub fn key_count(&self) -> usize {
        self.joints.iter().flat_map(|j| &j.keys).filter(|k| k.valid).count()
    }

    /// Keys that will not: placed where they do not fit.
    pub fn invalid_count(&self) -> usize {
        self.joints.iter().flat_map(|j| &j.keys).filter(|k| !k.valid).count()
    }

    /// The joint with the largest face: the one that decides what "the peg
    /// side" means when joints differ.
    pub fn main_joint(&self) -> Option<&JointPlan> {
        self.joints.iter().max_by(|x, y| x.joint.area.total_cmp(&y.joint.area))
    }
}

fn mm(millimetres: f32) -> f32 {
    units::from_millimetres(millimetres)
}

/// A length in millimetres with up to `decimals` places and no trailing
/// zeros: "5", "2.5", "0.15".
pub fn show_mm(world: f32, decimals: usize) -> String {
    let text = format!("{:.decimals$}", units::to_millimetres(world));
    if text.contains('.') { text.trim_end_matches('0').trim_end_matches('.').to_string() } else { text }
}

/// Most any part of a joint's cut face may turn away from the joint's axis.
/// Past a right angle the face would be facing backwards; this leaves a
/// little margin before that.
const MAX_JOINT_SPREAD_DEGREES: f32 = 84.0;

/// Plans keys for every joint between `a` and `b`. `pairs` are mating cut
/// faces (index into `a.cut_faces`, index into `b.cut_faces`):
/// `sculpt_cut::Split::face_pairs`, or what `mating::find` returns. With
/// `place_keys` off the joints are only sized, ready for keys placed by hand.
pub fn plan(
    a: Arc<KeyPart>,
    b: Arc<KeyPart>,
    pairs: &[(usize, usize)],
    settings: &KeySettings,
    place_keys: bool,
) -> KeyPlan {
    let joints = KeyJoint::group(&a, &b, pairs)
        .into_iter()
        .map(|joint| {
            let map = JointMap::new(&a, &b, &joint);
            let mut jp = JointPlan::new(joint, map);
            if place_keys {
                auto_place(&mut jp, settings);
            } else {
                size(&mut jp, settings);
            }
            jp
        })
        .collect();
    KeyPlan { a, b, joints }
}

/// Sizes a joint's key without placing any (the tool's starting point when
/// keys are placed by hand): side, size and height as automatic placement
/// would choose them.
pub fn size(jp: &mut JointPlan, s: &KeySettings) {
    jp.keys.clear();
    jp.peg_on_a = choose_side(jp, s);
    let r0 = max_clear(&jp.map.clearance(jp.peg_on_a, 0.0));
    jp.size = if s.size > 0.0 { s.size } else { auto_size(r0) };
    jp.height = if s.height > 0.0 { s.height } else { auto_height(jp.size) };
    set_profile(jp, s);
}

/// Re-derives size, height and profile after the settings changed, keeping
/// the keys where they are, and re-checks every key.
pub fn resize(jp: &mut JointPlan, s: &KeySettings) {
    if s.side != KeySide::Auto {
        jp.peg_on_a = s.side == KeySide::PegOnFirst;
    }
    if s.size > 0.0 {
        jp.size = s.size;
    }
    if s.height > 0.0 {
        jp.height = s.height;
    }
    set_profile(jp, s);
    validate_all(jp, s);
}

fn set_profile(jp: &mut JointPlan, s: &KeySettings) {
    let mut d = s.dimensions(jp.height);
    let (profile, eased) = builder::fit_profile(s.shape, jp.size, s.aspect, s.corner_radius, &mut d, jp.joint.edge);
    jp.profile = Some(Arc::new(profile));
    jp.dimensions = d;
    if eased {
        jp.note = Some(format!(
            "tip chamfer reduced to {} mm to suit the {} at this size",
            show_mm(d.tip_chamfer, 2),
            s.shape.name()
        ));
    }
}

fn auto_size(face_inradius: f32) -> f32 {
    (face_inradius * 0.55).clamp(mm(2.0), mm(10.0))
}

fn auto_height(size: f32) -> f32 {
    size.clamp(mm(1.5), mm(12.0))
}

fn max_clear(clear: &[f32]) -> f32 {
    clear.iter().copied().fold(0.0, f32::max)
}

fn choose_side(jp: &JointPlan, s: &KeySettings) -> bool {
    match s.side {
        KeySide::PegOnFirst => true,
        KeySide::PegOnSecond => false,
        // The hole needs depth, the peg does not: sink the holes into
        // whichever half has more material behind the cut (a typical depth,
        // not the single deepest column).
        KeySide::Auto => percentile(&jp.map, true, 0.7) >= percentile(&jp.map, false, 0.7),
    }
}

fn percentile(map: &JointMap, hole_in_b: bool, q: f32) -> f32 {
    let mut depths: Vec<f32> =
        (0..map.len()).filter(|&i| map.eligible(i)).map(|i| map.depth(i, hole_in_b).min(1e6)).collect();
    if depths.is_empty() {
        return 0.0;
    }
    depths.sort_unstable_by(f32::total_cmp);
    depths[((q * depths.len() as f32) as usize).min(depths.len() - 1)]
}

// ------------------------------------------------------- automatic placement

/// Places a joint's keys afresh from the settings, replacing any it had.
pub fn auto_place(jp: &mut JointPlan, s: &KeySettings) {
    jp.keys.clear();
    jp.note = None;
    if jp.map.eligible_cells() == 0 {
        jp.note = Some("the cut face is too steep or uneven here for keys".to_string());
        set_defaults(jp, s);
        return;
    }
    // Keys only work if the halves pull straight apart along the key axis:
    // the cut face must be a height field over it, never turned away from it,
    // never folded back over itself. A gently curved lasso cut passes; one
    // that wraps round a corner does not (those halves come apart sideways,
    // along the cut, where no peg could engage).
    if jp.joint.spread > MAX_JOINT_SPREAD_DEGREES || jp.map.folded_fraction() > 0.02 {
        jp.note = Some(
            "the cut face curves round too far for keys - the halves do not pull straight apart; a straight-line cut keys cleanly"
                .to_string(),
        );
        set_defaults(jp, s);
        return;
    }

    jp.peg_on_a = choose_side(jp, s);
    let r0 = max_clear(&jp.map.clearance(jp.peg_on_a, 0.0));
    let mut size = if s.size > 0.0 { s.size } else { auto_size(r0) };
    let mut height = if s.height > 0.0 { s.height } else { auto_height(size) };

    // A lone round key is a hinge: the parts turn on it. With the count and
    // size both left to it, the planner would rather have two somewhat
    // smaller round keys than one big one, so it tries that first, down to
    // 60% of the size it would have used.
    if s.count == 0 && s.shape == KeyShape::Circle {
        let (mut try_size, mut try_height) = (size, height);
        for _ in 0..5 {
            jp.size = try_size;
            jp.height = try_height;
            set_profile(jp, s);
            if jp.valid_profile().is_some() && arrange(jp, s, 2).is_ok() {
                return;
            }
            if s.size > 0.0 {
                break;
            }
            try_size *= 0.88;
            if try_size < size * 0.6 || try_size < mm(2.0) {
                break;
            }
            if s.height <= 0.0 {
                try_height = auto_height(try_size);
            }
        }
    }

    let mut why = String::new();
    for _ in 0..14 {
        jp.size = size;
        jp.height = height;
        set_profile(jp, s);
        if jp.valid_profile().is_none() {
            why = format!("a {} key cannot be made at {} mm", s.shape.name(), show_mm(size, 1));
        } else {
            match arrange(jp, s, 1) {
                Ok(()) => return,
                Err(reason) => why = reason,
            }
        }

        // Did not fit. Ease off whatever was left to choose (depth first: a
        // shorter peg loses little; then size) and try again.
        if s.height <= 0.0 && height > mm(1.5) + 1e-6 {
            height = mm(1.5).max(height * 0.75);
        } else if s.size <= 0.0 && size > mm(1.5) + 1e-6 {
            size = mm(1.5).max(size * 0.8);
            if s.height <= 0.0 {
                height = auto_height(size);
            }
        } else {
            break;
        }
    }
    let open = jp.map.open_columns(jp.peg_on_a);
    jp.note = Some(if open * 2 > jp.map.eligible_cells() {
        "no key: the part is not closed behind this cut face (it has holes or open edges), so a socket has nothing to sit in"
            .to_string()
    } else if why.is_empty() {
        "no room for a key: the cut face is too small".to_string()
    } else {
        format!("no room for a key: {why}")
    });
}

fn set_defaults(jp: &mut JointPlan, s: &KeySettings) {
    jp.size = if s.size > 0.0 { s.size } else { mm(4.0) };
    jp.height = if s.height > 0.0 { s.height } else { auto_height(jp.size) };
    set_profile(jp, s);
}

/// Places keys on every separate stretch of the face: at least one on each
/// that can take one. Fails (with a reason) only when none could or, with
/// `minimum_on_largest` above one, when the largest stretch cannot take that
/// many (no reason given then: the caller goes on to try fewer).
fn arrange(jp: &mut JointPlan, s: &KeySettings, minimum_on_largest: usize) -> Result<(), String> {
    jp.keys.clear();
    let wall = s.min_wall;
    let clear = jp.clear(wall);
    let Some(profile) = jp.profile.clone() else { return Err("no profile".to_string()) };

    let mouth = profile.outline(jp.dimensions.mouth_offset());
    let r_circ = mouth.iter().map(|p| p.length()).fold(0.0, f32::max);
    let r_in = mouth.iter().map(|p| p.length()).fold(f32::MAX, f32::min);

    let mut placed = 0;
    for (r, region) in regions(&jp.map).iter().enumerate() {
        let least = if r == 0 { minimum_on_largest } else { 1 };
        // Cells a key could possibly centre on (it needs at least its
        // inradius clear).
        let mut candidates: Vec<usize> = region.iter().copied().filter(|&i| clear[i] >= r_in + wall).collect();
        if candidates.is_empty() {
            if least > 1 {
                jp.keys.clear();
                return Err(String::new());
            }
            continue;
        }

        let wanted = if s.count > 0 { s.count } else { least.max(auto_count(jp, &profile, region)) };
        let mut done = false;
        for count in (least..=wanted).rev() {
            let keys = if count == 1 {
                place_one(jp, s, &mut candidates, &clear, wall)
            } else {
                place_many(jp, s, &candidates, &clear, wall, count, r_circ)
            };
            let Some(keys) = keys else { continue };
            placed += keys.len();
            jp.keys.extend(keys);
            if s.count > 0 && count < s.count {
                jp.note = Some(format!("only {count} of {} keys fit on one part of the cut", s.count));
            } else if s.count == 0 && count == 1 && profile.is_rotationally_symmetric() {
                jp.note = Some(
                    "one round key fits - the parts can still turn on it; a keyed shape (or two keys) will stop that"
                        .to_string(),
                );
            }
            done = true;
            break;
        }
        if !done && least > 1 {
            jp.keys.clear();
            return Err(String::new());
        }
    }
    if placed > 0 {
        return Ok(());
    }
    Err(format!(
        "not enough flat face, or not enough material behind it, for a {} mm key {} mm deep",
        show_mm(jp.size, 1),
        show_mm(jp.height, 1)
    ))
}

/// Separate stretches of usable face (8-connected), largest first.
fn regions(map: &JointMap) -> Vec<Vec<usize>> {
    let (w, h) = (map.width(), map.height());
    let mut seen = vec![false; w * h];
    let mut regions = Vec::new();
    let mut stack = Vec::new();
    for start in 0..w * h {
        if !map.eligible(start) || seen[start] {
            continue;
        }
        let mut region = Vec::new();
        seen[start] = true;
        stack.push(start);
        while let Some(i) = stack.pop() {
            region.push(i);
            let (x, y) = (i % w, i / w);
            for ny in y.saturating_sub(1)..=(y + 1).min(h - 1) {
                for nx in x.saturating_sub(1)..=(x + 1).min(w - 1) {
                    let j = ny * w + nx;
                    if !seen[j] && map.eligible(j) {
                        seen[j] = true;
                        stack.push(j);
                    }
                }
            }
        }
        regions.push(region);
    }
    regions.sort_by_key(|region| std::cmp::Reverse(region.len()));
    regions
}

/// One key for an ordinary face; two where one alone would leave something
/// to be desired: a round key (which the parts could turn on) or a long,
/// narrow face.
fn auto_count(jp: &JointPlan, profile: &KeyProfile, region: &[usize]) -> usize {
    if profile.is_rotationally_symmetric() || elongation(&jp.map, region) > 2.2 { 2 } else { 1 }
}

/// How many times longer than wide a stretch of face is.
fn elongation(map: &JointMap, region: &[usize]) -> f32 {
    let n = region.len() as f64;
    if region.len() < 2 {
        return 1.0;
    }
    let (mut sx, mut sy, mut sxx, mut syy, mut sxy) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0.0f64);
    for &i in region {
        let (x, y) = ((i % map.width()) as f64, (i / map.width()) as f64);
        sx += x;
        sy += y;
        sxx += x * x;
        syy += y * y;
        sxy += x * y;
    }
    let (mx, my) = (sx / n, sy / n);
    let (cxx, cyy, cxy) = (sxx / n - mx * mx, syy / n - my * my, sxy / n - mx * my);
    let (tr, det) = (cxx + cyy, cxx * cyy - cxy * cxy);
    let disc = (tr * tr / 4.0 - det).max(0.0).sqrt();
    let (l1, l2) = (tr / 2.0 + disc, tr / 2.0 - disc);
    if l2 > 1e-9 { (l1 / l2).sqrt() as f32 } else { 100.0 }
}

/// The turns worth trying for a key: its natural one first.
fn angles(profile: &KeyProfile, s: &KeySettings) -> Vec<f32> {
    let r = s.rotation_degrees;
    if profile.is_rotationally_symmetric() {
        return vec![r];
    }
    vec![r, r + 90.0, r + 180.0, r + 270.0, r + 45.0, r + 135.0]
}

/// A key at `position` at the first turn it fits at.
fn fit_at(jp: &JointPlan, s: &KeySettings, position: Vec2, clear: &[f32], wall: f32) -> Option<PlannedKey> {
    let profile = jp.profile.as_ref()?;
    angles(profile, s)
        .into_iter()
        .map(|angle| PlannedKey::new(position, angle))
        .find(|key| jp.map.fits(&jp.mouth(key), clear, wall).is_ok())
}

/// The single most-interior spot the key fits in, trying its natural
/// orientation first.
fn place_one(
    jp: &JointPlan,
    s: &KeySettings,
    candidates: &mut [usize],
    clear: &[f32],
    wall: f32,
) -> Option<Vec<PlannedKey>> {
    candidates.sort_by(|&p, &q| clear[q].total_cmp(&clear[p]).then(p.cmp(&q)));
    // A key centred where the face is widest reads best, but on a face with a
    // thin obstruction the widest point can be just off the true middle, so
    // the first few hundred best-cleared cells are all fair game, widest
    // first.
    candidates.iter().take(600).find_map(|&c| fit_at(jp, s, jp.map.cell_centre(c), clear, wall)).map(|key| vec![key])
}

/// `count` keys spread evenly over the usable face: farthest-point seeding,
/// then a few rounds of Lloyd relaxation (each key to the middle of the cells
/// nearest it), so two keys on a long face land about a quarter of the way in
/// from each end.
fn place_many(
    jp: &JointPlan,
    s: &KeySettings,
    candidates: &[usize],
    clear: &[f32],
    wall: f32,
    count: usize,
    r_circ: f32,
) -> Option<Vec<PlannedKey>> {
    // Only cells where the key fits at ANY turn, so relaxation can move
    // freely.
    let cells: Vec<Vec2> =
        candidates.iter().filter(|&&i| clear[i] >= r_circ + wall).map(|&i| jp.map.cell_centre(i)).collect();
    if cells.len() < count {
        return None;
    }

    let mean = cells.iter().copied().sum::<Vec2>() / cells.len() as f32;
    let mut seeds = vec![farthest(&cells, &[mean])];
    while seeds.len() < count {
        seeds.push(farthest(&cells, &seeds));
    }
    let extremes = seeds.clone();

    for _ in 0..10 {
        let mut sum = vec![Vec2::ZERO; count];
        let mut n = vec![0usize; count];
        for &c in &cells {
            let best = (0..count).min_by(|&i, &j| c.distance_squared(seeds[i]).total_cmp(&c.distance_squared(seeds[j])))?;
            sum[best] += c;
            n[best] += 1;
        }
        for k in (0..count).filter(|&k| n[k] > 0) {
            seeds[k] = nearest(&cells, sum[k] / n[k] as f32);
        }
    }

    // Relaxed positions sit in the middle of their share of the face, which
    // on a round face puts two keys under half the face's radius apart: often
    // too close. So if they crowd, they are eased out toward the extremes the
    // seeding found, a step at a time, until they are far enough apart that
    // neither hole's wall runs into the other's.
    let spacing = 2.0 * r_circ + wall;
    (0..=4).find_map(|step| {
        let t = step as f32 / 4.0;
        let at: Vec<Vec2> = (0..count).map(|k| nearest(&cells, seeds[k].lerp(extremes[k], t))).collect();
        let spaced = (0..count).all(|i| (i + 1..count).all(|j| at[i].distance(at[j]) >= spacing));
        if !spaced {
            return None;
        }
        at.iter().map(|&p| fit_at(jp, s, p, clear, wall)).collect::<Option<Vec<PlannedKey>>>()
    })
}

/// The cell farthest from every point of `from`.
fn farthest(cells: &[Vec2], from: &[Vec2]) -> Vec2 {
    let reach = |c: Vec2| from.iter().map(|f| c.distance_squared(*f)).fold(f32::MAX, f32::min);
    let mut best = (cells[0], -1.0f32);
    for &c in cells {
        let d = reach(c);
        if d > best.1 {
            best = (c, d);
        }
    }
    best.0
}

fn nearest(cells: &[Vec2], p: Vec2) -> Vec2 {
    let mut best = (cells[0], f32::MAX);
    for &c in cells {
        let d = c.distance_squared(p);
        if d < best.1 {
            best = (c, d);
        }
    }
    best.0
}

// ---------------------------------------------------------- manual placement

/// Re-checks one key where it stands: on the face, clear of the edges by the
/// minimum wall, enough material behind it for the hole, and not crowding
/// another key.
pub fn validate(jp: &mut JointPlan, index: usize, s: &KeySettings) {
    let verdict = check(jp, index, s);
    let key = &mut jp.keys[index];
    key.valid = verdict.is_ok();
    key.problem = verdict.err();
}

/// `validate` for every key of a joint: whether a key crowds its neighbour is
/// decided by both, so moving one re-checks them all.
pub fn validate_all(jp: &mut JointPlan, s: &KeySettings) {
    for index in 0..jp.keys.len() {
        validate(jp, index, s);
    }
}

fn check(jp: &mut JointPlan, index: usize, s: &KeySettings) -> Result<(), String> {
    let profile = match &jp.profile {
        Some(profile) if profile.is_valid() => profile.clone(),
        Some(profile) => return Err(profile.problem().unwrap_or("no profile").to_string()),
        None => return Err("no profile".to_string()),
    };
    let clear = jp.clear(s.min_wall);
    let key = &jp.keys[index];
    let mouth = jp.mouth(key);
    if let Err(problem) = jp.map.fits(&mouth, &clear, s.min_wall) {
        let depth = jp.map.min_depth_under(&mouth, jp.peg_on_a);
        let need = jp.dimensions.hole_depth() + s.min_wall;
        if depth > 0.0 && depth < need && problem == NOT_ENOUGH {
            return Err(format!(
                "only {} mm of material behind the face here - the hole needs {} mm",
                show_mm(depth, 1),
                show_mm(need, 1)
            ));
        }
        return Err(problem.to_string());
    }
    let r_circ = profile.circumradius(jp.dimensions.mouth_offset());
    let crowded = jp
        .keys
        .iter()
        .enumerate()
        .any(|(other, k)| other != index && k.position.distance(key.position) < 2.0 * r_circ + s.min_wall * 0.5);
    if crowded { Err("too close to another key".to_string()) } else { Ok(()) }
}

// ------------------------------------------------------------------- building

/// Both halves with a plan's keys built in.
#[derive(Clone, Debug, Default)]
pub struct BuildResult {
    /// `None` when the keys kept failing to build; see `first_problem`.
    pub a: Option<Built>,
    pub b: Option<Built>,
    /// Keys built into both halves.
    pub built: usize,
    /// Keys of the plan that are not valid, whether they were placed that
    /// way or failed to build.
    pub failed: usize,
    /// The first reason a key could not be built, if any.
    pub first_problem: Option<String>,
}

impl BuildResult {
    /// The two meshes, when there are any.
    pub fn meshes(&self) -> Option<(&TriMesh, &TriMesh)> {
        Some((&self.a.as_ref()?.mesh, &self.b.as_ref()?.mesh))
    }
}

/// Builds every valid key of the plan into both halves. A key whose peg or
/// hole cannot be built is dropped from BOTH halves (and marked invalid, with
/// the reason), so the parts never come out with a peg that has nowhere to
/// go.
pub fn build(plan: &mut KeyPlan) -> BuildResult {
    let mut result = BuildResult::default();
    for _ in 0..4 {
        let (mut features_a, mut features_b) = (Vec::new(), Vec::new());
        let mut owners: Vec<(usize, usize)> = Vec::new();
        for (j, jp) in plan.joints.iter_mut().enumerate() {
            let Some(profile) = jp.valid_profile().cloned() else { continue };
            for k in 0..jp.keys.len() {
                if !jp.keys[k].valid {
                    continue;
                }
                let cell = jp.map.cell_of(jp.keys[k].position);
                let faces = cell.and_then(|i| jp.map.face_a(i).zip(jp.map.face_b(i)));
                let Some((face_a, face_b)) = faces else {
                    jp.keys[k].valid = false;
                    jp.keys[k].problem = Some("off the cut face".to_string());
                    continue;
                };
                let frame = jp.frame(&jp.keys[k]);
                let (peg_patch, hole_patch) = if jp.peg_on_a { (face_a, face_b) } else { (face_b, face_a) };
                let (peg, hole): (KeyFeature, KeyFeature) =
                    builder::feature_pair(&profile, jp.dimensions, frame, peg_patch, hole_patch);
                let (for_a, for_b) = if jp.peg_on_a { (peg, hole) } else { (hole, peg) };
                features_a.push(for_a);
                features_b.push(for_b);
                owners.push((j, k));
            }
        }

        // The halves are independent: built side by side.
        let (built_a, built_b) =
            rayon::join(|| builder::build(&plan.a, &features_a), || builder::build(&plan.b, &features_b));

        let mut dropped = false;
        for (i, &(j, k)) in owners.iter().enumerate() {
            let Some(error) = built_a.errors[i].clone().or_else(|| built_b.errors[i].clone()) else { continue };
            let key = &mut plan.joints[j].keys[k];
            key.valid = false;
            key.problem = Some(error.clone());
            result.first_problem.get_or_insert(error);
            dropped = true;
        }
        // Rebuild both halves without the key that failed.
        if dropped {
            continue;
        }

        result.a = Some(built_a);
        result.b = Some(built_b);
        result.built = owners.len();
        result.failed = plan.invalid_count();
        return result;
    }
    result.first_problem.get_or_insert_with(|| "keys kept failing to build".to_string());
    result
}
