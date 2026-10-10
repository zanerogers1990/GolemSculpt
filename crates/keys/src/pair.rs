//! The two objects being keyed, as the tool holds them: the scaffold.
//!
//! A pair is the halves WITHOUT keys (what every edit plans against and what
//! Apply builds from), the faces along which they mate, and the joints as
//! planned. It stays with the tool after Apply, so that undoing the Apply
//! hands the planned joints back exactly as they were.

use std::sync::Arc;

use glam::{Mat4, Vec3};
use rayon::prelude::*;
use sculpt_geom::edge_key::{EdgeSet, directed_edge_key};
use sculpt_toolkit::{MeshId, MeshView, ObjectId, Scene};

use crate::joint::KeyPart;
use crate::mating;
use crate::planner::{self, JointPlan, KeyPlan, KeySettings, PlannedKey};

/// How the tool came by its two objects, for saying so.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// The two selected objects.
    Selected,
    /// The selected object, and the one found touching it face to face.
    Found,
    /// The selected object, and one the user clicked.
    Clicked,
}

/// What the scene shows of a pair right now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Showing {
    /// The halves as they were planned against: the joints can be edited.
    Plain,
    /// The halves with this pair's keys built in: Apply has been done and
    /// not undone.
    Keyed,
    /// Anything else: an object is gone, or has other geometry now.
    Changed,
}

/// The keys of a plan and which side of each joint has the pegs: one step of
/// the tool's own history.
#[derive(Clone, Debug, PartialEq)]
pub struct Snapshot {
    keys: Vec<Vec<PlannedKey>>,
    peg_on_a: Vec<bool>,
}

pub struct Pair {
    pub a: ObjectId,
    pub b: ObjectId,
    pub source: Source,
    pub plan: KeyPlan,
    /// The halves' masks, one value per vertex, to carry through a build.
    pub masks: [Arc<Vec<f32>>; 2],
    /// The geometry the plan was made against, by id and by content.
    plain: [MeshId; 2],
    prints: [u64; 2],
    /// The geometry Apply put on the objects.
    pub keyed: Option<[MeshId; 2]>,
    /// What the scene showed when the tool last looked.
    pub showing: Showing,
    /// Each joint's cut-face outline, in the world the plan was made in.
    pub rims: Vec<Vec<Vec<Vec3>>>,
}

/// One object as a part to key, with what the pair remembers of it.
struct Half {
    part: KeyPart,
    mask: Vec<f32>,
    mesh: MeshId,
    print: u64,
}

fn half(scene: &dyn Scene, id: ObjectId) -> Option<Half> {
    let info = scene.info(id)?;
    let view = scene.mesh(id)?;
    if view.triangle_count() == 0 {
        return None;
    }
    Some(Half {
        part: KeyPart::new(view.to_tri_mesh(), info.matrix(), Vec::new()),
        mask: view.mask.to_vec(),
        mesh: view.id,
        print: fingerprint(&view),
    })
}

/// A number that changes when a mesh's vertices or triangles do. Mesh ids do
/// not change when a brush moves vertices, so this is how the tool notices
/// that a half has been sculpted since its joints were planned (a build from
/// the halves on record would throw that work away).
pub fn fingerprint(mesh: &MeshView) -> u64 {
    const PRIME: u64 = 1_099_511_628_211;
    const SEED: u64 = 14_695_981_039_346_656_037;
    const CHUNK: usize = 1 << 15;
    let mix = |h: u64, v: u32| (h ^ v as u64).wrapping_mul(PRIME);
    let positions = mesh.positions.par_chunks(CHUNK).map(|chunk| {
        chunk.iter().fold(SEED, |h, p| mix(mix(mix(h, p.x.to_bits()), p.y.to_bits()), p.z.to_bits()))
    });
    let indices = mesh.indices.par_chunks(CHUNK).map(|chunk| chunk.iter().fold(SEED, |h, &i| mix(h, i)));
    let parts: Vec<u64> = positions.chain(indices).collect();
    parts.iter().fold(SEED, |h, &part| (h ^ part).wrapping_mul(PRIME)) ^ mesh.positions.len() as u64
}

