//! What counts as unsaved, and what is named as lost, without a window: the
//! scene, its history and the hosted tools as `Running` holds them, worked by
//! the routines the keys and the cards run.
//!
//! For each kind of change that a file keeps and no undo step records, the
//! promise is the same and `Desk::check` holds it to all of it: the scene
//! counts as unsaved, a recovery copy is owed and one copy settles that, and
//! Save makes the scene clean again.

use glam::Vec3;
use sculpt_array::{ArrayKind, ArraySettings, ArrayState};
use sculpt_toolkit::Arrival;

use super::*;
use crate::app::recovery::copy_owed;
use crate::app::visibility::toggle_local_view;
use crate::camera::Camera;
use crate::doc::{Options, Stats, Tool};
use crate::io::Details;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::Space;
use crate::sculpt::{RadialAxis, Sculptor};

fn ball() -> Mesh {
    primitives::build(Primitive::Sphere, 80, 0.5)
}

/// Three balls, the first selected, saved as they stand.
struct Desk {
    scene: Scene,
    mesh: Mesh,
    history: History,
    tools: Tools,
    /// The mark at which the scene is what its file holds (`files::State::clean`).
    clean: Mark,
    /// The mark at which the newest recovery copy was written (`recovery::State::held`).
    held: Option<Mark>,
}

impl Desk {
    fn new() -> Desk {
        let (mut scene, mut mesh) = (Scene::new("Ball 1"), ball());
        let first = scene.active_id();
        scene.add("Ball 2", ball(), &mut mesh);
        scene.add("Ball 3", ball(), &mut mesh);
        scene.select(first, &mut mesh);
        let (history, tools) = (History::default(), Tools::default());
        let clean = Mark::of(&history, &scene, &tools);
        Desk { scene, mesh, history, tools, clean, held: None }
    }

    fn mark(&self) -> Mark {
        Mark::of(&self.history, &self.scene, &self.tools)
    }

    /// As `Running::unsaved`.
    fn unsaved(&self) -> bool {
        self.mark() != self.clean
    }

    /// Whether `Running::recovery_tick` would count the scene as owed a copy.
    fn owed(&self) -> bool {
        copy_owed(true, self.unsaved(), self.held, self.mark())
    }

    /// A recovery copy written (`Running::keep_copy`).
    fn copy(&mut self) {
        self.held = Some(self.mark());
    }

    /// Save (`Running::save_scene`): the scene is what its file holds, and its copies are cleared.
    fn save(&mut self) {
        self.clean = self.mark();
        self.held = None;
    }

    fn id(&self, row: usize) -> u64 {
        self.scene.objects[row].id
    }

    /// The whole promise, for one change that is saved and is not an undo step.
    fn check(&mut self, what: &str, change: impl FnOnce(&mut Desk)) {
        assert!(!self.unsaved() && !self.owed(), "{what}: the scene starts saved");
        let steps = (self.history.edits, self.history.steps());
        change(self);
        assert_eq!((self.history.edits, self.history.steps()), steps, "{what}: it is not an undo step");
        assert!(self.unsaved(), "{what}: the scene counts as unsaved");
        assert!(self.owed(), "{what}: a recovery copy is owed");
        self.copy();
        assert!(!self.owed(), "{what}: the copy holds it, and no second one is written for the same change");
        assert!(self.unsaved(), "{what}: a recovery copy is not a save");
        self.save();
        assert!(!self.unsaved() && !self.owed(), "{what}: Save makes the scene clean");
    }

    /// Gives the object in `row` this array, as the Array tool holds one.
    fn array(&mut self, row: usize, state: &ArrayState) {
        let mut details = Details::default();
        details.set_tool_object(row, "array", &sculpt_array::state_to_saved(state));
        self.tools.restore_from_file(&Tools::rows_of(&self.scene), &details, Arrival::Added);
    }
}

fn line(count: i32) -> ArrayState {
    let mut settings = ArraySettings::new(ArrayKind::Line);
    settings.count = count;
    ArrayState { settings, points: vec![Vec3::ZERO, Vec3::new(2.0, 0.0, 0.0)] }
}

