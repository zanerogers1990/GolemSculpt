//! Set Origin to Mesh, on the app's side: the button under Selected Object in
//! the Objects card.
//!
//! What it does to the scene is `scene/origin.rs`: the selected object's
//! centre goes to the middle of its mesh and nothing moves in the world. Here
//! is the rest of what the running app has to keep straight.
//!
//! - **One undo step.** The object list from before is the step, as for a
//!   join: it names the old mesh, which waits in the scene, so Undo gives
//!   back every vertex and every position to the bit.
//! - **Arrays.** An array is laid out from its object's position, so the
//!   Array tool is told (`Tool::origin_moved`) and carries the path the other
//!   way; its copies stay where they are, and it follows the scene's Undo.
//! - **Work that is measured from the centre and is in no file** would be
//!   left behind: unmerged tubes and planned keys (kept in their object's own
//!   space) and an SSphere rig attached to the object (which follows the
//!   object's position). The command is refused while any of those stands,
//!   with a line that says which and what finishes it. Tubes and keys are
//!   asked of their tools as a closing scene would ask (`scaffolds_at_risk`),
//!   which does not say which object they stand on, so they are in the way
//!   whichever object is selected.
//! - **A background job** (a merge, a cut) is working on a mesh as it was,
//!   and would land it in a space that has moved: refused until it is done.
//!
//! The status line says what moved and what did not, and when the object has
//! a Local symmetry it says that the planes went with the centre: that is the
//! point of the command for a piece far from its mirror plane, and news to
//! someone who pressed it on a figure that was symmetric about the centre it
//! had.

use sculpt_toolkit as kit;
use sculpt_toolkit::Loss;

use glam::Vec3;

use super::Running;
use crate::doc::Action;
use crate::history::History;
use crate::mesh::Mesh;
use crate::scene::{OriginMoved, Scene, Space, Symmetry};
use crate::ui::question::listed;

/// Moves the selected object's centre onto its mesh as one undo step.
/// `None`, with no step, when it is there already.
pub(super) fn move_origin(scene: &mut Scene, slot: &mut Mesh, history: &mut History) -> Option<OriginMoved> {
    let before = scene.state();
    let moved = scene.move_origin_to_mesh(slot)?;
    history.push_scene(before);
    Some(moved)
}

/// The status line after the centre of `name` was moved. `symmetry` is the
/// object's own.
fn moved_line(name: &str, moved: &OriginMoved, symmetry: Symmetry) -> String {
    let mut text = format!("Moved the centre of \"{name}\" to the middle of its mesh. Nothing moved in the scene.");
    match moved.objects.len().saturating_sub(1) {
        0 => {}
        1 => text += " Its instance shares the mesh, and stays where it is too.",
        count => text += &format!(" Its {count} instances share the mesh, and stay where they are too."),
    }
    if symmetry.space == Space::Local && !symmetry.is_off() {
        text += &format!(" Its own symmetry ({}) goes through the new centre now.", symmetry.summary());
    }
    text + " Undo (Z) puts the centre back."
}

/// Why the centre cannot be moved just now, if it cannot: `job` is a
/// background job still at work, `scaffolds` the work in no file that stands
/// on the scene's objects (`Loss::SceneReplaced`), and `rig_on` the name of
/// the object, if it is one of those whose centre would move, that an
/// SSphere rig with spheres in it is attached to.
fn refusal(job: Option<&str>, scaffolds: &[String], rig_on: Option<&str>) -> Option<String> {
    if let Some(job) = job {
        return Some(format!("{job} is still working. Wait for it, or press its Cancel, then set the origin."));
    }
    if !scaffolds.is_empty() {
        return Some(format!(
            "The centre was not moved: {} placed from the centres of the objects they stand on, and would be left \
             behind. Finish them first, then set the origin.",
            match scaffolds {
                [one] => format!("{one} is"),
                several => format!("{} are", listed(several)),
            }
        ));
    }
    let on = rig_on?;
    Some(format!(
        "The centre was not moved: the SSphere rig is attached to \"{on}\" and follows its centre. Detach it first \
         (the Attach card of the SSphere tool), then set the origin."
    ))
}