impl Pair {
    /// Makes a pair of two objects, if they touch face to face. With `place`
    /// the joints are planned automatically; without, the faces start empty.
    pub fn of(
        scene: &dyn Scene,
        a: ObjectId,
        b: ObjectId,
        source: Source,
        settings: &KeySettings,
        place: bool,
    ) -> Result<Pair, String> {
        let name = |id: ObjectId| scene.info(id).map_or("?".to_string(), |info| info.name.to_string());
        if a == b {
            return Err("Keys need two pieces: this is one object.".to_string());
        }
        let (Some(half_a), Some(half_b)) = (half(scene, a), half(scene, b)) else {
            return Err("One of the two pieces has no geometry.".to_string());
        };
        if half_a.mesh == half_b.mesh {
            return Err(format!("{} and {} are instances of one mesh; keys need two separate pieces.", name(a), name(b)));
        }
        let (mut half_a, mut half_b) = (half_a, half_b);
        let pairs = mating::find(&mut half_a.part, &mut half_b.part, 0.0);
        if pairs.is_empty() {
            let (a, b) = (name(a), name(b));
            return Err(format!("{a} and {b} do not touch face to face - keys go where two halves of a cut meet."));
        }
        Ok(Pair::planned([a, b], half_a, half_b, &pairs, source, settings, place))
    }

    fn planned(
        [a, b]: [ObjectId; 2],
        half_a: Half,
        half_b: Half,
        pairs: &[(usize, usize)],
        source: Source,
        settings: &KeySettings,
        place: bool,
    ) -> Pair {
        let plan = planner::plan(Arc::new(half_a.part), Arc::new(half_b.part), pairs, settings, place);
        let rims = plan.joints.iter().map(|jp| rim(&plan.a, jp)).collect();
        Pair {
            a,
            b,
            source,
            plan,
            masks: [Arc::new(half_a.mask), Arc::new(half_b.mask)],
            plain: [half_a.mesh, half_b.mesh],
            prints: [half_a.print, half_b.print],
            keyed: None,
            showing: Showing::Plain,
            rims,
        }
    }

    /// The pair of `a` and whichever other object touches it over the largest
    /// face. Hidden objects count: hiding one half is how the cut face of the
    /// other gets looked at.
    pub fn with_mate(scene: &dyn Scene, a: ObjectId, settings: &KeySettings, place: bool) -> Option<Pair> {
        let mut half_a = half(scene, a)?;
        let reach = scene.bounds(a);
        let mut best: Option<(f32, ObjectId, Half, Vec<Vec<u32>>, Vec<(usize, usize)>)> = None;
        for id in scene.objects() {
            let near = !scene.bounds(id).is_empty() && scene.bounds(id).expanded(1e-3).overlaps(&reach);
            if id == a || !near {
                continue;
            }
            let Some(mut other) = half(scene, id).filter(|other| other.mesh != half_a.mesh) else { continue };
            let pairs = mating::find(&mut half_a.part, &mut other.part, 0.0);
            let area = mating::mating_area(&half_a.part, &pairs);
            if !pairs.is_empty() && best.as_ref().is_none_or(|(most, ..)| area > *most) {
                best = Some((area, id, other, half_a.part.cut_faces.clone(), pairs));
            }
        }
        let (_, b, half_b, faces_a, pairs) = best?;
        half_a.part.cut_faces = faces_a;
        Some(Pair::planned([a, b], half_a, half_b, &pairs, Source::Found, settings, place))
    }

    pub fn has(&self, id: ObjectId) -> bool {
        self.a == id || self.b == id
    }

    /// Whether this is the pair of `x` and `y`, either way round.
    pub fn is(&self, x: ObjectId, y: ObjectId) -> bool {
        (self.a == x && self.b == y) || (self.a == y && self.b == x)
    }

    /// The two objects' names, as they are now.
    pub fn names(&self, scene: &dyn Scene) -> [String; 2] {
        [self.a, self.b].map(|id| scene.info(id).map_or("?".to_string(), |info| info.name.to_string()))
    }