#[test]
fn the_x_key_and_the_symmetry_card_make_the_scene_unsaved() {
    let mut desk = Desk::new();
    let ball = desk.id(0);
    // The X key: Mirror X of the selected object.
    desk.check("Mirror X", |desk| desk.scene.edit_symmetry(ball, |symmetry| symmetry.mirror[0] = true));
    // The card, tick by tick.
    desk.check("Mirror Z", |desk| desk.scene.edit_symmetry(ball, |symmetry| symmetry.mirror[2] = true));
    desk.check("Radial on", |desk| desk.scene.edit_symmetry(ball, |symmetry| symmetry.radial.on = true));
    desk.check("Radial Repeats", |desk| desk.scene.edit_symmetry(ball, |symmetry| symmetry.radial.count = 5));
    desk.check("Radial Axis", |desk| desk.scene.edit_symmetry(ball, |symmetry| symmetry.radial.axis = RadialAxis::Z));
    desk.check("a custom axis", |desk| {
        desk.scene.edit_symmetry(ball, |symmetry| symmetry.radial.set_custom_axis(Vec3::new(1.0, 1.0, 0.0)))
    });
    desk.check("Symmetry space", |desk| desk.scene.edit_symmetry(ball, |symmetry| symmetry.space = Space::World));
    // Another object's symmetry is the scene's too, selected or not.
    let other = desk.id(2);
    desk.check("another object's", |desk| desk.scene.edit_symmetry(other, |symmetry| symmetry.mirror[1] = true));
}

#[test]
fn a_symmetry_put_back_as_it_was_leaves_nothing_unsaved() {
    let mut desk = Desk::new();
    let ball = desk.id(0);
    desk.scene.edit_symmetry(ball, |symmetry| symmetry.mirror[0] = true);
    assert!(desk.unsaved());
    desk.scene.edit_symmetry(ball, |symmetry| symmetry.mirror[0] = false);
    assert!(!desk.unsaved(), "X twice: the scene is what its file holds");
    assert!(!desk.owed());
}

#[test]
fn the_hide_keys_make_the_scene_unsaved() {
    let mut desk = Desk::new();
    desk.check("H", |desk| assert_eq!(desk.scene.hide_selected(&mut desk.mesh), 1));
    desk.check("Alt + H", |desk| assert_eq!(desk.scene.unhide_all(), 1));
    desk.check("Shift + H", |desk| assert_eq!(desk.scene.hide_unselected(), 2));
    // Hidden and shown again before a save: nothing to save.
    desk.scene.unhide_all();
    desk.save();
    desk.scene.hide_unselected();
    assert!(desk.unsaved());
    desk.scene.unhide_all();
    assert!(!desk.unsaved(), "H then Alt + H: the scene is what its file holds");
}

#[test]
fn local_view_is_unsaved_while_it_is_on_since_a_save_would_write_what_it_hides() {
    let mut desk = Desk::new();
    let mut camera = Camera::default();
    desk.check("Local View", |desk| {
        toggle_local_view(&mut desk.scene, &desk.mesh, &mut camera, 1.6);
        assert!(desk.scene.in_local_view());
        assert_eq!(desk.scene.objects.iter().filter(|o| o.visible).count(), 1);
    });
    desk.check("leaving Local View", |desk| {
        toggle_local_view(&mut desk.scene, &desk.mesh, &mut camera, 1.6);
        assert_eq!(desk.scene.objects.iter().filter(|o| o.visible).count(), 3);
    });
    // In and out between two saves: nothing changed.
    toggle_local_view(&mut desk.scene, &desk.mesh, &mut camera, 1.6);
    assert!(desk.unsaved());
    toggle_local_view(&mut desk.scene, &desk.mesh, &mut camera, 1.6);
    assert!(!desk.unsaved());
}

