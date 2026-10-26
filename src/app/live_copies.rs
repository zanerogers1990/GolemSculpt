//! Scripted runs for the copies a tool keeps of an object (an array's), for
//! looking at and for checking the wiring with no one at the tablet. The copies
//! are the tool's (`Draw::live_copies`); what is the app's is that they are
//! places the object is shown, so the pointer meets them (`scene/places.rs`).
//!
//! - `--demo-tool hide:array` (also `mask`, `trim`, `split`): the Array tool
//!   lays a column of copies out first, then that tool draws its shape over one
//!   of them, and every copy shows the result.
//! - `--demo-tool array:stroke`: the column, the tool put away, and brush
//!   strokes begun on its copies by way of the window's pointer routines. The
//!   first is checked step by step and prints a line per check.
//! - `--demo-tool array:arrow`: the gizmo's arrow on the end of the path
//!   dragged with the pointer, then Z, checked the same way.

use glam::{Vec2, Vec3};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

use super::{Drag, Running};
use crate::scene::TOOL_COPY;
use crate::sculpt::Brush;

impl Running {
    /// The `--demo-tool` specs that involve an array (see the top of this
    /// file). True when the spec was one of them and has been dealt with.
    pub(super) fn demo_with_array(&mut self, index: usize, id: &str, what: &str) -> bool {
        match (id, what) {
            ("array", "stroke") => {
                if self.demo_array_set_up() {
                    self.tools_leave();
                    self.array_strokes();
                }
            }
            ("array", "arrow") => {
                self.tools_call(index, None, |tool, cx| tool.demo(cx, "point"));
                self.doc.camera.update(10.0);
                self.sync_view();
                self.tools_frame();
                self.arrow_drag();
            }
            (_, "array") if id != "array" => {
                if self.demo_array_set_up() {
                    self.tools_call(index, None, |tool, cx| tool.demo(cx, "array"));
                }
                self.sync_view();
            }
            _ => return false,
        }
        true
    }

    /// Drags the gizmo's red arrow on the end point of an array's path with the
    /// pointer, through the routines the window feeds, and checks what the app
    /// and the tool did with it: the end of the path went along X and nowhere
    /// else, the copies followed, and one Z put it all back.
    fn arrow_drag(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        let id = self.doc.scene.active_id();
        let last = |app: &Running| app.doc.scene.tool_places(id).last().map(|m| m.transform_point3(Vec3::ZERO));
        let Some(end) = last(self) else { return say("the array keeps copies", false) };
        let (Some(dot), Some(along)) = (self.view.project(end), self.view.project(end + Vec3::X * 0.05)) else {
            return say("the end of the path is in view", false);
        };
        // The arrow is a shaft from the dot toward the screen position of +X; a press well along it is on it.
        let toward = (along - dot).normalize_or(Vec2::X);
        let (steps, before) = (self.doc.history.steps(), end);
        say("the arrows are offered on the end point", self.doc.tools.gizmo(self.doc.tool).is_some());
        let press = dot + toward * 55.0;
        self.pointer_moved(press, None);
        self.sync_view();
        self.pointer_pressed(MouseButton::Left, None);
        say("a press on the arrow is the gizmo's", self.input.drag == Drag::Gizmo);
        for i in 1..=12 {
            self.pointer_moved(press + toward * (i as f32 * 8.0), None);
        }
        self.pointer_released(MouseButton::Left);
        self.tools_frame();
        let Some(after) = last(self) else { return say("the array still keeps copies", false) };
        let moved = after - before;
        println!("     the end of the path moved by {moved}");
        say("the end of the path went along X and nowhere else", moved.x > 0.2 && moved.y.abs() < 1e-4 && moved.z.abs() < 1e-4);
        say("the scene's history is untouched: the array's edit is the tool's", self.doc.history.steps() == steps);
        self.act(crate::doc::Action::Undo);
        self.tools_frame();
        let back = last(self).is_some_and(|p| p.distance(before) < 1e-6);
        say("one Ctrl+Z puts the copies back exactly", back);
        self.input.inside = false;
        self.sync_view();
    }

    /// The Array tool lays out a column of copies of the selected object, which
    /// stay on screen and are said to be places of it, and the camera is on all
    /// of them.
    fn demo_array_set_up(&mut self) -> bool {
        let Some(array) = self.doc.tools.info_index("array") else {
            eprintln!("demo: there is no Array tool");
            return false;
        };
        self.tools_call(array, None, |tool, cx| tool.demo(cx, "column"));
        // One frame, which is when the tool says where its copies are.
        self.tools_frame();
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        // Room round it: the panels cover the sides of the picture.
        self.doc.camera.distance *= 1.25;
        self.sync_view();
        true
    }

