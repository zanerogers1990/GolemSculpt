//! The app's side of showing and hiding objects: the eye buttons, the hide
//! keys and Local View. The keys are the Unity app's, which are Blender's: H
//! hides the selection, Shift + H everything else, Alt + H shows everything
//! again, and / toggles Local View, the selection soloed and framed with the
//! view put back on the way out.
//!
//! The keys are not undo steps, here as there; a toast and the status line
//! say what happened instead. Whatever hides the object the brushes were on
//! moves them to one that can still be seen. What they hide is saved with the
//! scene all the same, so a scene they have changed counts as unsaved until
//! it is saved or put back as it was (app/unsaved.rs).

use super::Running;
use crate::camera::Camera;
use crate::mesh::Mesh;
use crate::render::flash;
use crate::scene::Scene;

/// What a press of the Local View key did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum LocalView {
    On,
    Off,
    /// Nothing selected can be seen, so there was nothing to solo.
    Nothing,
}

/// Toggles Local View. Going in hides everything that is not selected and
/// glides the camera onto the selection; coming out shows what that hid and
/// glides back to where the view was, at the angle it has now: the model is
/// wherever you turned it while it was soloed.
pub(super) fn toggle_local_view(scene: &mut Scene, slot: &Mesh, camera: &mut Camera, aspect: f32) -> LocalView {
    if let Some((pivot, distance)) = scene.exit_local_view() {
        camera.glide_to(pivot, distance);
        return LocalView::Off;
    }
    if !scene.enter_local_view((camera.pivot, camera.distance)) {
        return LocalView::Nothing;
    }
    camera.frame(scene.selection_bounds(slot), aspect, true);
    LocalView::On
}

/// "Cube", or "3 objects".
fn named(names: &[String]) -> String {
    match names {
        [one] => format!("\"{one}\""),
        many => format!("{} objects", many.len()),
    }
}

impl Running {
    /// After something was hidden: the scene is not the size it was, and if
    /// the brushes had to move to another object, says which and pulses it.
    /// `was` is the object that was active before.
    fn after_hiding(&mut self, hid: String, was: u64) {
        self.scene_changed();
        let scene = &self.doc.scene;
        let active = scene.active();
        let text = if active.id != was {
            format!("Hid {hid}. Brushes now work on \"{}\".", active.name)
        } else if !active.visible {
            format!("Hid {hid}. Nothing else is shown, so \"{}\" stays selected.", active.name)
        } else {
            format!("Hid {hid}.")
        };
        if active.id != was {
            self.flash.start(active.id, flash::Kind::Select);
        }
        self.status(text + " Alt + H shows everything again.", true);
    }

    /// An eye button: shows or hides one object. One undo step, like the
    /// other buttons of a row.
    pub(super) fn toggle_visible(&mut self, id: u64) {
        let scene = &self.doc.scene;
        let Some(object) = scene.get(id) else { return };
        let (hiding, name, was) = (object.visible, object.name.clone(), scene.active_id());
        self.edit_scene(|scene, slot| scene.toggle_visible(id, slot));
        // Worth a word only when the selection had to move with it.
        if hiding && id == was {
            self.after_hiding(format!("\"{name}\""), was);
        }
    }

    /// H, Shift + H and Alt + H.
    pub(super) fn hide_key(&mut self) {
        let mods = self.input.mods;
        if mods.alt_key() {
            self.show_everything();
        } else if mods.shift_key() {
            let was = self.doc.scene.active_id();
            let hidden = self.doc.scene.hide_unselected();
            if hidden > 0 {
                self.toast("Hide Unselected");
                let what = if hidden == 1 { "the 1 object".to_owned() } else { format!("the {hidden} objects") };
                self.after_hiding(format!("{what} not selected"), was);
            }
        } else {
            self.hide_selection();
        }
    }

    /// Alt + H: every hidden object shows again.
    pub(super) fn show_everything(&mut self) {
        let shown = self.doc.scene.unhide_all();
        let aside = self.doc.scene.in_local_view();
        self.scene_changed();
        self.toast(if shown > 0 { "Unhide All" } else { "Nothing Hidden" });
        let what = if shown == 1 { "1 object".to_owned() } else { format!("{shown} objects") };
        match (shown, aside) {
            (0, false) => self.status("Nothing was hidden.", true),
            (0, true) => self.status("Nothing else was hidden. Press / to leave Local View and see the rest.", true),
            (_, false) => self.status(format!("Showing {what} again."), true),
            (_, true) => self.status(
                format!("Showing {what} again. What Local View set aside comes back when you leave it (/)."),
                true,
            ),
        }
    }

