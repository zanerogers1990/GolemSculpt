//! Apply Scale, on the app's side: the button under the Scale fields of the
//! Objects card, what Remesh does first, and the line that says why a brush
//! is landing stretched.
//!
//! What it does to the scene is `scene/apply_scale.rs`: the selected object's
//! uneven Scale goes into its mesh and nothing changes on screen. It is one
//! undo step, the object list from before, as for Set Origin to Mesh
//! (app/origin.rs). It is refused for what would be left behind:
//!
//! - **Work measured in the object's space and in no file**: unmerged tubes
//!   and planned keys, and an SSphere rig attached to the object.
//! - **Subdivision levels and sculpt layers**, whose coarser shapes and frames
//!   are kept in the mesh's own axes. Remesh merges them anyway, so it goes on
//!   without; deleting the levels or baking the layers lets the button through.
//! - **A background job**, working on a mesh as it was.

use std::collections::HashSet;

use sculpt_toolkit::Loss;

use glam::Vec3;

use super::Running;
use crate::doc::Action;
use crate::ui::question::listed;

/// How uneven a Scale has to be for it to be worth a word when a stroke begins,
/// and for Remesh to even it first: below this a brush looks round.
const NOTICEABLE: f32 = 1.05;

/// What the app keeps for Apply Scale.
#[derive(Default)]
pub(super) struct State {
    /// The objects a stroke has already said this about, so that it is said
    /// once for each and not for every stroke after.
    warned: HashSet<u64>,
}

impl Running {
    /// Why the selected object's Scale cannot go into its mesh just now, if it
    /// cannot (see the top of this file).
    fn apply_scale_refusal(&self) -> Option<String> {
        let doc = &self.doc;
        if let Some(job) = doc.tools.jobs.running().first() {
            return Some(format!("{} is still working. Wait for it, or press its Cancel, then apply the Scale.", job.label));
        }
        let scaffolds = doc.tools.scaffolds_at_risk(Loss::SceneReplaced);
        if !scaffolds.is_empty() {
            return Some(format!(
                "The Scale was not applied: {} placed in the space of the objects they stand on, and would be left \
                 behind. Finish them first, then apply it.",
                match scaffolds.as_slice() {
                    [one] => format!("{one} is"),
                    several => format!("{} are", listed(several)),
                }
            ));
        }
        let mesh = doc.scene.active_mesh();
        let rig = doc.spheres.attached_to().filter(|_| !doc.spheres.rig.is_empty());
        if let Some(on) = rig.and_then(|id| doc.scene.get(id)).filter(|o| o.mesh == mesh) {
            return Some(format!(
                "The Scale was not applied: the SSphere rig is attached to \"{}\". Detach it first (the Attach card of \
                 the SSphere tool), then apply it.",
                on.name
            ));
        }
        if doc.mesh.levels.is_some() {
            return Some(
                "The Scale was not applied: the coarser subdivision levels keep the axes they were made in. Delete \
                 them first (the Levels card), then apply it."
                    .into(),
            );
        }
        if doc.mesh.layers.is_some() {
            return Some("The Scale was not applied: sculpt layers keep the axes they were made in. Bake them first (the Layers card), then apply it.".into());
        }
        None
    }

    /// Puts the Scale into the mesh as one undo step. False, with no step, when
    /// it is even already.
    fn apply_scale_step(&mut self) -> Option<usize> {
        let doc = &mut self.doc;
        let before = doc.scene.state();
        let instances = doc.scene.apply_scale(&mut doc.mesh)?;
        doc.history.name_next(Some("Apply Scale".to_owned()));
        doc.history.push_scene(before);
        doc.history.name_next(None);
        doc.mesh_replaced = true;
        self.scene_changed();
        Some(instances)
    }

    /// Apply Scale: the button.
    pub(super) fn apply_scale(&mut self) {
        let name = self.doc.scene.active().name.clone();
        if !self.doc.scene.scale_is_uneven() {
            return self.status(format!("The Scale of \"{name}\" is the same every way: there is nothing to apply."), true);
        }
        if let Some(why) = self.apply_scale_refusal() {
            return self.status(why, false);
        }
        let Some(instances) = self.apply_scale_step() else { return };
        self.toast("Apply Scale");
        let mut text = format!("Put the Scale of \"{name}\" into its mesh. Nothing changed on screen, and brushes land round on it now.");
        match instances {
            0 => {}
            1 => text += " Its instance shares the mesh, and looks as it did.",
            count => text += &format!(" Its {count} instances share the mesh, and look as they did."),
        }
        self.status(text + " Press Remesh to even out the triangles too. Undo (Ctrl+Z) puts it back.", true);
    }

    /// As a Remesh starts: evens a noticeably uneven Scale first, if nothing
    /// stands in the way, so the voxels are cubes in the world. Whether it did.
    /// Refused, the Remesh goes on as it would have, stretched triangles and all.
    pub(super) fn even_scale_for_remesh(&mut self) -> bool {
        let scene = &self.doc.scene;
        if !scene.scale_is_uneven() || scene.stretch() < NOTICEABLE || self.apply_scale_refusal().is_some() {
            return false;
        }
        self.apply_scale_step().is_some()
    }