    /// Where `local` (a point of the object's mesh) is on screen as place
    /// `code` of the active object shows it.
    fn place_pixel(&self, code: u16, local: Vec3) -> Option<Vec2> {
        let scene = &self.doc.scene;
        self.view.project(scene.place_matrix(scene.active(), code).transform_point3(local))
    }

    /// Where the middle of place `code` is on screen, and about how far its
    /// edge is from there, in pixels: what strokes are laid out in units of.
    fn place_on_screen(&self, code: u16) -> Option<(Vec2, f32)> {
        let (centre, up) = (self.place_pixel(code, Vec3::ZERO)?, self.place_pixel(code, Vec3::Y * 0.5)?);
        Some((centre, centre.distance(up)))
    }

    /// Strokes begun on the copies of the array, as the pointer would make
    /// them. The first is checked: it has to be the copy that is under the
    /// pointer that the stroke goes through, and it has to be one undo step.
    fn array_strokes(&mut self) {
        fn say(what: &str, ok: bool) {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.input.inside = true;
        self.input.mods = ModifiersState::empty();
        let id = self.doc.scene.active_id();
        let copies = self.doc.scene.tool_places(id).len();
        say(&format!("the array keeps {copies} copies as places of the object"), copies == 4);
        self.demo_symmetry(Some([false; 3]));

        let code = TOOL_COPY + 2;
        let Some((centre, r)) = self.place_on_screen(code) else { return say("the copy is in view", false) };
        let copy_matrix = self.doc.scene.place_matrix(self.doc.scene.active(), code);
        let (rest, steps) = (self.doc.mesh.positions.clone(), self.doc.history.steps().0);

        self.doc.sculptor.settings.brush = Brush::Inflate;
        self.doc.sculptor.settings.radius_px = r * 0.35;
        self.pointer_moved(centre, None);
        self.sync_view();
        say("the copy is under the pointer where the geometry says", self.pick_placed(centre) == Some((id, code)));
        say("the pointer over the copy counts as over the mesh", self.hover_on_active());
        self.pointer_pressed(MouseButton::Left, None);
        say("a press on the copy begins a stroke", self.input.drag == Drag::Sculpt);
        say("which goes through the copy's matrix", self.doc.scene.brush_matrix() == copy_matrix);
        for i in 1..=15 {
            self.pointer_moved(centre + Vec2::new(i as f32 * r * 0.04, 0.0), None);
        }
        self.pointer_released(MouseButton::Left);
        let moved: Vec<Vec3> =
            self.doc.mesh.positions.iter().zip(&rest).filter(|(a, b)| a != b).map(|(_, &b)| b).collect();
        let middle = moved.iter().copied().sum::<Vec3>() / moved.len().max(1) as f32;
        let (origin, dir) = self.view.ray(centre);
        let seen = self.doc.scene.pick(origin, dir, &self.doc.mesh);
        let want = seen.map(|p| copy_matrix.inverse().transform_point3(p.pos));
        println!("     {} vertices moved, around {middle}", moved.len());
        let there = !moved.is_empty() && want.is_some_and(|w| w.distance(middle) < 0.25);
        say("it moves the vertices the copy shows under the pointer", there);
        say("as one undo step", self.doc.history.steps().0 == steps + 1);
        say("the orbit pivot is on the copy", {
            let pivot = self.anchor();
            let last = self.doc.sculptor.last_hit.unwrap_or(Vec3::ZERO);
            pivot.is_some_and(|a| a.distance(copy_matrix.transform_point3(last)) < 1e-4)
        });

        // A few more, on other copies: the one mesh shows every stroke on every copy.
        for (copy, brush) in [(0, Brush::Clay), (3, Brush::Crease)] {
            let Some((c, r)) = self.place_on_screen(TOOL_COPY + copy) else { continue };
            self.demo_drag_stroke(brush, 0.8, r * 0.3, c + Vec2::new(-0.6, -0.1) * r, c + Vec2::new(0.5, 0.05) * r);
        }
        self.doc.sculptor.settings.brush = Brush::Clay;
        self.doc.sculptor.settings.demo_setup(true);
        // The brush ring is where the last stroke ended; leave it out of the picture.
        self.input.inside = false;
        self.sync_view();
    }
}
