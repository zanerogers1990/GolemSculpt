//! The Deform card on the web: Bend, Bend Arc, Twist, Taper, Flatten,
//! Inflate, Stretch and Lattice, under the Transform card while the gizmo is
//! up, as on the desktop.
//!
//! The maths is the desktop's own `src/deform/` (compiled in as
//! `crate::deform`). This file is the desktop's `app/deform.rs`,
//! `app/deform/handles.rs`, `app/deform/mirror.rs`, `app/deform/radial.rs`
//! and `ui/deform.rs` cut down to the web version, with the same words:
//!
//! - While a deformer is picked and the Transform tool is up, a session is
//!   open on the selected objects. Every frame the card's settings have
//!   changed, every mesh is put where the deformer has it, worked out from
//!   where it was when the session opened. Apply (Enter) keeps it as one undo
//!   step; Cancel (Esc) puts every vertex back as it was.
//! - A change of the selection, of the gizmo's axes or of an object's place
//!   applies what is on the model and opens a fresh session; so does putting
//!   the Transform tool away, and Undo.
//! - Masked parts stay where they are; hidden ones too.
//! - The symmetry is the brushes' own, on the object's own axes (the web
//!   version has no world symmetry): a point of the Lattice's cage takes its
//!   mirror images with it, and under radial symmetry what the dragged points
//!   do is repeated round the axis. A deformation that would make a mirrored
//!   model lopsided says so on the status line.

use std::sync::Arc;

use eframe::egui::{self, Color32, Key, Modifiers, Pos2, Stroke as Line, Ui, vec2};
use glam::{DAffine3, DMat3, DVec3, Mat4, Vec2, Vec3};
use rayon::prelude::*;

use super::ui::theme::{self, ACCENT, ButtonStyle};
use super::{Drag, GolemApp, Mode, Step};
use crate::deform::{self, Anchor, Bounds, Deformer, Frame, Kind, Params, Target, lattice};
use crate::gizmo::{self, Shape};
use crate::mesh::Mesh;
use crate::mesh::layers::Ride;
use crate::scene::Drawn;
use crate::sculpt::{Radial, Settings};

/// The card's colour.
const ACCENT_DEFORM: Color32 = Color32::from_rgb(230, 170, 90);
/// How near the pointer has to be to a handle to take it, in points.
const GRAB_PX: f32 = 11.0;
const AXIS_COLORS: [[u8; 3]; 3] = [[255, 64, 64], [89, 255, 89], [77, 140, 255]];
const TIP_COLOR: [u8; 4] = [255, 214, 64, 255];
/// How near a point's mirror image has to come to another point for the two
/// to pair, as a share of the cage's size along each axis.
const PAIRS: f64 = 0.02;

/// What the Deform card and its session keep between frames.
#[derive(Default)]
pub(crate) struct DeformState {
    /// Whether a deformer is picked: the gizmo then shows the deformer's
    /// handles in place of its arrows.
    pub on: bool,
    pub params: Params,
    session: Option<Session>,
    grab: Option<Grab>,
}

/// One mesh a session is deforming: an object's.
struct Part {
    object: u64,
    model: Mat4,
    target: Target,
}

/// What a session was opened on: when any of it changes, it is applied and
/// another opened.
#[derive(Clone, PartialEq, Debug)]
struct SessionKey {
    objects: Vec<u64>,
    models: Vec<Mat4>,
    frame: Frame,
}

struct Session {
    key: SessionKey,
    frame: Frame,
    bounds: Bounds,
    parts: Vec<Part>,
    shown: Option<Params>,
    shown_copies: Option<Vec<DAffine3>>,
    /// The copies of the symmetry, and the symmetry they were worked out for.
    radial: Option<(([bool; 3], Radial), Option<Vec<DAffine3>>)>,
    warned: bool,
}