    fn meshes(&self, scene: &dyn Scene) -> Option<[MeshId; 2]> {
        Some([scene.info(self.a)?.mesh, scene.info(self.b)?.mesh])
    }

    /// What the scene shows of the pair: compares mesh ids, so it is cheap
    /// enough to ask every frame.
    pub fn look(&self, scene: &dyn Scene) -> Showing {
        match self.meshes(scene) {
            Some(now) if now == self.plain => Showing::Plain,
            Some(now) if Some(now) == self.keyed => Showing::Keyed,
            _ => Showing::Changed,
        }
    }

    /// Whether the halves still have the vertices and triangles the joints
    /// were planned against: a pass over both meshes, for when the tool
    /// comes up.
    pub fn unsculpted(&self, scene: &dyn Scene) -> bool {
        let print = |id: ObjectId| scene.mesh(id).map(|view| fingerprint(&view));
        print(self.a) == Some(self.prints[0]) && print(self.b) == Some(self.prints[1])
    }

    /// From the world the plan was made in to where one of the objects is
    /// now: keys are drawn and picked through this, so they stay on their
    /// half if it has been moved since.
    pub fn display(&self, scene: &dyn Scene, on_a: bool) -> Mat4 {
        let (id, part) = if on_a { (self.a, &self.plan.a) } else { (self.b, &self.plan.b) };
        match scene.info(id) {
            Some(info) => info.matrix() * part.local_to_world().inverse(),
            None => Mat4::IDENTITY,
        }
    }

    pub fn snapshot(&self) -> Snapshot {
        Snapshot {
            keys: self.plan.joints.iter().map(|jp| jp.keys.clone()).collect(),
            peg_on_a: self.plan.joints.iter().map(|jp| jp.peg_on_a).collect(),
        }
    }

    /// Puts a snapshot's keys back and re-checks them against the settings
    /// as they are now.
    pub fn restore(&mut self, snapshot: Snapshot, settings: &KeySettings) {
        for ((jp, keys), peg_on_a) in self.plan.joints.iter_mut().zip(snapshot.keys).zip(snapshot.peg_on_a) {
            jp.keys = keys;
            jp.peg_on_a = peg_on_a;
            planner::validate_all(jp, settings);
        }
    }

    /// Takes over which keys a build found it could not make.
    pub fn take_verdicts(&mut self, built: &KeyPlan) {
        for (jp, from) in self.plan.joints.iter_mut().zip(&built.joints) {
            for (key, verdict) in jp.keys.iter_mut().zip(&from.keys) {
                key.valid = verdict.valid;
                key.problem.clone_from(&verdict.problem);
            }
        }
    }

    /// Records the geometry Apply put on the two objects.
    pub fn applied(&mut self, scene: &dyn Scene) {
        self.keyed = self.meshes(scene);
        self.showing = self.look(scene);
    }
}

/// A joint's cut face outline on half A: the edges of its face triangles
/// that border anything else, chained into loops.
fn rim(a: &KeyPart, jp: &JointPlan) -> Vec<Vec<Vec3>> {
    let indices = a.indices();
    let edges = || {
        jp.joint.faces_a.iter().flat_map(|&f| a.cut_faces[f].iter()).flat_map(|&t| {
            let c = &indices[t as usize * 3..t as usize * 3 + 3];
            (0..3).map(move |k| (c[k], c[(k + 1) % 3]))
        })
    };
    let directed: EdgeSet = edges().map(|(u, v)| directed_edge_key(u, v)).collect();
    let mut next: Vec<(u32, u32)> = edges().filter(|&(u, v)| !directed.contains(&directed_edge_key(v, u))).collect();
    next.sort_unstable();
    let follow = |u: u32| next.binary_search_by_key(&u, |e| e.0).ok().map(|i| next[i].1);

    let mut used = std::collections::HashSet::new();
    let mut loops = Vec::new();
    for &(start, _) in &next {
        let mut path = Vec::new();
        let mut at = start;
        while used.insert(at) {
            path.push(a.world()[at as usize]);
            match follow(at) {
                Some(to) => at = to,
                None => break,
            }
        }
        if path.len() > 2 {
            loops.push(path);
        }
    }
    loops
}
