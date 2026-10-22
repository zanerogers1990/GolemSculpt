//! `--demo-groups`: face groups worked in the real app. `play` runs every
//! command of the card, the click on the model, Stay in Group and everything
//! else that meets a mesh with groups, with a line printed per check (ok /
//! FAIL) and the status line the steps that matter leave. The other words
//! set up a picture for `--screenshot`:
//!
//!   - `parts`: a figure of seven separate parts in one object, after Auto
//!     Groups, its colours showing.
//!   - `figure`: that figure voxel remeshed into one surface, which carries
//!     the groups across; `figure-wire`: with the wireframe on; `figure-flat`:
//!     with flat shading; `figure-lit`: with the lit material, no matcap.
//!   - `isolate`: the figure with one group isolated by the click on the model.
//!   - `stay`: a ball in two groups with strokes of a brush that stays in its
//!     group drawn across the border; `stay-off`: the same strokes without
//!     the tick.
//!   - `quads`: the figure quad remeshed, its quads and colours showing.
//!   - `card`: the figure with the Face Groups card open and a group picked.

use glam::{Quat, Vec2, Vec3};
use serde_json::json;
use winit::keyboard::ModifiersState;

use crate::app::Running;
use crate::doc::{Action, Tool};
use crate::mesh::Mesh;
use crate::mesh::groups::{self, hue_gap};
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::Brush;
use crate::ui::groups::{Armed, Ask};

mod follow;

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

fn centre(mesh: &Mesh, t: usize) -> Vec3 {
    let c = &mesh.indices[t * 3..t * 3 + 3];
    (mesh.positions[c[0] as usize] + mesh.positions[c[1] as usize] + mesh.positions[c[2] as usize]) / 3.0
}

/// Every group of `mesh` with how many faces it has, in order: what is the
/// same for the same groups however the mesh is numbered or moved.
fn census(mesh: &Mesh) -> Vec<(u16, usize)> {
    let mut faces = std::collections::BTreeMap::new();
    for &id in mesh.group_ids() {
        *faces.entry(id).or_insert(0usize) += 1;
    }
    faces.into_iter().collect()
}

/// A figure of seven parts that pass through each other and share no
/// vertex: a body, a neck, a head, two arms and two legs.
fn parts() -> Mesh {
    let (mut positions, mut indices): (Vec<Vec3>, Vec<u32>) = (Vec::new(), Vec::new());
    let mut limb = |from: Vec3, to: Vec3, radius: f32| {
        let part = primitives::build(Primitive::Capsule, 6_000, 0.5);
        let turn = Quat::from_rotation_arc(Vec3::Y, (to - from).try_normalize().unwrap_or(Vec3::Y));
        // The capsule as built is 1 across and 2 tall.
        let scale = Vec3::new(radius / 0.5, from.distance(to) * 0.5 + radius, radius / 0.5);
        let base = positions.len() as u32;
        positions.extend(part.positions.iter().map(|&p| (from + to) * 0.5 + turn * (p * scale)));
        indices.extend(part.indices.iter().map(|&i| i + base));
    };
    let v = Vec3::new;
    limb(v(0.0, 0.05, 0.0), v(0.0, 0.62, 0.0), 0.2);
    limb(v(0.0, 0.7, 0.0), v(0.0, 0.95, 0.0), 0.075);
    limb(v(0.0, 1.0, 0.0), v(0.0, 1.06, 0.0), 0.17);
    for side in [-1.0, 1.0] {
        limb(v(0.2 * side, 0.68, 0.0), v(0.95 * side, 0.68, 0.0), 0.07);
        limb(v(0.11 * side, 0.0, 0.0), v(0.14 * side, -0.85, 0.0), 0.09);
    }
    Mesh::new(positions, indices, None)
}

impl Running {
    fn groups_steps(&self) -> usize {
        self.doc.history.steps().0
    }