#[test]
fn what_a_live_array_keeps_makes_the_scene_unsaved_when_it_changes() {
    let mut desk = Desk::new();
    desk.check("an array made", |desk| desk.array(1, &line(4)));
    desk.check("Copies", |desk| desk.array(1, &line(7)));
    let mut tapered = line(7);
    tapered.settings.end_scale = 0.4;
    desk.check("End Size", |desk| desk.array(1, &tapered));
    tapered.settings.mid_scale = 1.6;
    desk.check("Middle Size", |desk| desk.array(1, &tapered));
    tapered.settings.follow = !tapered.settings.follow;
    desk.check("a tick", |desk| desk.array(1, &tapered));
    tapered.points[1] = Vec3::new(2.0, 1.0, 0.5);
    desk.check("a path point moved", |desk| desk.array(1, &tapered));
    tapered.points.push(Vec3::new(3.0, 2.0, 0.0));
    desk.check("a path point added", |desk| desk.array(1, &tapered));
    // The same array on another object is another scene.
    desk.check("an array on another object", |desk| desk.array(2, &tapered));
    // And back to exactly what was saved: nothing unsaved.
    let saved = line(4);
    desk.array(1, &saved);
    desk.save();
    desk.array(1, &line(9));
    assert!(desk.unsaved());
    desk.array(1, &saved);
    assert!(!desk.unsaved());
}

/// The rest of what a file keeps of an object is changed by undo steps, which move the mark by
/// themselves; the fingerprint moves with them all the same, so a change that reaches an object by
/// some way that records no step still counts.
#[test]
fn the_fingerprint_reads_every_part_of_an_object_that_a_file_keeps() {
    let desk = Desk::new();
    let start = kept_print(&desk.scene, &desk.tools);
    let changed = |change: &dyn Fn(&mut Desk)| {
        let mut desk = Desk::new();
        change(&mut desk);
        kept_print(&desk.scene, &desk.tools)
    };
    assert_eq!(changed(&|_| {}), start, "the same scene gives the same number");
    let changes: [(&str, &dyn Fn(&mut Desk)); 7] = [
        ("a name", &|desk| desk.scene.objects[1].name = "Head".into()),
        ("the eye", &|desk| desk.scene.objects[1].visible = false),
        ("a move", &|desk| desk.scene.objects[1].position.x += 1.0),
        ("a turn", &|desk| desk.scene.objects[1].rotation = glam::Quat::from_rotation_y(0.5)),
        ("a resize", &|desk| desk.scene.objects[1].scale.y = 2.0),
        ("a live mirror", &|desk| _ = desk.scene.set_mirror(desk.id(1), 1, Vec3::ZERO)),
        ("the order of the list", &|desk| _ = desk.scene.move_to(desk.id(2), 0)),
    ];
    let mut seen = vec![start];
    for (what, change) in changes {
        let print = changed(change);
        assert!(!seen.contains(&print), "{what} does not show in the fingerprint");
        seen.push(print);
    }
    // Which objects share a mesh is in the file too: an instance made unique is the same list of
    // names and places, and another scene.
    let mut desk = Desk::new();
    let instance = desk.scene.instance_active();
    let shared = kept_print(&desk.scene, &desk.tools);
    assert!(!seen.contains(&shared), "an instance added does not show in the fingerprint");
    desk.scene.select(instance, &mut desk.mesh);
    assert!(desk.scene.make_unique(&mut desk.mesh));
    assert_ne!(kept_print(&desk.scene, &desk.tools), shared, "an instance made unique does not show");
}

#[test]
fn the_selection_does_not_count_and_nor_does_what_only_the_tools_draw() {
    let mut desk = Desk::new();
    let (second, third) = (desk.id(1), desk.id(2));
    desk.scene.select(second, &mut desk.mesh);
    desk.scene.select_also(third);
    assert!(!desk.unsaved(), "clicking another object is not a change to save");
    // The places a tool draws an object at are worked out again from what is saved.
    desk.scene.set_tool_places(std::collections::HashMap::from([(second, vec![glam::Mat4::IDENTITY])]));
    assert!(!desk.unsaved());
}

#[test]
fn no_copy_is_owed_where_none_is_kept_or_nothing_is_unsaved() {
    let mut desk = Desk::new();
    let mark = desk.mark();
    assert!(!copy_owed(true, false, None, mark), "a saved scene needs no copy");
    desk.scene.hide_unselected();
    let now = desk.mark();
    assert!(copy_owed(true, true, None, now));
    assert!(copy_owed(true, true, Some(mark), now), "the copy on disk is from before the change");
    assert!(!copy_owed(true, true, Some(now), now), "the copy on disk holds it");
    assert!(!copy_owed(false, true, None, now), "a run that keeps no copies owes none");
}