    /// H: hides the selected objects.
    pub(super) fn hide_selection(&mut self) {
        let doc = &mut self.doc;
        let (scene, was) = (&doc.scene, doc.scene.active_id());
        let names: Vec<String> = scene
            .selected()
            .iter()
            .filter_map(|&id| scene.get(id))
            .filter(|o| o.visible)
            .map(|o| o.name.clone())
            .collect();
        if doc.scene.hide_selected(&mut doc.mesh) > 0 {
            self.toast("Hide Selected");
            self.after_hiding(named(&names), was);
        }
    }

    /// The / key: Local View on or off.
    pub(super) fn local_view_key(&mut self) {
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let doc = &mut self.doc;
        match toggle_local_view(&mut doc.scene, &doc.mesh, &mut doc.camera, aspect) {
            LocalView::On => {
                let scene = &doc.scene;
                let names: Vec<String> =
                    scene.selected().iter().filter_map(|&id| scene.get(id)).map(|o| o.name.clone()).collect();
                let ids = scene.selected().to_vec();
                for id in ids {
                    self.flash.start(id, flash::Kind::Select);
                }
                self.toast("Local View");
                self.status(
                    format!("Local View: only {} shown. Press / again to bring the rest back.", match &names[..] {
                        [one] => format!("\"{one}\" is"),
                        many => format!("the {} selected objects are", many.len()),
                    }),
                    true,
                );
            }
            LocalView::Off => {
                self.toast("Local View Off");
                self.status("Local View off: what it hid is back, and so is the view.", true);
            }
            LocalView::Nothing => {
                self.toast("Nothing Selected");
                self.status("Local View needs a selected object that is shown.", false);
            }
        }
        self.scene_changed();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::primitives::{self, Primitive};
    use glam::Vec3;

    fn cube() -> Mesh {
        primitives::build(Primitive::Cube, 600, 0.5)
    }

    /// Runs the camera's glide to its end.
    fn settle(camera: &mut Camera) {
        for _ in 0..200 {
            if !camera.update(1.0 / 60.0) {
                return;
            }
        }
        panic!("the camera never came to rest");
    }

    #[test]
    fn local_view_restores_visibility_and_the_camera() {
        const ASPECT: f32 = 16.0 / 9.0;
        let mut mesh = cube();
        let mut scene = Scene::new("Cube");
        let b = scene.add("Cube", cube(), &mut mesh);
        let c = scene.add("Cube", cube(), &mut mesh);
        scene.get_mut(b).unwrap().position = Vec3::new(4.0, 1.0, 0.0);
        scene.get_mut(c).unwrap().position = Vec3::new(-4.0, 0.0, 2.0);
        let shown = |scene: &Scene| scene.objects.iter().map(|o| o.visible).collect::<Vec<_>>();
        // `c` is hidden beforehand; `b` is the one worked on.
        scene.hide_selected(&mut mesh);
        scene.select(b, &mut mesh);

        let mut camera = Camera::default();
        camera.frame(scene.bounds(&mesh), ASPECT, false);
        (camera.yaw, camera.pitch) = (30.0, 10.0);
        let (pivot, distance) = (camera.pivot, camera.distance);

        // In: everything else goes, and the view glides onto the selection.
        assert_eq!(toggle_local_view(&mut scene, &mesh, &mut camera, ASPECT), LocalView::On);
        assert!(scene.in_local_view());
        assert_eq!(shown(&scene), [false, true, false]);
        assert!(camera.is_animating());
        settle(&mut camera);
        assert!(camera.pivot.distance(Vec3::new(4.0, 1.0, 0.0)) < 1e-4, "pivot {}", camera.pivot);
        assert!(camera.distance < distance, "one cube is framed closer than the pair");

        // The view is turned and zoomed while soloed.
        (camera.yaw, camera.pitch) = (-70.0, 25.0);
        camera.distance *= 0.5;

        // Out: what it hid is back, what was hidden before is not, and the
        // camera glides to where it was, keeping the new angle.
        assert_eq!(toggle_local_view(&mut scene, &mesh, &mut camera, ASPECT), LocalView::Off);
        assert!(!scene.in_local_view());
        assert_eq!(shown(&scene), [true, true, false]);
        settle(&mut camera);
        assert!(camera.pivot.distance(pivot) < 1e-5 && (camera.distance - distance).abs() < 1e-5);
        assert_eq!((camera.yaw, camera.pitch), (-70.0, 25.0));
        assert_eq!((scene.active_id(), scene.selected()), (b, &[b][..]));

        // With nothing shown selected there is nothing to solo.
        scene.select(c, &mut mesh);
        assert_eq!(toggle_local_view(&mut scene, &mesh, &mut camera, ASPECT), LocalView::Nothing);
        assert_eq!(shown(&scene), [true, true, false]);
        assert!(!camera.is_animating());
    }
}