    /// As a stroke begins: says once for each object that it is stretched
    /// enough for a brush to land visibly out of round, since nothing else
    /// shows why.
    pub(super) fn warn_if_stretched(&mut self) {
        let scene = &self.doc.scene;
        let stretch = scene.stretch();
        if !scene.scale_is_uneven() || stretch < NOTICEABLE {
            return;
        }
        let (id, name) = (scene.active_id(), scene.active().name.clone());
        if self.apply_scale.warned.insert(id) {
            self.status(
                format!(
                    "\"{name}\" is stretched by its Scale, so brushes land {stretch:.1} times longer one way than the \
                     ring shows. Press Remesh, or Apply Scale in the Objects card, to even it out: the shape stays."
                ),
                false,
            );
        }
    }

    /// `SCULPT_DEMO_SELECT=apply-scale` (with `--demo-scene --demo-select`):
    /// Apply Scale and a Remesh, through the routines their buttons run, on
    /// the demo scene's cube stretched three times as tall, with its two
    /// instances, a mirror copy and an array. Prints the status lines and a
    /// line per check (ok / FAIL); the picture is the scene at the end.
    pub(super) fn apply_scale_demo(&mut self) {
        let named = |app: &Running, name: &str| app.doc.scene.objects.iter().find(|o| o.name == name).map(|o| o.id);
        let (Some(cube), Some(array)) = (named(self, "Cube"), self.doc.tools.info_index("array")) else { return };
        self.act(Action::SelectObject(cube));
        self.sync_view();
        self.tools_activate(array);
        self.tools_call(array, None, |tool, cx| tool.demo(cx, "mirror"));
        self.tools_leave();
        self.doc.scene.active_mut().scale = Vec3::new(1.0, 3.0, 1.0);
        self.doc.history.clear();
        self.doc.camera.update(10.0);
        self.sync_view();
        self.tools_frame();

        let mesh = self.doc.scene.active_mesh();
        let sharing: Vec<u64> = self.doc.scene.objects.iter().filter(|o| o.mesh == mesh).map(|o| o.id).collect();
        let start = self.drawn_boxes(&sharing);
        let across = self.doc.mesh.bounds().size().max_element() * 3.0;
        let same = |app: &Running| {
            let now = app.drawn_boxes(&sharing);
            let off = |a: &(Vec3, Vec3), b: &(Vec3, Vec3)| (a.0 - b.0).abs().max((a.1 - b.1).abs()).max_element();
            now.len() == start.len() && now.iter().zip(&start).all(|(a, b)| off(a, b) < across * 2e-3)
        };
        let scale_of = |app: &Running| app.doc.scene.active().scale;
        let mut checks = vec![("the cube has instances, a mirror copy and an array", sharing.len() > 1 && start.len() > 6)];

        self.warn_if_stretched();
        println!("{}", self.ui.status.0);
        checks.push(("the first stroke says the object is stretched", self.ui.status.0.contains("Apply Scale")));
        self.ui.status.0.clear();
        self.warn_if_stretched();
        checks.push(("and the next does not", self.ui.status.0.is_empty()));

        let steps = self.doc.history.steps().0;
        self.act(Action::ApplyScale);
        println!("{}", self.ui.status.0);
        self.tools_frame();
        checks.push(("the Scale comes out 1", scale_of(self) == Vec3::ONE));
        checks.push(("nothing drawn for the cube moved", same(self)));
        checks.push(("it is one undo step", self.doc.history.steps().0 == steps + 1));
        checks.push(("the status line is good news", self.ui.status.1 == crate::ui::theme::OK));

        self.act(Action::Undo);
        self.tools_frame();
        checks.push(("one Undo puts the stretch back", scale_of(self) == Vec3::new(1.0, 3.0, 1.0) && same(self)));
        self.act(Action::Redo);
        self.tools_frame();
        checks.push(("Redo applies it again", scale_of(self) == Vec3::ONE && same(self)));
        self.act(Action::Undo);
        self.tools_frame();

        self.act(Action::Remesh);
        self.remesh_wait();
        println!("{}", self.ui.status.0);
        self.tools_frame();
        checks.push(("Remesh evens the Scale first, and says so", scale_of(self) == Vec3::ONE && self.ui.status.0.contains("uneven Scale")));
        checks.push(("and a Remesh of even triangles came of it", self.ui.status.0.starts_with("Remeshed to ")));
        self.act(Action::Undo);
        self.act(Action::Undo);
        self.tools_frame();
        checks.push(("two Undos go back past the Remesh and the Scale", scale_of(self) == Vec3::new(1.0, 3.0, 1.0) && same(self)));
        for (what, ok) in checks {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.input.inside = false;
        self.sync_view();
    }
}