// ---- scaffolds: what is in no file

fn doc() -> Doc {
    Doc {
        mesh: ball(),
        scene: Scene::new("Ball"),
        mesh_replaced: false,
        history: History::default(),
        sculptor: Sculptor::default(),
        camera: Camera::default(),
        opts: Options::default(),
        path: None,
        stats: Stats::default(),
        tool: Tool::Sculpt,
        spheres: Default::default(),
        tools: Default::default(),
        turntable: Default::default(),
        timelapse: Default::default(),
        still: Default::default(),
    }
}

/// A spine of three spheres on the mirror plane and an arm of two to one side: seven on screen.
fn build_rig(doc: &mut Doc) {
    let rig = &mut doc.spheres.rig;
    let pelvis = rig.add_root(Vec3::new(0.0, 1.0, 0.0), 0.16);
    let chest = rig.add_child(pelvis, Vec3::new(0.0, 1.3, 0.0), 0.14).unwrap();
    rig.add_child(chest, Vec3::new(0.0, 1.6, 0.0), 0.12).unwrap();
    let arm = rig.add_child(chest, Vec3::new(0.3, 1.3, 0.0), 0.08).unwrap();
    rig.add_child(arm, Vec3::new(0.6, 1.2, 0.0), 0.06).unwrap();
}

#[test]
fn a_scene_with_nothing_on_it_closes_and_opens_without_a_question() {
    let doc = doc();
    for loss in [Loss::Closing, Loss::SceneReplaced] {
        assert!(doc.scaffolds_at_risk(loss).is_empty(), "the tools as they start hold nothing");
    }
    assert_eq!(closing_question(false, Vec::new()), None, "nothing unsaved, nothing to lose: the app just closes");
    let file = PathBuf::from("dragon head.sclp");
    assert_eq!(opening_question(file, true, false, Vec::new()), None, "an untouched scene is simply replaced");
}

#[test]
fn an_ssphere_rig_is_named_before_closing_even_when_the_scene_is_saved() {
    let mut doc = doc();
    build_rig(&mut doc);
    let lost = doc.scaffolds_at_risk(Loss::Closing);
    assert_eq!(lost, ["an SSphere rig of 7 spheres (Make Skin first)"]);
    // The scene itself is saved (nothing was done to it), and the app still asks.
    assert!(doc.mark() == Mark::of(&History::default(), &Scene::new("Ball"), &Tools::default()));
    let question = closing_question(false, lost.clone());
    assert_eq!(question, Some(Question::Exit { unsaved: false, lost: lost.clone() }));
    // With unsaved changes as well, the one question says both.
    assert_eq!(closing_question(true, lost.clone()), Some(Question::Exit { unsaved: true, lost }));
    // Unsaved changes alone ask as they always did.
    assert_eq!(closing_question(true, Vec::new()), Some(Question::Exit { unsaved: true, lost: Vec::new() }));
}

#[test]
fn the_rig_is_not_named_before_an_open_since_it_is_still_there_afterwards() {
    let mut doc = doc();
    build_rig(&mut doc);
    assert!(doc.scaffolds_at_risk(Loss::SceneReplaced).is_empty());
}

#[test]
fn once_make_skin_has_used_the_spheres_they_are_not_named_until_the_skin_is_gone_or_they_change() {
    let mut doc = doc();
    build_rig(&mut doc);
    // As `Running::spheres_make_skin`: the skin becomes an object, and the spheres are kept.
    let made = doc.spheres.make_skin().expect("the rig skins");
    let skin = doc.scene.add("SSphere Mesh", Mesh::new(made.skin.positions, made.skin.indices, None), &mut doc.mesh);
    doc.spheres.skin_object = Some(skin);
    assert!(doc.scaffolds_at_risk(Loss::Closing).is_empty(), "the shape is an object of the scene now");
    // Z takes the skin away: the spheres are the only place the shape is.
    doc.scene.remove(skin, &mut doc.mesh);
    assert_eq!(doc.scaffolds_at_risk(Loss::Closing).len(), 1);
}