impl Running {
    /// Set Origin to Mesh: the selected object's centre to the middle of its
    /// mesh, the object left where it is. One undo step.
    pub(super) fn set_origin_to_mesh(&mut self) {
        let doc = &self.doc;
        let name = doc.scene.active().name.clone();
        if doc.scene.origin_off_mesh(&doc.mesh).is_none() {
            let text = if doc.mesh.triangle_count() == 0 {
                format!("\"{name}\" has no mesh to put its centre on.")
            } else {
                format!("The centre of \"{name}\" is already in the middle of its mesh.")
            };
            return self.status(text, true);
        }
        // What would be left behind, or land in the wrong place.
        let jobs = doc.tools.jobs.running();
        let scaffolds = doc.tools.scaffolds_at_risk(Loss::SceneReplaced);
        let mesh = doc.scene.active_mesh();
        let rig = doc.spheres.attached_to().filter(|_| !doc.spheres.rig.is_empty());
        let rig_on = rig.and_then(|id| doc.scene.get(id)).filter(|o| o.mesh == mesh).map(|o| o.name.as_str());
        if let Some(why) = refusal(jobs.first().map(|job| job.label.as_str()), &scaffolds, rig_on) {
            return self.status(why, false);
        }

        let doc = &mut self.doc;
        let Some(moved) = move_origin(&mut doc.scene, &mut doc.mesh, &mut doc.history) else { return };
        self.origin_was_moved(&moved);
        let symmetry = self.doc.scene.active_symmetry();
        self.toast("Set Origin to Mesh");
        self.status(moved_line(&name, &moved, symmetry), true);
    }

    /// After the scene moved a centre: the renderer has a new mesh to draw,
    /// and the tools are told, each object in turn (the Array tool carries a
    /// path the other way).
    fn origin_was_moved(&mut self, moved: &OriginMoved) {
        let doc = &mut self.doc;
        doc.mesh_replaced = true;
        for &(id, from, to) in &moved.objects {
            let placed =
                |p: sculpt_symmetry::Placement| kit::Placement { position: p.position, rotation: p.rotation, scale: p.scale };
            doc.tools.origin_moved(id, placed(from), placed(to));
        }
        self.scene_changed();
    }

    // ---- for visual checks ---------------------------------------------------

    /// The box round everything drawn for each of `objects`, in the world:
    /// the object, its live mirror copies and the copies of its array, one
    /// box for each place, in the scene's own order.
    fn drawn_boxes(&self, objects: &[u64]) -> Vec<(Vec3, Vec3)> {
        let (scene, slot) = (&self.doc.scene, &self.doc.mesh);
        let mut boxes = Vec::new();
        for object in objects.iter().filter_map(|&id| scene.get(id)) {
            let Some(mesh) = scene.mesh(object.mesh, slot) else { continue };
            for (_, model) in scene.places_of(object) {
                let points = mesh.positions.iter().map(|&p| model.transform_point3(p));
                boxes.push(points.fold((Vec3::MAX, Vec3::MIN), |(low, high), p| (low.min(p), high.max(p))));
            }
        }
        boxes
    }