    /// Where a point of the active object's own space is on screen.
    fn groups_pixel(&mut self, at: Vec3) -> Vec2 {
        self.sync_view();
        let world = self.doc.scene.active().matrix().transform_point3(at);
        self.view.project(world).unwrap_or(self.view.size * 0.5)
    }

    /// A left press at `at` with these keys held, as the pointer routine
    /// hands it on. Returns whether the groups took it.
    fn groups_press(&mut self, at: Vec2, ctrl_shift: bool) -> bool {
        self.input.pointer = at;
        self.input.mods =
            if ctrl_shift { ModifiersState::CONTROL | ModifiersState::SHIFT } else { ModifiersState::empty() };
        let taken = self.groups_takes_press();
        if taken {
            self.groups_pressed();
        }
        self.input.mods = ModifiersState::empty();
        taken
    }

    /// One scripted stroke between two points of the model, in its own space.
    fn groups_stroke(&mut self, brush: Brush, strength: f32, radius: f32, from: Vec3, to: Vec3) {
        let (a, b) = (self.groups_pixel(from), self.groups_pixel(to));
        let (middle, k) = (self.view.origin + self.view.size * 0.5, self.view.size.y / 900.0);
        self.levels_stroke(brush, strength, radius, (a - middle) / k, (b - middle) / k);
    }