#[test]
fn a_file_chosen_over_work_that_stands_on_the_scene_asks_even_of_an_untouched_scene() {
    let file = PathBuf::from("dragon head.sclp");
    let tubes = vec!["2 unmerged tubes (Merge or Keep Separate first)".to_owned()];
    // Tubes drawn on the ball the app starts with: nothing is unsaved, and replacing it loses them.
    let asked = opening_question(file.clone(), true, false, tubes.clone());
    assert_eq!(asked, Some(Question::Open { path: file.clone(), unsaved: false, lost: tubes.clone() }));
    // A scene that has been worked on asks as it always did, and names them too.
    let asked = opening_question(file.clone(), false, true, tubes.clone());
    assert_eq!(asked, Some(Question::Open { path: file.clone(), unsaved: true, lost: tubes }));
    let asked = opening_question(file.clone(), false, false, Vec::new());
    assert_eq!(asked, Some(Question::Open { path: file, unsaved: false, lost: Vec::new() }));
}

/// New Scene asks exactly when something would be lost: unsaved changes, or
/// work in no file that stands on the scene's objects. Otherwise the scene
/// it replaces is one Undo away, and it is simply made.
#[test]
fn a_new_scene_asks_only_when_there_is_unsaved_work_or_a_scaffold_on_the_scene() {
    assert_eq!(new_scene_question(false, Vec::new()), None, "saved, and nothing stands on it: no question");
    let unsaved = new_scene_question(true, Vec::new());
    assert_eq!(unsaved, Some(Question::NewScene { unsaved: true, lost: Vec::new() }));
    let tubes = vec!["2 unmerged tubes (Merge or Keep Separate first)".to_owned()];
    // Tubes on a saved scene: nothing to save, and still something to lose.
    let scaffold = new_scene_question(false, tubes.clone());
    assert_eq!(scaffold, Some(Question::NewScene { unsaved: false, lost: tubes.clone() }));
    assert_eq!(new_scene_question(true, tubes.clone()), Some(Question::NewScene { unsaved: true, lost: tubes }));
}

/// What it names is what a scene put in place of this one costs, as for an
/// Open: not the spheres, which stand in the world and are still there.
#[test]
fn a_new_scene_does_not_ask_about_spheres_which_it_leaves_standing() {
    let mut doc = doc();
    build_rig(&mut doc);
    assert_eq!(doc.scaffolds_at_risk(Loss::Closing).len(), 1, "closing would lose the rig");
    let lost = doc.scaffolds_at_risk(Loss::SceneReplaced);
    assert_eq!(new_scene_question(false, lost), None, "a new scene would not, and does not ask");
}

/// What the fingerprint costs, since it is worked out between every batch of events:
/// `cargo test --release timing_of_the_kept_fingerprint -- --ignored --nocapture`.
#[test]
#[ignore = "timing run"]
fn timing_of_the_kept_fingerprint() {
    let (mut scene, mut mesh) = (Scene::new("Ball 1"), ball());
    for i in 1..200 {
        scene.add(&format!("Ball {}", i + 1), ball(), &mut mesh);
    }
    let (history, tools, clean) = (History::default(), Tools::default(), Desk::new().clean);
    let mut desk = Desk { scene, mesh, history, tools, clean, held: None };
    let time = |desk: &Desk| {
        let start = std::time::Instant::now();
        let mut sum = 0u64;
        for _ in 0..1000 {
            sum = sum.wrapping_add(kept_print(&desk.scene, &desk.tools));
        }
        (start.elapsed().as_secs_f64() * 1e6 / 1000.0, sum)
    };
    println!("200 objects, no arrays: {:.1} microseconds", time(&desk).0);
    // Every tenth object with an array along a path of 64 points.
    let mut curve = ArrayState { settings: ArraySettings::new(ArrayKind::Curve), points: Vec::new() };
    curve.points = (0..64).map(|i| Vec3::new(i as f32 * 0.1, (i as f32 * 0.3).sin(), 0.0)).collect();
    for row in (0..200).step_by(10) {
        desk.array(row, &curve);
    }
    println!("200 objects, 20 arrays of 64 points: {:.1} microseconds", time(&desk).0);
}