    /// `SCULPT_DEMO_SELECT=origin` (with `--demo-scene --demo-select`): Set
    /// Origin to Mesh, through the routine its button runs, on a piece whose
    /// centre is a long way from it, as a piece split off a figure's is.
    ///
    /// The demo scene's cube (which has two instances) is given a mirrored
    /// column of copies by the Array tool, and then its centre is stood 400
    /// of its own lengths away along X without moving anything, and Mirror X
    /// is ticked. Set Origin to Mesh is pressed; then Undo, Redo, and a
    /// Remesh, which runs (brush symmetry never mirrors one). At each point
    /// everything drawn for the cube (the cube, its instances, its mirror
    /// copy, every copy of its array on both sides) is measured against where
    /// it was at the start. Prints the status lines and a line per check
    /// (ok / FAIL); the picture is the scene at the end.
    pub(super) fn origin_demo(&mut self) {
        let named = |app: &Running, name: &str| app.doc.scene.objects.iter().find(|o| o.name == name).map(|o| o.id);
        let (Some(cube), Some(array)) = (named(self, "Cube"), self.doc.tools.info_index("array")) else { return };
        self.act(Action::SelectObject(cube));
        self.sync_view();
        self.tools_activate(array);
        self.tools_call(array, None, |tool, cx| tool.demo(cx, "mirror"));
        self.tools_leave();
        self.doc.camera.update(10.0);
        self.sync_view();
        self.tools_frame();

        let mesh = self.doc.scene.active_mesh();
        let sharing: Vec<u64> = self.doc.scene.objects.iter().filter(|o| o.mesh == mesh).map(|o| o.id).collect();
        let start = self.drawn_boxes(&sharing);
        // To within a five-hundredth of the piece's own size.
        let across = self.doc.mesh.bounds().size().max_element() * self.doc.scene.active().scale.max_element();
        let same = |app: &Running| {
            let now = app.drawn_boxes(&sharing);
            let off = |a: &(Vec3, Vec3), b: &(Vec3, Vec3)| (a.0 - b.0).abs().max((a.1 - b.1).abs()).max_element();
            now.len() == start.len() && now.iter().zip(&start).all(|(a, b)| off(a, b) < across * 2e-3)
        };
        let mut checks = vec![("the cube has two instances, a mirror copy and an array", sharing.len() == 3 && start.len() > 6)];

        // The centre a long way off, nothing moved: a piece that kept its figure's centre.
        let far = Vec3::X * 400.0 * self.doc.mesh.bounds().size().max_element();
        let stood = self.doc.scene.move_origin_by(-far, &mut self.doc.mesh);
        self.origin_was_moved(&stood);
        self.doc.history.clear();
        self.tools_frame();
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, false, false]);
        self.doc.opts.remesh_resolution = crate::doc::Options::default().remesh_resolution;
        checks.push(("standing the centre far off moved nothing", same(self)));
        let placed_far = self.doc.scene.active().placement();

        let steps = self.doc.history.steps().0;
        self.act(Action::SetOriginToMesh);
        println!("{}", self.ui.status.0);
        self.tools_frame();
        let centred = self.doc.mesh.bounds().center().abs().max_element() < across * 1e-3;
        let nothing_left = self.doc.scene.origin_off_mesh(&self.doc.mesh).is_none();
        checks.push(("the centre is in the middle of the mesh", centred && nothing_left));
        checks.push(("nothing drawn for the cube moved", same(self)));
        checks.push(("it is one undo step", self.doc.history.steps().0 == steps + 1));
        checks.push(("the status line is good news", self.ui.status.1 == crate::ui::theme::OK));

        self.act(Action::Undo);
        self.tools_frame();
        let back = self.doc.scene.active().placement() == placed_far && self.doc.history.steps().0 == steps;
        checks.push(("one Undo puts the centre back exactly", back));
        checks.push(("and nothing moved, the array's copies included", same(self)));
        self.act(Action::Redo);
        self.tools_frame();
        let again = self.doc.scene.origin_off_mesh(&self.doc.mesh).is_none();
        checks.push(("Redo moves it again, and still nothing moved", same(self) && again));

        self.act(Action::Remesh);
        println!("{}", self.ui.status.0);
        self.tools_frame();
        let rebuilt = self.ui.status.1 == crate::ui::theme::OK && self.ui.status.0.starts_with("Remeshed to ");
        checks.push(("Remesh runs at the default resolution, Mirror X on or not", rebuilt));
        for (what, ok) in checks {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        self.input.inside = false;
        self.sync_view();
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use glam::{Mat4, Quat, Vec3};

    use super::super::install;
    use super::*;
    use crate::io::{self, Details, SavedObject};
    use crate::mesh::primitives::{self, Primitive};
    use crate::sculpt::Radial;

    /// A cube whose own centre is `off` from its middle.
    fn cube_off(off: Vec3) -> Mesh {
        let cube = primitives::build(Primitive::Cube, 600, 0.5);
        Mesh::new(cube.positions.iter().map(|&p| p + off).collect(), cube.indices.clone(), None)
    }

    /// The corners of the box round a mesh as it is drawn through `model`:
    /// the same for the same shape in the same place, however its vertices
    /// are numbered and wherever its centre is.
    fn world_box(mesh: &Mesh, model: Mat4) -> (Vec3, Vec3) {
        let points = mesh.positions.iter().map(|&p| model.transform_point3(p));
        points.fold((Vec3::MAX, Vec3::MIN), |(low, high), p| (low.min(p), high.max(p)))
    }

    fn same_box(a: (Vec3, Vec3), b: (Vec3, Vec3)) -> bool {
        (a.0 - b.0).abs().max_element() < 1e-4 && (a.1 - b.1).abs().max_element() < 1e-4
    }

    #[test]
    fn it_is_one_undo_step_and_none_when_there_is_nothing_to_move() {
        let mut mesh = cube_off(Vec3::new(2.0, 0.0, -1.0));
        let mut scene = Scene::new("Piece");
        let mut history = History::default();
        let moved = move_origin(&mut scene, &mut mesh, &mut history).expect("the centre is off the mesh");
        assert_eq!(moved.objects.len(), 1);
        assert_eq!(history.steps(), (1, 0));
        // A second press finds the centre where it should be, and records nothing.
        assert!(move_origin(&mut scene, &mut mesh, &mut history).is_none());
        assert_eq!(history.steps(), (1, 0));
        assert!(history.step_scene(true, &mut scene, &mut mesh));
        assert_eq!(scene.origin_off_mesh(&mesh), Some(Vec3::new(2.0, 0.0, -1.0)));
    }

    #[test]
    fn the_line_says_what_stayed_put_and_what_went_with_the_centre() {
        let stand = sculpt_symmetry::Placement::new(Vec3::ZERO, Quat::IDENTITY, Vec3::ONE);
        let one = OriginMoved { shift: Vec3::X, objects: vec![(1, stand, stand)] };
        assert_eq!(
            moved_line("Rivet", &one, Symmetry::default()),
            "Moved the centre of \"Rivet\" to the middle of its mesh. Nothing moved in the scene. Undo (Z) puts \
             the centre back."
        );
        let three = OriginMoved { shift: Vec3::X, objects: vec![(1, stand, stand); 3] };
        let radial = Radial { on: true, count: 6, ..Radial::default() };
        let local = Symmetry { mirror: [true, false, false], radial, ..Symmetry::default() };
        assert_eq!(
            moved_line("Rivet", &three, local),
            "Moved the centre of \"Rivet\" to the middle of its mesh. Nothing moved in the scene. Its 2 instances \
             share the mesh, and stay where they are too. Its own symmetry (X + Radial 6) goes through the new \
             centre now. Undo (Z) puts the centre back."
        );
        // The world's planes stay in the world: nothing to say of them.
        let two = OriginMoved { shift: Vec3::X, objects: vec![(1, stand, stand); 2] };
        let world = Symmetry { space: Space::World, ..local };
        let said = moved_line("Rivet", &two, world);
        assert!(said.contains(" Its instance shares the mesh, and stays where it is too. Undo (Z)"), "{said}");
    }

    #[test]
    fn work_measured_from_the_centre_is_named_before_the_centre_moves() {
        assert_eq!(refusal(None, &[], None), None);
        assert_eq!(
            refusal(Some("Merge Tubes"), &["3 unmerged tubes (Merge or Keep Separate first)".into()], Some("Body")).unwrap(),
            "Merge Tubes is still working. Wait for it, or press its Cancel, then set the origin."
        );
        let tubes = "3 unmerged tubes (Merge or Keep Separate first)".to_owned();
        let keys = "1 planned key (Build Keys first)".to_owned();
        assert_eq!(
            refusal(None, &[keys.clone()], Some("Body")).unwrap(),
            "The centre was not moved: 1 planned key (Build Keys first) is placed from the centres of the objects \
             they stand on, and would be left behind. Finish them first, then set the origin."
        );
        let both = refusal(None, &[tubes, keys], None).unwrap();
        assert!(both.starts_with("The centre was not moved: 3 unmerged tubes (Merge or Keep Separate first) and 1 planned key (Build Keys first) are placed from"), "{both}");
        assert_eq!(
            refusal(None, &[], Some("Body")).unwrap(),
            "The centre was not moved: the SSphere rig is attached to \"Body\" and follows its centre. Detach it \
             first (the Attach card of the SSphere tool), then set the origin."
        );
    }

    fn folder(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join("sculpt_origin_test").join(name);
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        folder
    }

    /// What `Running::save_scene` writes for a scene, without a window.
    fn save(scene: &Scene, slot: &Mesh, path: &std::path::Path) {
        let mut ids: Vec<u64> = Vec::new();
        let objects: Vec<SavedObject> = scene
            .objects
            .iter()
            .map(|o| {
                let mesh = ids.iter().position(|&id| id == o.mesh).unwrap_or_else(|| {
                    ids.push(o.mesh);
                    ids.len() - 1
                });
                let (name, visible) = (o.name.clone(), o.visible);
                SavedObject { name, visible, mesh, position: o.position, rotation: o.rotation, scale: o.scale }
            })
            .collect();
        let meshes: Vec<&Mesh> = ids.iter().map(|&id| scene.mesh(id, slot).unwrap()).collect();
        let mut details = Details::default();
        scene.keep_mirrors(&mut details);
        scene.keep_symmetry(&mut details);
        io::save_scene_with(&meshes, &objects, scene.index_of(scene.active_id()).unwrap(), &details, path).unwrap();
    }

    /// What opening a file does: `install`, then the mirrors and symmetry it kept.
    fn open(path: &std::path::Path) -> (Scene, Mesh) {
        let saved = io::load_scene(path).unwrap();
        let (mut scene, mut slot) = (Scene::new("Placeholder"), cube_off(Vec3::ZERO));
        let details = install(&mut scene, &mut slot, saved);
        scene.restore_mirrors(&details);
        scene.restore_symmetry(&details);
        (scene, slot)
    }

    /// An object with an instance and a live mirror, its centre moved onto
    /// its mesh, saved and opened: every object, and every mirror copy, is
    /// drawn where it was before the centre moved, the instance still shares
    /// the mesh, and each has the symmetry it had.
    #[test]
    fn a_scene_saved_after_the_centre_moved_opens_with_everything_where_it_was() {
        let mut mesh = cube_off(Vec3::new(4.0, -2.0, 1.0));
        let mut scene = Scene::new("Piece");
        let mut history = History::default();
        let first = scene.active_id();
        let object = scene.active_mut();
        (object.position, object.rotation, object.scale) =
            (Vec3::new(0.5, 1.0, -2.0), Quat::from_euler(glam::EulerRot::XYZ, 0.4, 1.3, -0.2), Vec3::new(1.5, 0.75, 1.0));
        let twin = scene.instance_active();
        let object = scene.get_mut(twin).unwrap();
        (object.position, object.rotation) = (Vec3::new(-3.0, 0.0, 2.0), Quat::from_rotation_x(0.9));
        scene.select(first, &mut mesh);
        assert!(scene.set_mirror(first, 1 | 2, Vec3::new(0.2, 0.0, 0.0)));
        let symmetry = Symmetry { mirror: [true, false, true], ..Symmetry::default() };
        scene.set_symmetry(first, symmetry);

        let drawn = |scene: &Scene, slot: &Mesh| -> Vec<(Vec3, Vec3)> {
            let boxes = scene.objects.iter().flat_map(|o| {
                let mesh = scene.mesh(o.mesh, slot).unwrap();
                o.placements().map(move |(_, model)| world_box(mesh, model))
            });
            boxes.collect()
        };
        let before = drawn(&scene, &mesh);
        assert_eq!(before.len(), 5, "the object, its three reflections and its instance");

        move_origin(&mut scene, &mut mesh, &mut history).unwrap();
        let moved = drawn(&scene, &mesh);
        assert!(moved.iter().zip(&before).all(|(a, b)| same_box(*a, *b)), "{moved:?} against {before:?}");

        let path = folder("save").join("recentred.sclp");
        save(&scene, &mesh, &path);
        let (opened, slot) = open(&path);
        assert_eq!(opened.objects.len(), 2);
        let after = drawn(&opened, &slot);
        assert_eq!(after.len(), 5);
        assert!(after.iter().zip(&before).all(|(a, b)| same_box(*a, *b)), "{after:?} against {before:?}");
        // The file holds the mesh about its own middle, and the positions that go with it.
        let (a, b) = (&opened.objects[0], &opened.objects[1]);
        assert_eq!(a.mesh, b.mesh, "the instance still shares the mesh");
        assert!(opened.mesh(a.mesh, &slot).unwrap().bounds().center().abs().max_element() < 1e-5);
        for (opened, saved) in opened.objects.iter().zip(&scene.objects) {
            assert_eq!((opened.position, opened.rotation, opened.scale), (saved.position, saved.rotation, saved.scale));
            assert_eq!(opened.mirror, saved.mirror);
        }
        assert_eq!((opened.symmetry(a.id), opened.symmetry(b.id)), (symmetry, symmetry));
        assert_eq!(opened.origin_off_mesh(&slot), None, "nothing left for the command to do");
    }
}