    /// Turns the camera to look at the model straight on from the front, so
    /// that left and right on the model are left and right in the picture
    /// and a stroke's pixels are measured from the model's own middle.
    fn groups_front(&mut self) {
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 0.0);
        self.frame_view();
        self.doc.camera.update(10.0);
        self.sync_view();
    }

    /// Masks the vertices `inside` says yes to and unmasks the rest, as one
    /// undo step, the way the mask commands do.
    fn groups_mask_where(&mut self, inside: impl Fn(Vec3) -> bool + Sync) {
        let delta = self.doc.mesh.remap_mask(|mesh, v, _| if inside(mesh.positions[v as usize]) { 1.0 } else { 0.0 });
        if let Some(delta) = delta {
            self.doc.history.push_stroke(delta);
        }
    }

    /// Puts every face in the group `of` gives its centre, as one undo step.
    fn groups_set(&mut self, of: impl Fn(Vec3) -> u16) {
        let mesh = &mut self.doc.mesh;
        let ids = (0..mesh.triangle_count()).map(|t| of(centre(mesh, t))).collect();
        let before = mesh.set_group_ids(ids);
        self.doc.history.push_groups(groups::GroupsStep::groups(before, None));
    }

    /// The figure of seven parts in place of the scene's object, each part a
    /// group, sculpted a little.
    fn groups_parts(&mut self) {
        self.replace_mesh(parts(), false);
        self.doc.scene.active_mut().name = "Figure".into();
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);
        self.scene_changed();
        self.groups_front();
        self.act(Action::Groups(Ask::Auto));
    }

    /// The figure made one surface, with a belly and a brow sculpted on it.
    fn groups_figure(&mut self) {
        self.groups_parts();
        self.doc.opts.remesh_resolution = 224.0;
        self.act(Action::Remesh);
        self.remesh_wait();
        let v = Vec3::new;
        self.groups_stroke(Brush::Clay, 0.6, 24.0, v(-0.12, 0.45, 0.2), v(0.12, 0.45, 0.2));
        self.groups_stroke(Brush::Clay, 0.5, 20.0, v(-0.08, 0.18, 0.2), v(0.08, 0.2, 0.2));
        self.groups_stroke(Brush::Inflate, 0.5, 12.0, v(-0.03, 1.08, 0.17), v(0.03, 1.08, 0.17));
        self.groups_stroke(Brush::Crease, 0.8, 8.0, v(0.45, 0.72, 0.07), v(0.8, 0.7, 0.07));
    }

    pub(in crate::app) fn groups_demo(&mut self, what: &str) {
        if what == "play" {
            return self.groups_play();
        }
        self.doc.opts.tabs.sculpt = crate::ui::tabs::Sculpt::Mesh;
        match what {
            "parts" => self.groups_parts(),
            "stay" | "stay-off" => self.groups_demo_stay(what == "stay"),
            "quads" => {
                self.groups_figure();
                self.doc.opts.quad_target = 3_000.0;
                self.act(Action::QuadRemesh);
            }
            _ => self.groups_figure(),
        }
        match what {
            "figure-wire" => self.doc.opts.wireframe = true,
            "figure-flat" => self.doc.opts.flat_shading = true,
            "figure-lit" => self.doc.opts.matcap_on = false,
            "isolate" => {
                let chest = self.groups_pixel(Vec3::new(0.0, 0.3, 0.2));
                self.groups_press(chest, true);
            }
            "card" => {
                let arm = self.groups_pixel(Vec3::new(0.6, 0.68, 0.07));
                self.act(Action::Groups(Ask::Arm(Some(Armed::Pick))));
                self.groups_press(arm, false);
            }
            _ => {}
        }
        self.doc.opts.groups.colours = true;
        self.doc.sculptor.settings.brush = Brush::Clay;
        self.groups_front();
        println!("{}", self.ui.status.0);
    }

    /// A ball in two groups, left and right, and three strokes drawn from
    /// the left group across the border, with Stay in Group or without.
    fn groups_demo_stay(&mut self, stay: bool) {
        self.levels_ball(120_000);
        self.groups_front();
        self.groups_mask_where(|p| p.x > 0.0);
        self.act(Action::Groups(Ask::GroupMasked));
        self.act(Action::Groups(Ask::GroupVisible));
        self.groups_mask_where(|p| p.x > 0.0);
        self.act(Action::Groups(Ask::GroupMasked));
        let settings = &mut self.doc.sculptor.settings;
        settings.per_brush[Brush::Clay.index()].stay_in_group = stay;
        settings.per_brush[Brush::Crease.index()].stay_in_group = stay;
        for y in [0.22, 0.0] {
            self.groups_stroke(Brush::Clay, 0.9, 34.0, Vec3::new(-0.36, y, 0.3), Vec3::new(0.3, y - 0.06, 0.38));
        }
        self.groups_stroke(Brush::Crease, 0.9, 16.0, Vec3::new(-0.36, -0.22, 0.3), Vec3::new(0.3, -0.26, 0.35));
    }

    // ---------------------------------------------------------------- the play

    fn groups_play(&mut self) {
        let folder = crate::prefs::scratch("sculpt_groups_demo");
        self.levels_ball(80_000);
        self.groups_play_making();
        self.groups_play_using();
        self.groups_play_stay();
        self.groups_play_following();
        self.groups_play_rebuilders();
        self.groups_play_keepers();
        self.groups_play_files(&folder);
        self.groups_play_bridge();
        self.groups_play_timing();
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The commands that make groups, and their undo.
    fn groups_play_making(&mut self) {
        let status = |app: &Running| app.ui.status.0.clone();
        let steps = self.groups_steps();
        self.act(Action::Groups(Ask::Clear));
        say(
            "Clear Groups on an object without groups says so and is no step",
            status(self).contains("no groups") && self.groups_steps() == steps,
        );
        self.act(Action::Groups(Ask::Isolate));
        say(
            "Isolate with no group picked says how to pick one",
            status(self).contains("No group is picked") && !self.doc.mesh.any_hidden(),
        );
        self.act(Action::Groups(Ask::GroupMasked));
        println!("     {}", status(self));
        say(
            "Group Masked with nothing masked says so and does nothing",
            !self.doc.mesh.has_groups() && self.groups_steps() == steps,
        );

        self.groups_mask_where(|p| p.x > 0.1);
        let (mask, count) = (self.doc.mesh.mask.clone(), self.groups_steps());
        let wanted = {
            let mesh = &self.doc.mesh;
            let masked = |t: &[u32]| t.iter().map(|&v| mesh.mask[v as usize]).sum::<f32>() >= 1.5;
            mesh.indices.chunks_exact(3).filter(|t| masked(t)).count()
        };
        self.act(Action::Groups(Ask::GroupMasked));
        println!("     {}", status(self));
        let mesh = &self.doc.mesh;
        let made = mesh.group_count() == 1 && mesh.group_triangles(1, true).len() == wanted && wanted > 10_000;
        say(
            "Group Masked makes a group of the masked faces, as one undo step",
            made && self.groups_steps() == count + 1,
        );
        say("and clears the mask in the same step", mesh.mask.iter().all(|&m| m == 0.0));
        say("the first group of an object switches Show Group Colours on, and says so", {
            self.doc.opts.groups.colours && status(self).contains("Show Group Colours is on now")
        });
        say("the new group is the one picked", self.group_picked().is_some_and(|p| p.group == 1 && p.faces == wanted));
        let print = self.doc.mesh.groups_print();
        self.act(Action::Undo);
        say(
            "Undo takes the group away and puts the mask back, to the bit",
            !self.doc.mesh.has_groups() && self.doc.mesh.mask == mask,
        );
        self.act(Action::Redo);
        say(
            "Redo makes the same group again",
            self.doc.mesh.groups_print() == print && self.doc.mesh.mask.iter().all(|&m| m == 0.0),
        );
        self.act(Action::Undo);
        self.doc.opts.groups.keep_mask = true;
        self.act(Action::Groups(Ask::GroupMasked));
        println!("     {}", status(self));
        say(
            "with Keep the mask ticked the mask is left as it was",
            self.doc.mesh.groups_print() == print && self.doc.mesh.mask == mask,
        );
        self.doc.opts.groups.keep_mask = false;

        // A second group across the first, and one of what shows.
        self.groups_mask_where(|p| p.y > 0.2);
        self.act(Action::Groups(Ask::GroupMasked));
        let mesh = &self.doc.mesh;
        let second = self.group_picked().map_or(0, |p| p.group);
        say(
            "a second group takes its faces out of the first",
            mesh.group_count() == 2 && mesh.group_triangles(1, true).len() < wanted,
        );
        say("and its colour is not near the colour of the group it touches", second != 0 && hue_gap(1, second) >= 0.1);
        let low: Vec<u32> = (0..mesh.triangle_count() as u32).filter(|&t| centre(mesh, t as usize).z < 0.0).collect();
        let kept: Vec<u16> = low.iter().map(|&t| mesh.group_of(t)).collect();
        self.act(Action::HideFaces { triangles: low.clone(), hide: true });
        let count = self.groups_steps();
        self.act(Action::Groups(Ask::GroupVisible));
        println!("     {}", status(self));
        let mesh = &self.doc.mesh;
        let visible: Vec<u16> = (0..mesh.triangle_count() as u32)
            .filter(|&t| !mesh.is_triangle_hidden(t))
            .map(|t| mesh.group_of(t))
            .collect();
        let one = visible.first().copied().unwrap_or(0);
        say("Group Visible makes one group of what shows, as one undo step", {
            one != 0 && visible.iter().all(|&id| id == one) && self.groups_steps() == count + 1
        });
        say(
            "and leaves what is hidden in the groups it was in",
            low.iter().zip(&kept).all(|(&t, &id)| mesh.group_of(t) == id),
        );
        self.act(Action::Undo);
        self.act(Action::ShowAllFaces);

        let (print, count, had) = (self.doc.mesh.groups_print(), self.groups_steps(), self.doc.mesh.group_count());
        self.act(Action::Groups(Ask::Clear));
        println!("     {}", status(self));
        say(
            "Clear Groups takes them all away, as one undo step",
            !self.doc.mesh.has_groups() && self.groups_steps() == count + 1,
        );
        self.act(Action::Undo);
        say(
            "and Undo brings them back exactly",
            self.doc.mesh.groups_print() == print && self.doc.mesh.group_count() == had,
        );

        // Auto Groups and Groups by Angle, each on a model made for it.
        let ball = self.doc.scene.active_id();
        self.groups_parts();
        println!("     {}", status(self));
        let mesh = &self.doc.mesh;
        let head = (0..mesh.triangle_count())
            .filter(|&t| centre(mesh, t).y > 1.1)
            .map(|t| mesh.group_of(t as u32))
            .collect::<std::collections::BTreeSet<u16>>();
        say(
            "Auto Groups gives each of a figure's seven parts a group of its own",
            mesh.group_count() == 7 && head.len() == 1,
        );
        self.act(Action::Groups(Ask::Auto));
        say("pressed again it makes the same seven", self.doc.mesh.group_count() == 7);
        self.act(Action::New(Primitive::Cube));
        self.doc.opts.groups.angle = 35.0;
        self.act(Action::Groups(Ask::ByAngle));
        println!("     {}", status(self));
        say("Groups by Angle splits a cube into its six sides", self.doc.mesh.group_count() == 6);
        self.doc.opts.groups.angle = 120.0;
        self.act(Action::Groups(Ask::ByAngle));
        say("and at an angle no edge reaches it is one group", self.doc.mesh.group_count() == 1);
        self.act(Action::Undo);
        say("Undo of that is the six sides again", self.doc.mesh.group_count() == 6);
        self.doc.opts.groups.angle = 35.0;
        // Back to the ball as it was.
        let cube = self.doc.scene.active_id();
        self.act(Action::DeleteObject(cube));
        self.act(Action::SelectObject(ball));
    }

    /// Picking a group on the model, and what the card does with the one picked.
    fn groups_play_using(&mut self) {
        let status = |app: &Running| app.ui.status.0.clone();
        // The ball in three groups: right, top left and bottom left.
        self.levels_ball(80_000);
        self.groups_set(|p| {
            if p.x > 0.0 {
                1
            } else if p.y > 0.0 {
                2
            } else {
                3
            }
        });
        let faces = |app: &Running, group: u16| app.doc.mesh.group_triangles(group, true).len();
        let (right, top, bottom) =
            (Vec3::new(0.3, 0.0, 0.4), Vec3::new(-0.25, 0.25, 0.35), Vec3::new(-0.25, -0.25, 0.35));
        let (at_right, at_top, at_bottom) =
            (self.groups_pixel(right), self.groups_pixel(top), self.groups_pixel(bottom));
        say("the three groups are under the three points the play clicks", {
            (self.group_under(at_right), self.group_under(at_top), self.group_under(at_bottom))
                == (Some(1), Some(2), Some(3))
        });

        // The gesture.
        say("a plain click on the model is the brush's, not the groups'", !self.groups_press(at_top, false));
        let count = self.groups_steps();
        say("Ctrl + Shift + click is the groups'", self.groups_press(at_top, true));
        println!("     {}", status(self));
        let mesh = &self.doc.mesh;
        let alone = mesh.visible_triangle_count() == faces(self, 2)
            && (0..mesh.triangle_count() as u32).all(|t| mesh.is_triangle_hidden(t) == (mesh.group_of(t) != 2));
        say("it isolates the group under the pointer, as one undo step", alone && self.groups_steps() == count + 1);
        say(
            "and picks it for the card",
            self.group_picked().is_some_and(|p| p.group == 2 && p.faces == faces(self, 2)),
        );
        self.groups_press(at_top, true);
        say("the same click again shows everything", !self.doc.mesh.any_hidden() && self.groups_steps() == count + 2);
        self.groups_press(at_right, true);
        let corner = Vec2::new(self.view.size.x * 0.5, 4.0);
        self.groups_press(corner, true);
        say(
            "isolated, the same click on the background shows everything",
            !self.doc.mesh.any_hidden() && self.groups_steps() == count + 4,
        );
        self.groups_press(at_bottom, true);
        self.groups_press(at_top, true);
        say(
            "isolated, the same click where a hidden group was is a click on the background too",
            !self.doc.mesh.any_hidden(),
        );
        while self.groups_steps() > count {
            self.act(Action::Undo);
        }
        say("Undo of every one of those is everything showing, as it was", !self.doc.mesh.any_hidden());
        // Isolate over a hide of the sculptor's own: Undo gives back that hide, not everything.
        let strip: Vec<u32> = (0..self.doc.mesh.triangle_count() as u32)
            .filter(|&t| centre(&self.doc.mesh, t as usize).y.abs() < 0.05)
            .collect();
        self.act(Action::HideFaces { triangles: strip.clone(), hide: true });
        self.groups_press(at_right, true);
        self.act(Action::Undo);
        say(
            "Undo of an Isolate gives back what was hidden before it, exactly",
            self.doc.mesh.hidden_triangle_count() == strip.len(),
        );
        self.act(Action::Undo);

        // While a tool that reads Ctrl and Shift is up, the gesture is the tool's.
        self.act(Action::Tool(Tool::Transform));
        say("while another tool is up, Ctrl + Shift + click is that tool's", !self.groups_press(at_top, true));
        self.act(Action::Groups(Ask::Arm(Some(Armed::Pick))));
        let count = self.groups_steps();
        say("but a click the Pick button armed is the groups', in any mode", self.groups_press(at_bottom, false));
        println!("     {}", status(self));
        say("it picks the group and hides nothing, and is no undo step", {
            self.group_picked().is_some_and(|p| p.group == 3)
                && !self.doc.mesh.any_hidden()
                && self.groups_steps() == count
        });
        say(
            "the click after it is the tool's again",
            self.doc.opts.groups.armed.is_none() && !self.groups_press(at_bottom, false),
        );
        self.act(Action::Tool(Tool::Sculpt));

        // The card's buttons on the group picked.
        self.act(Action::Groups(Ask::Isolate));
        say("the Isolate button isolates the group picked", self.doc.mesh.visible_triangle_count() == faces(self, 3));
        self.act(Action::Groups(Ask::Isolate));
        say("pressed again it shows everything", !self.doc.mesh.any_hidden());
        self.act(Action::Groups(Ask::Hide));
        println!("     {}", status(self));
        say("Hide Group hides the group picked", self.doc.mesh.hidden_triangle_count() == faces(self, 3));
        self.act(Action::ShowAllFaces);
        self.act(Action::Groups(Ask::Mask { others: false }));
        let mesh = &self.doc.mesh;
        let on = mesh.group_vertices(3);
        say(
            "Mask Group masks the group picked and nothing else",
            mesh.mask.iter().zip(&on).all(|(&m, &on)| m == if on { 1.0 } else { 0.0 }),
        );
        self.act(Action::Groups(Ask::Mask { others: true }));
        let mesh = &self.doc.mesh;
        say(
            "Mask All But masks everything else",
            mesh.mask.iter().zip(&on).all(|(&m, &on)| m == if on { 0.0 } else { 1.0 }),
        );
        self.act(Action::Undo);
        self.act(Action::Undo);
        say("each is an undo step of its own", self.doc.mesh.mask.iter().all(|&m| m == 0.0));

        // Merge: the group picked into the group clicked.
        let (print, count, both) = (self.doc.mesh.groups_print(), self.groups_steps(), faces(self, 3) + faces(self, 2));
        self.act(Action::Groups(Ask::Arm(Some(Armed::Merge))));
        say(
            "Merge Into arms the next click",
            self.doc.opts.groups.armed == Some(Armed::Merge) && self.groups_press(at_top, false),
        );
        println!("     {}", status(self));
        say("the click merges the group picked into the group clicked, as one undo step", {
            self.doc.mesh.group_count() == 2
                && faces(self, 2) == both
                && faces(self, 3) == 0
                && self.groups_steps() == count + 1
        });
        say(
            "and the group it became part of is the one picked",
            self.group_picked().is_some_and(|p| p.group == 2 && p.faces == both),
        );
        self.act(Action::Undo);
        self.groups_watch();
        say("Undo of the merge is the three groups, exactly", self.doc.mesh.groups_print() == print);
        self.act(Action::Groups(Ask::Arm(Some(Armed::Merge))));
        self.groups_press(at_top, false);
        say(
            "a merge of a group into itself is refused, and is no step",
            status(self).contains("same group") && self.groups_steps() == count,
        );
    }

    /// Stay in Group, for the kinds of brush, under symmetry and with dynamic topology.
    fn groups_play_stay(&mut self) {
        self.levels_ball(80_000);
        self.groups_front();
        self.groups_set(|p| if p.x > 0.0 { 1 } else { 2 });
        let outside: Vec<bool> = self.doc.mesh.group_vertices(2).iter().map(|&on| !on).collect();
        let border = self.doc.mesh.group_vertices(1).iter().zip(&outside).filter(|(one, out)| **one && !**out).count();
        // Each stroke begins in the left group and runs well into the right.
        let strokes: [(Brush, Vec2, Vec2); 6] = [
            (Brush::Clay, Vec2::new(-150.0, -40.0), Vec2::new(150.0, -20.0)),
            (Brush::Standard, Vec2::new(-150.0, 40.0), Vec2::new(150.0, 60.0)),
            (Brush::Smooth, Vec2::new(-150.0, 0.0), Vec2::new(150.0, 0.0)),
            (Brush::Move, Vec2::new(-30.0, 100.0), Vec2::new(120.0, 100.0)),
            (Brush::SnakeHook, Vec2::new(-30.0, -100.0), Vec2::new(120.0, -100.0)),
            (Brush::Nudge, Vec2::new(-40.0, 0.0), Vec2::new(120.0, 20.0)),
        ];
        for (brush, from, to) in strokes {
            let rest = self.doc.mesh.positions.clone();
            let moved = |app: &Running, wanted: bool| {
                let now = &app.doc.mesh.positions;
                (0..now.len()).filter(|&v| outside[v] == wanted && now[v] != rest[v]).count()
            };
            let set = self.call("set_brush", &json!({ "brush": brush.name(), "stay_in_group": false }));
            self.levels_stroke(brush, 0.8, 60.0, from, to);
            let (free_in, free_out) = (moved(self, false), moved(self, true));
            self.act(Action::Undo);
            let set = set.and(self.call("set_brush", &json!({ "stay_in_group": true })));
            let told = set.is_ok_and(|state| state.contains("\"stay_in_group\":true"));
            self.levels_stroke(brush, 0.8, 60.0, from, to);
            let (kept_in, kept_out) = (moved(self, false), moved(self, true));
            println!(
                "     {}: free, the stroke moves {free_in} vertices of its group and {free_out} of the other; kept to its \
                 group, {kept_in} and {kept_out}",
                brush.name()
            );
            say(
                &format!(
                    "{} with Stay in Group moves nothing of the other group, and still works in its own",
                    brush.name()
                ),
                told && free_out > 20 && kept_out == 0 && kept_in > 50 && !self.doc.mesh.stays_in_groups(),
            );
            self.act(Action::Undo);
            say("  one undo step, back to the bit", self.doc.mesh.positions == rest);
            self.doc.sculptor.settings.per_brush[brush.index()].stay_in_group = false;
        }
        println!("     the border has {border} vertices: they are in both groups, and move with either");

        // Under Mirror X: left and right begin on their own groups, and the third group is left alone.
        self.levels_ball(80_000);
        self.groups_front();
        self.groups_set(|p| {
            if p.y < -0.05 {
                3
            } else if p.x > 0.0 {
                1
            } else {
                2
            }
        });
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [true, false, false]);
        self.doc.sculptor.settings.per_brush[Brush::Clay.index()].stay_in_group = true;
        let rest = self.doc.mesh.positions.clone();
        let third: Vec<bool> = {
            let (one, two) = (self.doc.mesh.group_vertices(1), self.doc.mesh.group_vertices(2));
            one.iter().zip(&two).map(|(one, two)| !one && !two).collect()
        };
        // Down the right side, from the upper group well into the lower one.
        self.levels_stroke(Brush::Clay, 0.8, 50.0, Vec2::new(90.0, -120.0), Vec2::new(110.0, 140.0));
        let now = &self.doc.mesh.positions;
        let moved: Vec<usize> = (0..now.len()).filter(|&v| now[v] != rest[v]).collect();
        let (left, right) =
            (moved.iter().filter(|&&v| rest[v].x < -0.05).count(), moved.iter().filter(|&&v| rest[v].x > 0.05).count());
        // (Adding zero makes -0.0 and 0.0 the same bits, for the vertices on the plane.)
        let bits = |p: Vec3| (p + Vec3::ZERO).to_array().map(f32::to_bits);
        let places: std::collections::HashSet<[u32; 3]> = moved.iter().map(|&v| bits(now[v])).collect();
        let mirrored = moved.iter().all(|&v| places.contains(&bits(Vec3::new(-now[v].x, now[v].y, now[v].z))));
        println!("     mirrored: {right} vertices moved on the right and {left} on the left");
        say("under Mirror X each side of a stroke stays in the group its own side began on", {
            right > 100 && left == right && moved.iter().all(|&v| !third[v])
        });
        say("and the two sides are mirror images to the bit", mirrored);
        self.act(Action::Undo);
        self.doc.sculptor.settings.per_brush[Brush::Clay.index()].stay_in_group = false;
        self.levels_stroke(Brush::Clay, 0.8, 50.0, Vec2::new(90.0, -120.0), Vec2::new(110.0, 140.0));
        let free = (0..rest.len()).filter(|&v| third[v] && self.doc.mesh.positions[v] != rest[v]).count();
        say("without the tick the same stroke runs on into the third group", free > 100);
        self.act(Action::Undo);
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);

        // With dynamic topology on: the stroke refines its own group only.
        self.levels_ball(6_000);
        self.groups_front();
        self.groups_set(|p| if p.x > 0.0 { 1 } else { 2 });
        let (ids, triangles) = (self.doc.mesh.group_ids().to_vec(), self.doc.mesh.triangle_count());
        let corners = self.doc.mesh.indices.clone();
        self.doc.sculptor.dyntopo.enabled = true;
        self.doc.sculptor.dyntopo.detail = 0.2;
        self.doc.sculptor.settings.per_brush[Brush::Clay.index()].stay_in_group = true;
        self.levels_stroke(Brush::Clay, 0.9, 45.0, Vec2::new(-120.0, 0.0), Vec2::new(120.0, 20.0));
        let mesh = &self.doc.mesh;
        let untouched = (0..triangles)
            .filter(|&t| ids[t] == 1)
            .all(|t| mesh.indices[t * 3..t * 3 + 3] == corners[t * 3..t * 3 + 3]);
        say("with dynamic topology a stroke kept to its group splits faces of that group only", {
            mesh.triangle_count() > triangles + 100
                && untouched
                && mesh.group_ids()[triangles..].iter().all(|&id| id == 2)
        });
        self.act(Action::Undo);
        say("and Undo takes the new faces and their groups away again", self.doc.mesh.group_ids() == &ids[..]);
        self.doc.sculptor.settings.per_brush[Brush::Clay.index()].stay_in_group = false;
        self.levels_stroke(Brush::Clay, 0.9, 45.0, Vec2::new(-120.0, 0.0), Vec2::new(120.0, 20.0));
        let mesh = &self.doc.mesh;
        let sides = (0..mesh.triangle_count())
            .all(|t| centre(mesh, t).x.abs() < 0.03 || (mesh.group_of(t as u32) == 1) == (centre(mesh, t).x > 0.0));
        say("a free stroke with dynamic topology keeps every new face in the group it was split from", {
            mesh.triangle_count() > triangles + 200
                && mesh.group_ids().len() == mesh.triangle_count()
                && sides
                && mesh.group_count() == 2
        });
        self.act(Action::Undo);
        self.doc.sculptor.dyntopo.enabled = false;
    }
}
