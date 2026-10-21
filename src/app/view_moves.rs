//! The view gizmo's pan and zoom handles and its Home button
//! (ui/view_handles.rs), and Numpad 5, which switches between orthographic
//! and perspective as Blender's does. Pan and zoom are the viewport's own
//! moves (a middle drag, a Ctrl + Alt drag), made from the gizmo instead.

use glam::Vec2;

use super::Running;
use super::new_scene::starting_view;
use crate::camera;

impl Running {
    /// A drag on the gizmo's hand, by `by` pixels: the model follows it.
    pub(super) fn pan_from_gizmo(&mut self, by: Vec2) {
        self.doc.camera.cancel_glide();
        let wpp = self.view.world_per_pixel(self.view_depth);
        self.doc.camera.pan(by, wpp);
    }

    /// A drag on the gizmo's magnifier, `dy` pixels down (up is negative):
    /// up comes nearer, as a Ctrl + Alt drag does.
    pub(super) fn zoom_from_gizmo(&mut self, dy: f32) {
        let factor = (1.0 + dy * camera::DRAG_ZOOM_PER_PX).max(0.1);
        let anchor = self.anchor();
        self.doc.camera.zoom(factor, self.view_depth, self.scene_size, anchor);
    }

    /// Home: glides back to the view a scene starts with, the whole scene in
    /// the picture from a little to one side and above, in perspective. The
    /// angle and the framing glide together, as an axis view's turn does.
    pub(super) fn home_view(&mut self) {
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        if bounds.is_empty() {
            return;
        }
        let home = starting_view(bounds, aspect);
        let camera = &mut self.doc.camera;
        camera.ortho = false;
        camera.ortho_from_axis = false;
        camera.snap_to(home.yaw, home.pitch, None);
        camera.glide_to(home.pivot, home.distance);
        self.toast("Home View");
        self.redraw = true;
    }
}