impl Session {
    fn pending(&self) -> bool {
        self.shown.as_ref().is_some_and(|p| !p.is_identity())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Handle {
    Start,
    End,
    Tip,
    Point(usize),
}

struct Grab {
    handle: Handle,
    on: Vec3,
    normal: Vec3,
    offset: Vec3,
}

/// What the card asks for.
enum Ask {
    Pick(Option<Kind>),
    Apply,
    Cancel,
}

fn ray_plane(origin: Vec3, dir: Vec3, point: Vec3, normal: Vec3) -> Option<Vec3> {
    let denom = dir.dot(normal);
    if denom.abs() < 1e-6 {
        return None;
    }
    let t = (point - origin).dot(normal) / denom;
    (t > 0.0).then(|| origin + dir * t)
}

fn disc(c: Vec2, r: f32) -> Vec<Vec2> {
    (0..20).map(|k| c + Vec2::from_angle(k as f32 / 20.0 * std::f32::consts::TAU) * r).collect()
}

/// How the settings read on the status line: "bent 45° along Y".
fn done_words(p: &Params) -> String {
    let axis = ["X", "Y", "Z"][p.axis];
    match p.kind {
        Kind::Bend => format!("bent {:.0}° along {axis}", p.angle),
        Kind::BendArc => format!("bent into an arc along {axis}"),
        Kind::Twist => format!("twisted {:.0}° about {axis}", p.angle),
        Kind::Taper => format!("tapered along {axis} from {:.0}% to {:.0}%", p.scales[0] * 100.0, p.scales[1] * 100.0),
        Kind::Flatten => format!("flattened {:.0}% along {axis}", p.flatten * 100.0),
        Kind::Inflate if p.inflate < 0.0 => format!("deflated by {:.1}% of its size", -p.inflate * 100.0),
        Kind::Inflate => format!("inflated by {:.1}% of its size", p.inflate * 100.0),
        Kind::Stretch => format!("stretched to {:.0}% along {axis}", p.stretch * 100.0),
        Kind::Lattice => "reshaped with the lattice".to_owned(),
    }
}

/// "Bent "Cube" 45° along Y": the name after the verb.
fn insert_name(words: &str, who: &str) -> String {
    match words.split_once(' ') {
        Some((verb, rest)) => format!("{verb} {who} {rest}"),
        None => format!("{words} {who}"),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or(String::new(), |c| c.to_uppercase().collect::<String>() + chars.as_str())
}

/// The status line's warning that `kind` breaks the mirror across `planes`.
fn asymmetry_words(kind: Kind, planes: &str, name: &str) -> String {
    let advice = match kind {
        Kind::Twist => {
            "A twist turns one half one way and its mirror image the other, so it cannot keep a mirrored model mirrored."
        }
        Kind::Bend => {
            "Bend along an axis that lies on the mirror plane, toward a direction along it (0° or 180°), to keep the halves alike."
        }
        Kind::Lattice => {
            "The cage is not even about the mirror plane (the model is wider on one side, or the gizmo's axes are turned from the plane), so its points cannot move in mirrored pairs."
        }
        _ => "Work along an axis that lies on the mirror plane to keep the halves alike.",
    };
    format!(
        "This {} is not mirror-symmetric across {planes}: the two halves of \"{name}\" will come out different. {advice}",
        kind.name().to_lowercase()
    )
}

/// Whether `deformer` maps the mirror image (across the object's own mirror
/// plane `axis`) of each of a sample of the mesh's vertices to the mirror
/// image of where it maps the vertex (the desktop's `keeps_mirror`).
fn keeps_mirror(mesh: &Mesh, target: &Target, deformer: &Deformer, axis: usize) -> bool {
    let flip = |p: DVec3| {
        let mut q = p;
        q[axis] = -q[axis];
        q
    };
    let n = mesh.vertex_count();
    let stride = (n / 256).max(1);
    let size = mesh.bounds().size().max_element().max(1e-12) as f64;
    (0..n).step_by(stride).all(|v| {
        let p = mesh.start_pos(v as u32).as_dvec3();
        let normal = mesh.start_nrm(v as u32).as_dvec3();
        let there = target.map(deformer, p, normal);
        let mirrored = target.map(deformer, flip(p), flip(normal));
        mirrored.distance(flip(there)) <= size * 1e-5
    })
}

// ---- the Lattice under mirror symmetry (the desktop's app/deform/mirror.rs) ----

/// The object's own mirror planes as reflections of the world.
fn planes(mirror: [bool; 3], model: Mat4) -> Vec<DAffine3> {
    let model = DAffine3::from_mat4(model.as_dmat4());
    (0..3)
        .filter(|&a| mirror[a])
        .map(|a| {
            let mut signs = DVec3::ONE;
            signs[a] = -1.0;
            model * DAffine3::from_scale(signs) * model.inverse()
        })
        .collect()
}

struct Across {
    partner: Vec<usize>,
    linear: DMat3,
}

fn snapped(m: DMat3) -> DMat3 {
    let snap = |x: f64| {
        let r = x.round();
        if r.abs() <= 1.0 && (x - r).abs() < 1e-6 { r } else { x }
    };
    DMat3::from_cols_array(&m.to_cols_array().map(snap))
}

fn across(rest: &[DVec3], size: DVec3, frame: &Frame, plane: DAffine3) -> Option<Across> {
    let into = DAffine3::from_rotation_translation(frame.rotation, frame.origin);
    let map = into.inverse() * plane * into;
    let reach = |d: DVec3| (d.abs() / size.max(DVec3::splat(1e-12))).max_element();
    let partner = rest
        .iter()
        .map(|&q| {
            let image = map.transform_point3(q);
            let (j, d) =
                rest.iter().enumerate().map(|(j, &r)| (j, reach(r - image))).min_by(|a, b| a.1.total_cmp(&b.1))?;
            (d <= PAIRS).then_some(j)
        })
        .collect::<Option<Vec<usize>>>()?;
    Some(Across { partner, linear: snapped(map.matrix3.into()) })
}

fn mirror_point(
    cage: &mut lattice::Lattice,
    moved: usize,
    bounds: &Bounds,
    axis: usize,
    span: (f64, f64),
    frame: &Frame,
    planes: &[DAffine3],
) {
    let mut rest = cage.clone();
    rest.rest();
    let points: Vec<DVec3> = (0..rest.len()).map(|i| lattice::point(&rest, i, bounds, axis, span)).collect();
    let (_, size) = lattice::cage_box(bounds, axis, span);
    let mut ways: Vec<(Vec<usize>, DMat3)> = vec![((0..rest.len()).collect(), DMat3::IDENTITY)];
    for &plane in planes {
        let Some(plane) = across(&points, size, frame, plane) else { continue };
        let more: Vec<(Vec<usize>, DMat3)> = ways
            .iter()
            .map(|(partner, linear)| (partner.iter().map(|&j| plane.partner[j]).collect(), plane.linear * *linear))
            .collect();
        ways.extend(more);
    }
    if ways.len() == 1 {
        return;
    }
    let way = DVec3::from_array(cage.moved[moved].map(f64::from)) * size;
    let mut sums: Vec<(DVec3, u32)> = vec![(DVec3::ZERO, 0); cage.len()];
    for (partner, linear) in &ways {
        let sum = &mut sums[partner[moved]];
        sum.0 += *linear * way;
        sum.1 += 1;
    }
    for (j, (sum, n)) in sums.into_iter().enumerate() {
        if n > 0 {
            let way = if n == 1 { sum } else { sum / n as f64 };
            cage.moved[j] = (way / size.max(DVec3::splat(1e-12))).as_vec3().to_array();
        }
    }
}

// ---- the Lattice under radial symmetry (the desktop's app/deform/radial.rs) ----

/// Every copy a drag of the cage is repeated as under the brushes' symmetry,
/// but the drag itself, as maps of the gizmo's frame; `None` with no turns.
fn copies(mirror: [bool; 3], radial: Radial, model: Mat4, frame: &Frame) -> Option<Vec<DAffine3>> {
    if radial.repeats() < Radial::MIN_COUNT {
        return None;
    }
    let mut settings = Settings::default();
    settings.set_symmetry(mirror, radial, None);
    let group = settings.symmetry_group();
    if group.radial_count() < 2 {
        return None;
    }
    let into = frame.from_mesh(model);
    let back = into.inverse();
    let copies =
        group.iter().filter(|op| !op.is_identity()).map(|op| into * DAffine3::from_mat4(op.to_mat4().as_dmat4()) * back);
    Some(copies.collect())
}

fn apart(d: DVec3, size: DVec3) -> f64 {
    (d.abs() / size.max(DVec3::splat(1e-12))).max_element()
}

fn at_rest(cage: &lattice::Lattice, bounds: &Bounds, axis: usize, span: (f64, f64)) -> (Vec<DVec3>, DVec3) {
    let mut rest = cage.clone();
    rest.rest();
    let points = (0..rest.len()).map(|i| lattice::point(&rest, i, bounds, axis, span)).collect();
    (points, lattice::cage_box(bounds, axis, span).1)
}

fn hold(cage: &mut lattice::Lattice, moved: usize, bounds: &Bounds, axis: usize, span: (f64, f64), copies: &[DAffine3]) {
    let (points, size) = at_rest(cage, bounds, axis, span);
    let home = points[moved];
    let way = DVec3::from_array(cage.moved[moved].map(f64::from)) * size;
    let (mut sum, mut n) = (way, 1u32);
    for copy in copies {
        if apart(copy.transform_point3(home) - home, size) <= PAIRS {
            sum += copy.matrix3 * way;
            n += 1;
        }
    }
    if n > 1 {
        cage.moved[moved] = (sum / f64::from(n) / size.max(DVec3::splat(1e-12))).as_vec3().to_array();
    }
    for (j, &point) in points.iter().enumerate() {
        let twin = || copies.iter().any(|copy| apart(copy.transform_point3(home) - point, size) <= PAIRS);
        if j != moved && cage.moved[j] != [0.0; 3] && twin() {
            cage.moved[j] = [0.0; 3];
        }
    }
}

fn ghosts(cage: &lattice::Lattice, bounds: &Bounds, axis: usize, span: (f64, f64), copies: &[DAffine3]) -> Vec<DVec3> {
    let (points, size) = at_rest(cage, bounds, axis, span);
    let mut out = Vec::new();
    for (i, &home) in points.iter().enumerate() {
        if cage.moved.get(i).is_none_or(|m| *m == [0.0; 3]) {
            continue;
        }
        let now = lattice::point(cage, i, bounds, axis, span);
        for copy in copies {
            if apart(copy.transform_point3(home) - home, size) > PAIRS {
                out.push(copy.transform_point3(now));
            }
        }
    }
    out
}

impl GolemApp {
    /// Whether the deformers are up: a deformer picked, the Transform tool up.
    pub(crate) fn deform_on(&self) -> bool {
        self.shaping.deform.on && self.mode == Mode::Transform
    }

    /// Whether a deformation is on the model and not yet applied.
    fn deform_pending(&self) -> bool {
        self.shaping.deform.session.as_ref().is_some_and(Session::pending)
    }

    /// The mesh of object `id`: the app's for the active one.
    pub(crate) fn mesh_of_mut(&mut self, id: u64) -> Option<&mut Mesh> {
        if id == self.active {
            return Some(&mut self.mesh);
        }
        self.objects.iter_mut().find(|o| o.id == id).and_then(|o| o.mesh.as_mut())
    }

    /// What the renderer draws of object `id` made again, after its mesh
    /// changed while it was not the active one.
    pub(crate) fn redrawn(&mut self, id: u64) {
        if id == self.active {
            return;
        }
        if let Some(o) = self.objects.iter_mut().find(|o| o.id == id) {
            if let Some(mesh) = o.mesh.as_mut() {
                mesh.flush_normals();
                o.drawn = Some(Arc::new(Drawn::of(mesh)));
            }
        }
    }

    /// What the selection would be deformed as now: the selected objects
    /// that show, their placements, and the gizmo's frame.
    fn deform_key_now(&self) -> Option<SessionKey> {
        let objects: Vec<&crate::scene::Object> = self
            .selected
            .iter()
            .filter_map(|id| self.objects.iter().find(|o| o.id == *id && o.visible))
            .collect();
        let targets: Vec<gizmo::Target> = objects
            .iter()
            .map(|o| {
                let b = o.world_bounds(self.mesh_of(o));
                let radius = if b.is_empty() { 0.5 } else { (b.max - b.min).length() * 0.5 };
                gizmo::Target::object(o, radius.max(1e-4))
            })
            .collect();
        let origin = gizmo::pivot_of(&targets)?;
        let rotation = gizmo::rotation_of(&targets, self.transform.orientation);
        let frame = Frame { origin: origin.as_dvec3(), rotation: rotation.as_dquat() };
        Some(SessionKey { objects: objects.iter().map(|o| o.id).collect(), models: objects.iter().map(|o| o.matrix()).collect(), frame })
    }

    /// Runs every frame before the mesh is sent to be drawn: opens, renews
    /// or closes the session, and puts the model where the card has it.
    pub(crate) fn deform_frame(&mut self) {
        if !self.deform_on() {
            self.deform_commit();
            return;
        }
        let key = self.deform_key_now();
        let mut stale = self.shaping.deform.session.as_ref().is_some_and(|s| Some(&s.key) != key.as_ref());
        // A mesh put in another's place under the session is let go.
        let parts: Vec<(u64, u64)> = self
            .shaping
            .deform
            .session
            .as_ref()
            .map_or(Vec::new(), |s| s.parts.iter().map(|p| (p.object, p.target.uid)).collect());
        for (object, uid) in parts {
            if self.mesh_of_mut(object).map(|m| m.uid) != Some(uid) {
                stale = true;
            }
        }
        if stale && self.shaping.deform.grab.is_none() {
            self.deform_commit();
        }
        if self.shaping.deform.session.is_none() {
            if let Some(key) = key {
                self.deform_open(key);
            }
        }
        self.deform_preview();
    }

    fn deform_open(&mut self, key: SessionKey) {
        let frame = key.frame;
        let mut parts = Vec::new();
        for (&object, &model) in key.objects.iter().zip(&key.models) {
            let Some(m) = self.mesh_of_mut(object) else { continue };
            parts.push(Part { object, model, target: Target::begin(m, model, &frame) });
        }
        let Some(bounds) =
            parts.iter().map(|p| p.target.bounds).reduce(|a, b| Bounds::new(a.min.min(b.min), a.max.max(b.max)))
        else {
            return;
        };
        self.shaping.deform.session =
            Some(Session { key, frame, bounds, parts, shown: None, shown_copies: None, radial: None, warned: false });
    }

    /// The card's settings, cleaned up, if they are the Lattice's and one
    /// point of its cage has been moved since the model was last shown.
    fn lattice_lone_change(&self) -> Option<(usize, Params)> {
        let clean = self.shaping.deform.params.sanitized();
        if clean.kind != Kind::Lattice {
            return None;
        }
        let session = self.shaping.deform.session.as_ref()?;
        let was = match &session.shown {
            Some(shown) if shown.kind == Kind::Lattice && shown.lattice.count == clean.lattice.count => {
                shown.lattice.moved.clone()
            }
            _ => vec![[0.0; 3]; clean.lattice.len()],
        };
        let mut changed = (0..clean.lattice.len()).filter(|&i| clean.lattice.moved[i] != was[i]);
        let (Some(moved), None) = (changed.next(), changed.next()) else { return None };
        Some((moved, clean))
    }

    fn deform_lattice_copies(&mut self) -> Option<Vec<DAffine3>> {
        if self.shaping.deform.params.kind != Kind::Lattice {
            return None;
        }
        let s = &self.sculptor.settings;
        let symmetry = (s.symmetry, s.radial);
        let session = self.shaping.deform.session.as_ref()?;
        let part = session.parts.first()?;
        if let Some((was, copies)) = &session.radial {
            if *was == symmetry {
                return copies.clone();
            }
        }
        let made = copies(symmetry.0, symmetry.1, part.model, &session.frame);
        self.shaping.deform.session.as_mut()?.radial = Some((symmetry, made.clone()));
        made
    }

    fn deform_mirror_lattice(&mut self) {
        let Some((moved, clean)) = self.lattice_lone_change() else { return };
        let Some(session) = self.shaping.deform.session.as_ref() else { return };
        let Some(part) = session.parts.first() else { return };
        let planes = planes(self.sculptor.settings.symmetry, part.model);
        if planes.is_empty() {
            return;
        }
        let span = session.bounds.span(clean.axis, clean.range);
        let mut cage = clean.lattice.clone();
        mirror_point(&mut cage, moved, &session.bounds, clean.axis, span, &session.frame, &planes);
        self.shaping.deform.params.lattice = cage;
    }

    fn deform_radial_lattice(&mut self, copies: &[DAffine3]) {
        let Some((moved, clean)) = self.lattice_lone_change() else { return };
        let Some(session) = self.shaping.deform.session.as_ref() else { return };
        let span = session.bounds.span(clean.axis, clean.range);
        let mut cage = clean.lattice.clone();
        hold(&mut cage, moved, &session.bounds, clean.axis, span, copies);
        self.shaping.deform.params.lattice = cage;
    }

    /// Puts every part where the card's settings have it, if they changed.
    fn deform_preview(&mut self) {
        let copies = self.deform_lattice_copies();
        match &copies {
            Some(copies) => self.deform_radial_lattice(copies),
            None => self.deform_mirror_lattice(),
        }
        let params = self.shaping.deform.params.sanitized();
        let Some(session) = self.shaping.deform.session.as_ref() else { return };
        if session.shown.as_ref() == Some(&params) && session.shown_copies == copies {
            return;
        }
        if !session.pending() && !params.is_identity() {
            if let Some(why) = self.deform_refusal() {
                let card = &mut self.shaping.deform;
                card.params = card.params.reset_values();
                self.status = why;
                return;
            }
        }
        let Some(mut session) = self.shaping.deform.session.take() else { return };
        let deformer = Deformer::new(&params, &session.bounds).map(|d| match &copies {
            Some(copies) => d.repeated(copies),
            None => d,
        });
        for part in &mut session.parts {
            if let Some(mesh) = self.mesh_of_mut(part.object) {
                part.target.apply(mesh, deformer.as_ref(), params.axis);
            }
            self.redrawn(part.object);
        }
        session.shown = Some(params.clone());
        session.shown_copies = copies.clone();
        self.sculptor.last_hit = None;
        // Repeated by the symmetry's own copies, the Lattice is as symmetric as that symmetry can be.
        if let Some(deformer) = deformer.as_ref().filter(|_| copies.is_none()) {
            self.deform_check_symmetry(&mut session, &params, deformer);
        }
        self.shaping.deform.session = Some(session);
    }

    fn deform_refusal(&mut self) -> Option<String> {
        let session = self.shaping.deform.session.as_ref()?;
        if session.parts.iter().all(|p| p.target.is_pinned()) {
            let one = session.parts.len() == 1;
            return Some(
                if one {
                    "Everything of the object is masked, so nothing can move. Clear or invert the mask first."
                } else {
                    "Everything of the selected objects is masked, so nothing can move. Clear or invert the mask first."
                }
                .to_owned(),
            );
        }
        let parts: Vec<u64> = session.parts.iter().map(|p| p.object).collect();
        parts.into_iter().find_map(|id| self.mesh_of_mut(id)?.layer_refusal())
    }

    /// Says once a session if the deformation would make a mirrored model lopsided.
    fn deform_check_symmetry(&mut self, session: &mut Session, params: &Params, deformer: &Deformer) {
        if session.warned {
            return;
        }
        let mirror = self.sculptor.settings.symmetry;
        let axes: Vec<usize> = (0..3).filter(|&a| mirror[a]).collect();
        if axes.is_empty() {
            return;
        }
        for part in &session.parts {
            let Some(mesh) = self.mesh_of_mut(part.object) else { continue };
            let broken: Vec<&str> =
                axes.iter().filter(|&&a| !keeps_mirror(mesh, &part.target, deformer, a)).map(|&a| ["X", "Y", "Z"][a]).collect();
            if broken.is_empty() {
                continue;
            }
            let name = self.objects.iter().find(|o| o.id == part.object).map(|o| o.name.clone()).unwrap_or_default();
            self.status = asymmetry_words(params.kind, &broken.join(" and "), &name);
            session.warned = true;
            return;
        }
    }

    /// Keeps what the session has done as one undo step, and closes it.
    pub(crate) fn deform_commit(&mut self) {
        let Some(session) = self.shaping.deform.session.take() else { return };
        self.shaping.deform.grab = None;
        let params = session.shown.clone().unwrap_or_default();
        let ride = if params.kind == Kind::Inflate { Ride::Still } else { Ride::Turned };
        let mut steps = Vec::new();
        let mut names = Vec::new();
        let mut masked = false;
        for part in session.parts {
            let Some(mesh) = self.mesh_of_mut(part.object) else { continue };
            masked |= mesh.mask.par_iter().with_min_len(1 << 16).any(|&m| m > 0.0);
            if let Some(delta) = part.target.end(mesh, ride) {
                steps.push(Step::Stroke(part.object, delta));
                names.push(self.objects.iter().find(|o| o.id == part.object).map(|o| o.name.clone()).unwrap_or_default());
            }
            self.redrawn(part.object);
        }
        let card = &mut self.shaping.deform;
        card.params = card.params.reset_values();
        if steps.is_empty() {
            return;
        }
        let step = if steps.len() == 1 { steps.pop().unwrap() } else { Step::Many(steps) };
        self.push_undo(step);
        self.mark_changed();
        self.sculptor.last_hit = None;
        let who = match names.len() {
            1 => format!("\"{}\"", names[0]),
            n => format!("{n} objects"),
        };
        let mut text = format!("{}.", capitalised(&insert_name(&done_words(&params), &who)));
        if masked {
            text += " The masked part stayed where it was.";
        }
        if session.warned {
            text += " The two halves of the mirrored model now differ.";
        }
        text += " Undo (Ctrl+Z) puts it back.";
        self.status = text;
    }

    /// Puts the model back as it was before the deformation, keeping nothing.
    fn deform_cancel(&mut self) {
        let Some(session) = self.shaping.deform.session.take() else { return };
        self.shaping.deform.grab = None;
        let had = session.pending();
        for part in session.parts {
            if let Some(mesh) = self.mesh_of_mut(part.object) {
                part.target.cancel(mesh);
            }
            self.redrawn(part.object);
        }
        let card = &mut self.shaping.deform;
        card.params = card.params.reset_values();
        self.sculptor.last_hit = None;
        if had {
            self.status = "The deformation was cancelled; the model is as it was.".into();
        }
    }

    fn deform_act(&mut self, ask: Ask) {
        match ask {
            Ask::Pick(kind) => {
                self.deform_commit();
                let card = &mut self.shaping.deform;
                match kind {
                    Some(kind) => {
                        card.on = true;
                        card.params = Params { kind, ..card.params.clone() }.reset_values();
                        self.mode = Mode::Transform;
                        // The handles are the deformer's now, not the pivot's.
                        self.shaping.pivot.on = false;
                        self.status = format!("{}: {}", kind.name(), kind.tip());
                    }
                    None => card.on = false,
                }
            }
            Ask::Apply => self.deform_commit(),
            Ask::Cancel => self.deform_cancel(),
        }
    }

    /// Esc cancels what is on the model, or with nothing on it puts the
    /// deformers away; Enter applies. True if a key was the card's.
    pub(crate) fn deform_keys(&mut self, ctx: &egui::Context) -> bool {
        if !self.deform_on() || self.drag != Drag::None {
            return false;
        }
        let (escape, enter) =
            ctx.input_mut(|i| (i.consume_key(Modifiers::NONE, Key::Escape), i.consume_key(Modifiers::NONE, Key::Enter)));
        if escape {
            if self.deform_pending() {
                self.deform_cancel();
            } else {
                self.deform_commit();
                self.shaping.deform.on = false;
            }
        } else if enter {
            self.deform_commit();
        }
        escape || enter
    }

    // ---- the handles ------------------------------------------------------------

    fn deform_handles(&self) -> Vec<(Handle, Vec3)> {
        let Some(session) = self.shaping.deform.session.as_ref() else { return Vec::new() };
        let p = self.shaping.deform.params.sanitized();
        let at = |q: DVec3| session.frame.to_world(q).as_vec3();
        let [start, end] = deform::range_ends(&p, &session.bounds);
        let mut out = vec![(Handle::Start, at(start)), (Handle::End, at(end))];
        match p.kind {
            Kind::BendArc => out.push((Handle::Tip, at(deform::tip_point(&p, &session.bounds)))),
            Kind::Lattice => {
                let span = session.bounds.span(p.axis, p.range);
                for i in 0..p.lattice.len() {
                    out.push((Handle::Point(i), at(lattice::point(&p.lattice, i, &session.bounds, p.axis, span))));
                }
            }
            _ => {}
        }
        out
    }

    fn deform_handle_at(&self, px: Vec2, ppp: f32) -> Option<(Handle, Vec3)> {
        let reach = GRAB_PX * ppp;
        let near = |h: &(Handle, Vec3)| self.view.project(h.1).map(|at| at.distance(px)).filter(|&d| d <= reach);
        let handles = self.deform_handles();
        let best = |range: bool| {
            handles
                .iter()
                .filter(|h| matches!(h.0, Handle::Start | Handle::End) == range)
                .filter_map(|h| near(h).map(|d| (*h, d)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(h, _)| h)
        };
        best(false).or_else(|| best(true))
    }

    /// A press on the view while the deformers are up: on a handle it takes
    /// the drag. Elsewhere `None`, and the press does what it would.
    pub(super) fn deform_pressed(&mut self, px: Vec2, ppp: f32) -> Option<Drag> {
        if !self.deform_on() {
            return None;
        }
        let (handle, at) = self.deform_handle_at(px, ppp)?;
        let session = self.shaping.deform.session.as_ref()?;
        let forward = self.view.forward;
        let normal = match handle {
            Handle::Start | Handle::End => {
                let axis = session.frame.rotation.as_quat() * Vec3::AXES[self.shaping.deform.params.axis.min(2)];
                let n = axis.cross(forward).cross(axis);
                if n.length_squared() < 1e-8 { forward } else { n.normalize() }
            }
            Handle::Tip | Handle::Point(_) => forward,
        };
        let (origin, dir) = self.view.ray(px);
        let hit = ray_plane(origin, dir, at, normal).unwrap_or(at);
        if let Handle::Point(i) = handle {
            self.shaping.deform.params.lattice.picked = i;
        }
        self.shaping.deform.grab = Some(Grab { handle, on: at, normal, offset: at - hit });
        Some(Drag::Gizmo)
    }

    /// The pointer moved during a handle drag. True if it was one.
    pub(crate) fn deform_moved(&mut self, pos: Vec2) -> bool {
        let Some(grab) = self.shaping.deform.grab.as_ref() else { return false };
        let Some(session) = self.shaping.deform.session.as_ref() else { return true };
        let (origin, dir) = self.view.ray(pos);
        let Some(hit) = ray_plane(origin, dir, grab.on, grab.normal) else { return true };
        let to = session.frame.from_world((hit + grab.offset).as_dvec3());
        let (bounds, handle) = (session.bounds, grab.handle);
        let p = &mut self.shaping.deform.params;
        let clean = p.sanitized();
        match handle {
            Handle::Start | Handle::End => {
                let (lo, hi) = (bounds.min[clean.axis], bounds.max[clean.axis]);
                let f = (((to[clean.axis] - lo) / (hi - lo).max(1e-12)) as f32).clamp(0.0, 1.0);
                let gap = 0.01;
                match handle {
                    Handle::Start => p.range[0] = f.min(clean.range[1] - gap).max(0.0),
                    _ => p.range[1] = f.max(clean.range[0] + gap).min(1.0),
                }
            }
            Handle::Tip => p.tip = deform::tip_for(&clean, &bounds, to),
            Handle::Point(i) => {
                let span = bounds.span(clean.axis, clean.range);
                if i < p.lattice.moved.len() {
                    p.lattice.moved[i] = lattice::moved_for(&clean.lattice, i, &bounds, clean.axis, span, to);
                }
            }
        }
        true
    }

    /// The button came up. True if it ended a handle drag.
    pub(crate) fn deform_released(&mut self) -> bool {
        self.shaping.deform.grab.take().is_some()
    }

    /// Draws the deformer's handles over the viewport.
    pub(crate) fn draw_deform(&self, painter: &egui::Painter, ppp: f32) {
        if !self.deform_on() {
            return;
        }
        let Some(session) = self.shaping.deform.session.as_ref() else { return };
        let p = self.shaping.deform.params.sanitized();
        let px = |q: DVec3| self.view.project(session.frame.to_world(q).as_vec3());
        let grabbed = self.shaping.deform.grab.as_ref().map(|g| g.handle);
        let hot = grabbed.or_else(|| {
            self.pointer_px.filter(|_| self.drag == Drag::None).and_then(|px| self.deform_handle_at(px, ppp)).map(|h| h.0)
        });
        let [r, g, b] = AXIS_COLORS[p.axis];
        let mut out = Vec::new();
        let [start, end] = deform::range_ends(&p, &session.bounds);
        let (mut first, mut last) = (start, end);
        first[p.axis] = session.bounds.min[p.axis];
        last[p.axis] = session.bounds.max[p.axis];
        if let (Some(a), Some(z)) = (px(first), px(last)) {
            out.push(Shape::Line { points: vec![a, z], width: 2.0 * ppp, color: [r, g, b, 90] });
        }
        if let (Some(a), Some(z)) = (px(start), px(end)) {
            out.push(Shape::Line { points: vec![a, z], width: 3.0 * ppp, color: [r, g, b, 255] });
        }
        if p.kind == Kind::Lattice {
            self.lattice_shapes(&p, session, hot, ppp, &mut out);
        }
        if p.kind == Kind::BendArc {
            let anchor = if p.anchor == Anchor::End { end } else { start };
            if let (Some(a), Some(t)) = (px(anchor), px(deform::tip_point(&p, &session.bounds))) {
                out.push(Shape::Line { points: vec![a, t], width: 1.5 * ppp, color: [255, 214, 64, 140] });
                let size = if hot == Some(Handle::Tip) { 10.0 } else { 8.0 } * ppp;
                let diamond = |s: f32| {
                    vec![t + Vec2::new(0.0, -s), t + Vec2::new(s, 0.0), t + Vec2::new(0.0, s), t + Vec2::new(-s, 0.0)]
                };
                out.push(Shape::Fill { points: diamond(size + 1.5 * ppp), color: [15, 15, 20, 255] });
                out.push(Shape::Fill { points: diamond(size), color: TIP_COLOR });
            }
        }
        for (handle, at) in [(Handle::Start, start), (Handle::End, end)] {
            let Some(c) = px(at) else { continue };
            let size = if hot == Some(handle) { 8.5 } else { 7.0 } * ppp;
            out.push(Shape::Fill { points: disc(c, size + 1.5 * ppp), color: [r, g, b, 255] });
            out.push(Shape::Fill { points: disc(c, size), color: [245, 245, 245, 255] });
        }
        paint_shapes(painter, &out, ppp);
    }

    fn lattice_shapes(&self, p: &Params, session: &Session, hot: Option<Handle>, ppp: f32, out: &mut Vec<Shape>) {
        let span = session.bounds.span(p.axis, p.range);
        let cage = &p.lattice;
        let at: Vec<Option<Vec2>> = (0..cage.len())
            .map(|i| self.view.project(session.frame.to_world(lattice::point(cage, i, &session.bounds, p.axis, span)).as_vec3()))
            .collect();
        for i in 0..cage.len() {
            let ijk = cage.ijk(i);
            for c in 0..3 {
                if ijk[c] + 1 >= cage.count[c] {
                    continue;
                }
                let mut next = ijk;
                next[c] += 1;
                if let (Some(a), Some(b)) = (at[i], at[cage.index(next)]) {
                    out.push(Shape::Line { points: vec![a, b], width: 1.5 * ppp, color: [235, 235, 235, 150] });
                }
            }
        }
        if let Some((_, Some(copies))) = &session.radial {
            for ghost in ghosts(cage, &session.bounds, p.axis, span, copies) {
                let Some(c) = self.view.project(session.frame.to_world(ghost).as_vec3()) else { continue };
                out.push(Shape::Fill { points: disc(c, 4.0 * ppp), color: [15, 15, 20, 120] });
                out.push(Shape::Fill { points: disc(c, 3.0 * ppp), color: [245, 245, 245, 140] });
            }
        }
        for (i, c) in at.iter().enumerate() {
            let Some(c) = *c else { continue };
            let picked = i == cage.picked;
            let size = if hot == Some(Handle::Point(i)) { 6.5 } else { 5.0 } * ppp;
            out.push(Shape::Fill { points: disc(c, size + 1.5 * ppp), color: [15, 15, 20, 255] });
            out.push(Shape::Fill { points: disc(c, size), color: if picked { TIP_COLOR } else { [245, 245, 245, 255] } });
        }
    }

    // ---- the card -----------------------------------------------------------------

    /// The Deform card, under the Transform card while the gizmo is up.
    pub(crate) fn deform_card(&mut self, ui: &mut Ui) {
        let tips = self.ui.tips;
        let pending = self.deform_pending();
        let icons: Vec<Option<egui::TextureId>> =
            Kind::ALL.iter().map(|k| self.shaping.icons.get(ui.ctx(), k.icon())).collect();
        let card = &mut self.shaping.deform;
        let summary = if card.on { card.params.kind.name() } else { "" };
        let mut asks = Vec::new();
        theme::category(ui, "Deform", theme::lit(ACCENT_DEFORM), summary, true, |ui| {
            theme::about(
                ui,
                "Bend, twist or reshape the whole selected object along one of the gizmo's axes. Masked parts stay where they are.",
            );
            const PER_ROW: usize = 4;
            let w = theme::row_width(ui, PER_ROW, 4.0);
            ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
            for (row, icons) in Kind::ALL.chunks(PER_ROW).zip(icons.chunks(PER_ROW)) {
                ui.horizontal(|ui| {
                    for (&kind, &icon) in row.iter().zip(icons) {
                        let picked = card.on && card.params.kind == kind;
                        let r = theme::tile(ui, vec2(w, 48.0), icon, kind.name(), "", picked.then_some(ACCENT()));
                        let text = format!("{}\n{}", kind.name(), kind.tip());
                        if theme::tip(r, tips, &text).clicked() {
                            asks.push(Ask::Pick(if picked { None } else { Some(kind) }));
                        }
                    }
                });
            }
            ui.spacing_mut().item_spacing = vec2(6.0, 6.0);
            if !card.on {
                theme::hint(ui, "Pick one to see its handles on the model.");
                return;
            }
            let p = &mut card.params;
            theme::heading(ui, "Along");
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 3, 6.0);
                for (i, name) in ["X", "Y", "Z"].into_iter().enumerate() {
                    let style = ButtonStyle { active: (p.axis == i).then_some(theme::AXIS[i]), ..Default::default() };
                    let help = format!("Work along the gizmo's {name} axis (its {} arrow).", ["red", "green", "blue"][i]);
                    if theme::tip(theme::button(ui, w, name, style), tips, &help).clicked() {
                        p.axis = i;
                    }
                }
            });
            values(ui, p, tips);
            if p.kind.anchors() {
                theme::heading(ui, "Stays Put");
                ui.horizontal(|ui| {
                    let anchors: &[Anchor] = if p.kind == Kind::BendArc { &[Anchor::Start, Anchor::End] } else { &Anchor::ALL };
                    let w = theme::row_width(ui, anchors.len(), 6.0);
                    for &anchor in anchors {
                        let style = ButtonStyle { active: (p.anchor == anchor).then_some(ACCENT()), ..Default::default() };
                        let help = match anchor {
                            Anchor::Start => "The start of the range stays where it is; the far end moves.",
                            Anchor::Middle => "The middle of the range stays where it is; both ends move alike.",
                            Anchor::End => "The end of the range stays where it is; the start moves.",
                        };
                        if theme::tip(theme::button(ui, w, anchor.name(), style), tips, help).clicked() {
                            p.anchor = anchor;
                        }
                    }
                });
            }
            theme::heading(ui, "Range");
            number(ui, "Start", &mut p.range[0], 0.0..=1.0, 100.0, "%", "Where along the axis the deformation starts. Or drag the first round handle on the axis.", tips);
            number(ui, "End", &mut p.range[1], 0.0..=1.0, 100.0, "%", "Where along the axis it ends. Or drag the second round handle.", tips);
            if p.range[0] > p.range[1] {
                p.range.swap(0, 1);
            }
            if p.kind != Kind::Lattice {
                number(
                    ui,
                    "Softness",
                    &mut p.softness,
                    0.0..=1.0,
                    100.0,
                    "%",
                    "How gently the deformation fades in and out at the ends of the range: 0% starts it sharply.",
                    tips,
                );
            }
            ui.horizontal(|ui| {
                let w = theme::row_width(ui, 2, 6.0);
                ui.add_enabled_ui(pending, |ui| {
                    let style = ButtonStyle { active: Some(ACCENT()), badge: "Enter", ..Default::default() };
                    if theme::tip(theme::button(ui, w, "Apply", style), tips, "Keep the deformation. Undo (Ctrl+Z) takes it back.").clicked() {
                        asks.push(Ask::Apply);
                    }
                    let style = ButtonStyle { badge: "Esc", ..Default::default() };
                    if theme::tip(theme::button(ui, w, "Cancel", style), tips, "Put the model back as it was.").clicked() {
                        asks.push(Ask::Cancel);
                    }
                });
            });
            if theme::tip(
                theme::button(ui, ui.available_width(), "Done", ButtonStyle::default()),
                tips,
                "Keep what is on the model and put the deformers away: the gizmo's arrows come back.",
            )
            .clicked()
            {
                asks.push(Ask::Pick(None));
            }
        });
        for ask in asks {
            self.deform_act(ask);
        }
    }
}

/// The gizmo's flat shapes, in window pixels, onto the painter (in points).
fn paint_shapes(painter: &egui::Painter, shapes: &[Shape], ppp: f32) {
    let to_pt = |p: &Vec2| Pos2::new(p.x / ppp, p.y / ppp);
    let rgba = |c: [u8; 4]| Color32::from_rgba_unmultiplied(c[0], c[1], c[2], c[3]);
    for shape in shapes {
        match shape {
            Shape::Line { points, width, color } => {
                let points: Vec<Pos2> = points.iter().map(to_pt).collect();
                painter.add(egui::Shape::line(points, Line::new(width / ppp, rgba(*color))));
            }
            Shape::Fill { points, color } => {
                let points: Vec<Pos2> = points.iter().map(to_pt).collect();
                painter.add(egui::Shape::convex_polygon(points, rgba(*color), Line::NONE));
            }
        }
    }
}

/// A slider with a number box beside it, both on one value.
#[allow(clippy::too_many_arguments)]
fn number(
    ui: &mut Ui,
    label: &str,
    value: &mut f32,
    range: std::ops::RangeInclusive<f32>,
    scale: f32,
    unit: &str,
    tip: &str,
    tips: bool,
) {
    let (min, max) = (*range.start(), *range.end());
    ui.horizontal(|ui| {
        let box_width = 62.0;
        let slider_width = (ui.available_width() - box_width - 6.0).max(60.0);
        ui.allocate_ui(vec2(slider_width, theme::SLIDER_HEIGHT), |ui| {
            let r = theme::slider(ui, label, value, min..=max, |v| format!("{:.0}{unit}", v * scale));
            theme::tip(r, tips, tip);
        });
        ui.vertical(|ui| {
            ui.add_space(theme::SLIDER_HEIGHT - 24.0);
            let mut shown = *value * scale;
            let drag = egui::DragValue::new(&mut shown)
                .speed(((max - min) * scale / 400.0).max(0.01))
                .range(min * scale..=max * scale)
                .max_decimals(2)
                .suffix(unit);
            let r = ui.add_sized([box_width, 22.0], drag);
            if r.changed() {
                *value = shown / scale;
            }
            theme::tip(r, tips, "Double-click to type a value, or drag left and right.");
        });
    });
}

/// The deformer's own values.
fn values(ui: &mut Ui, p: &mut Params, tips: bool) {
    let axis = ["X", "Y", "Z"][p.axis];
    match p.kind {
        Kind::Bend => {
            number(ui, "Angle", &mut p.angle, -360.0..=360.0, 1.0, "°", "How far the shape bends round over the range.", tips);
            let next = ["Y", "Z", "X"][p.axis];
            let help = format!("Which way it bends, turning round the {axis} axis. 0° bends toward {next}.");
            number(ui, "Direction", &mut p.direction, -180.0..=180.0, 1.0, "°", &help, tips);
        }
        Kind::BendArc => {
            theme::hint(
                ui,
                "Drag the yellow tip handle where the end should go, or type how far it has moved (as a share of the range's length).",
            );
            for (i, name) in ["Tip X", "Tip Y", "Tip Z"].into_iter().enumerate() {
                let help = format!("How far the tip has been moved along the gizmo's {}.", ["X", "Y", "Z"][i]);
                number(ui, name, &mut p.tip[i], -2.0..=2.0, 100.0, "%", &help, tips);
            }
        }
        Kind::Twist => {
            number(ui, "Angle", &mut p.angle, -720.0..=720.0, 1.0, "°", "How far the far end turns about the axis.", tips);
        }
        Kind::Taper => {
            number(ui, "Start Size", &mut p.scales[0], 0.0..=3.0, 100.0, "%", "How big across the shape is at the start of the range.", tips);
            number(ui, "End Size", &mut p.scales[1], 0.0..=3.0, 100.0, "%", "How big across the shape is at the end of the range.", tips);
        }
        Kind::Flatten => {
            number(ui, "Amount", &mut p.flatten, 0.0..=1.0, 100.0, "%", "How much flatter: at 100% the range is pressed flat.", tips);
        }
        Kind::Inflate => {
            number(
                ui,
                "Amount",
                &mut p.inflate,
                -0.25..=0.25,
                100.0,
                "%",
                "How far the surface moves out (or in), as a share of the model's size.",
                tips,
            );
        }
        Kind::Stretch => {
            number(ui, "Length", &mut p.stretch, 0.25..=4.0, 100.0, "%", "How long the range becomes: 200% is twice as long.", tips);
            let r = theme::checkbox(ui, &mut p.keep_volume, "Keep Volume", ACCENT());
            theme::tip(r, tips, "Thin the shape as it gets longer and thicken it as it gets shorter, as clay does.");
        }
        Kind::Lattice => lattice_values(ui, p, tips),
    }
}

/// The cage's size, and the number boxes of the point picked.
fn lattice_values(ui: &mut Ui, p: &mut Params, tips: bool) {
    theme::hint(ui, "Drag a white point of the cage on the model. The clay inside follows it smoothly.");
    let mut count = p.lattice.count;
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        for (i, n) in count.iter_mut().enumerate() {
            let drag = egui::DragValue::new(n).range(lattice::MIN_POINTS..=lattice::MAX_POINTS).prefix(format!("{} ", ["X", "Y", "Z"][i]));
            let r = ui.add_sized([w, 22.0], drag);
            theme::tip(r, tips, "How many points the cage has this way, 2 to 6. Changing it starts the cage afresh.");
        }
    });
    if count != p.lattice.count {
        p.lattice = p.lattice.with_count(count);
    }
    let picked = p.lattice.picked.min(p.lattice.len() - 1);
    let [i, j, k] = p.lattice.ijk(picked);
    ui.horizontal(|ui| {
        let w = theme::row_width(ui, 3, 6.0);
        if theme::button(ui, w, "Previous", ButtonStyle::default()).clicked() {
            p.lattice.picked = (picked + p.lattice.len() - 1) % p.lattice.len();
        }
        ui.add_sized([w, 26.0], egui::Label::new(format!("Point {} {} {}", i + 1, j + 1, k + 1)));
        if theme::button(ui, w, "Next", ButtonStyle::default()).clicked() {
            p.lattice.picked = (picked + 1) % p.lattice.len();
        }
    });
    let picked = p.lattice.picked.min(p.lattice.len() - 1);
    for c in 0..3 {
        let help = format!("How far the picked point has been moved along the gizmo's {}, as a share of the cage.", ["X", "Y", "Z"][c]);
        let mut value = p.lattice.moved[picked][c];
        number(ui, &format!("Point {}", ["X", "Y", "Z"][c]), &mut value, -1.0..=1.0, 100.0, "%", &help, tips);
        p.lattice.moved[picked][c] = value;
    }
    if theme::button(ui, ui.available_width(), "Reset Cage", ButtonStyle::default()).clicked() {
        p.lattice.rest();
    }
}
