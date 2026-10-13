//! The second half of `--demo-groups play`: the groups following the model
//! through subdivision levels, layers and everything that rebuilds a mesh,
//! through files and the bridge, and how long the work that grows with the
//! mesh takes.

use glam::{Quat, Vec2, Vec3};
use serde_json::{Value, json};

use super::{census, centre, say};
use crate::app::Running;
use crate::app::levels::{rebuilders, second_object};
use crate::doc::{Action, MaskOp, Tool};
use crate::io;
use crate::mesh::Mesh;
use crate::mesh::groups;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::Brush;
use crate::ui::groups::Ask;

impl Running {
    /// How long the work that grows with the mesh takes, on a ball of a
    /// million triangles and one of four: printed, not checked, since the
    /// machine may be busy with something else.
    pub(super) fn groups_play_timing(&mut self) {
        let ms = |since: std::time::Instant| since.elapsed().as_secs_f64() * 1000.0;
        for triangles in [1_000_000, 4_000_000] {
            self.levels_ball(triangles);
            self.groups_mask_where(|p| p.y > 0.0);
            let t = std::time::Instant::now();
            self.act(Action::Groups(Ask::GroupMasked));
            let masked = ms(t);
            let t = std::time::Instant::now();
            self.act(Action::Groups(Ask::GroupVisible));
            self.act(Action::Undo);
            let visible = ms(t);
            let t = std::time::Instant::now();
            let angle = groups::by_angle(&self.doc.mesh, 35.0).map(|made| made.groups);
            let by_angle = ms(t);
            let t = std::time::Instant::now();
            let parts = groups::parts(&self.doc.mesh).map(|made| made.groups);
            let auto = ms(t);
            let group = self.doc.mesh.group_of(0);
            let t = std::time::Instant::now();
            self.doc.mesh.stay_in_groups(&[group]);
            self.doc.mesh.leave_groups();
            let first = ms(t);
            let t = std::time::Instant::now();
            self.doc.mesh.stay_in_groups(&[group]);
            self.doc.mesh.leave_groups();
            let second = ms(t);
            // The same ball built again, turned so that no new face lies along an old one.
            let turn = Quat::from_rotation_y(0.4) * Quat::from_rotation_x(0.3);
            let other = primitives::build(Primitive::Sphere, triangles, 0.5);
            let mut new = Mesh::new(other.positions.iter().map(|&p| turn * p).collect(), other.indices.clone(), None);
            let t = std::time::Instant::now();
            let carried = groups::carry(&mut new, &[groups::Source::of(&self.doc.mesh)], groups::Fit::Whole);
            let carry = ms(t);
            println!(
                "     {} triangles: Group Masked {masked:.0} ms, Group Visible and its undo {visible:.0} ms, Groups by Angle \
                 {by_angle:.0} ms ({angle:?}), Auto Groups {auto:.0} ms ({parts:?}), Stay in Group at a stroke's start \
                 {first:.1} ms the first time and {second:.3} ms after, groups carried onto a rebuilt mesh {carry:.0} ms \
                 ({carried:?})",
                self.doc.mesh.triangle_count()
            );
        }
        self.levels_ball(20_000);
    }

    /// Subdivision levels and sculpt layers.
    pub(super) fn groups_play_following(&mut self) {
        self.levels_ball(5_000);
        self.groups_set(|p| if p.x + 0.3 * p.y > 0.04 { 1 } else { 2 });
        let ones = self.doc.mesh.group_triangles(1, true).len();
        self.act(Action::Subdivide);
        self.act(Action::Subdivide);
        say(
            "Subdivide gives each new face its parent's group",
            self.doc.mesh.group_triangles(1, true).len() == ones * 16,
        );
        // A border drawn on the finest level, finer than the coarse faces.
        self.groups_set(|p| if p.x + 0.3 * p.y + 0.02 * (p.z * 40.0).sin() > 0.04 { 1 } else { 2 });
        let ordered = |app: &Running| app.doc.mesh.levels.as_ref().map(|levels| levels.ordered_groups(&app.doc.mesh));
        let fine = ordered(self);
        self.act(Action::Level(0));
        say("down two levels the coarse faces are in the groups most of their part of the fine level is", {
            self.doc.mesh.group_count() == 2 && self.doc.mesh.group_ids().len() == self.doc.mesh.triangle_count()
        });
        self.act(Action::Level(2));
        say("and back up the groups are exactly the ones left there, to the face", ordered(self) == fine);
        self.act(Action::LevelBy(-1));
        self.act(Action::LevelBy(1));
        say("one level down and up changes nothing either", ordered(self) == fine);
        self.act(Action::Level(0));
        self.act(Action::Groups(Ask::GroupVisible));
        self.act(Action::Level(2));
        say("groups changed on a coarse level are the groups of the finer ones", self.doc.mesh.group_count() == 1);
        self.act(Action::Undo);
        self.act(Action::Undo);
        self.act(Action::Undo);
        say("Undo back to the finest level is its own groups again", ordered(self) == fine);

        // Layers know nothing of groups, and groups nothing of layers.
        let print = self.doc.mesh.groups_print();
        self.act(Action::Layer(crate::doc::LayerAction::New(None)));
        self.levels_stroke(Brush::Clay, 0.8, 50.0, Vec2::new(-100.0, 0.0), Vec2::new(100.0, 10.0));
        let layers = self.doc.mesh.layers_print();
        say("a layer and a stroke on it leave the groups as they were", self.doc.mesh.groups_print() == print);
        self.groups_mask_where(|p| p.y > 0.1);
        self.act(Action::Groups(Ask::GroupMasked));
        self.act(Action::Mask(MaskOp::Clear));
        say(
            "a new group leaves the layers as they were",
            self.doc.mesh.layers_print() == layers && self.doc.mesh.group_count() == 3,
        );

        // Mask Recesses and Mask Peaks on a model with levels, a layer and groups: a mask, and nothing else.
        let kept = |app: &Running| {
            let mesh = &app.doc.mesh;
            (mesh.positions.clone(), mesh.layers_print(), mesh.groups_print(), mesh.level_count())
        };
        let (was, steps) = (kept(self), self.doc.history.steps().0);
        for shape in [crate::sculpt::cavity::Shape::Recesses, crate::sculpt::cavity::Shape::Peaks] {
            self.act(Action::MaskShape(shape, 0.9));
            println!("     {}", self.ui.status.0);
        }
        say(
            "Mask Recesses and Mask Peaks mask the stroke's hollow and rim there, each as one undo step",
            self.doc.mesh.mask.iter().any(|&m| m > 0.5) && self.doc.history.steps().0 == steps + 2,
        );
        say("and leave the shape, the layers, the groups and the levels as they were", kept(self) == was);
        self.act(Action::Undo);
        self.act(Action::Undo);
        say("Undo of both is no mask again", self.doc.mesh.mask.iter().all(|&m| m == 0.0) && kept(self) == was);
    }

    /// Everything that puts a new mesh in the object's place keeps the
    /// groups, and Undo brings back exactly the ones there were.
    pub(super) fn groups_play_rebuilders(&mut self) {
        self.levels_ball(20_000);
        // Top and bottom, so that a cut at the object's own X plane keeps both.
        self.groups_set(|p| if p.y > 0.0 { 1 } else { 2 });
        let ball = self.doc.scene.active_id();
        let before = self.doc.mesh.groups_print();
        for (name, objects, run) in rebuilders() {
            // The play's own stand-in for Blender swaps the mesh behind the link's back: see below.
            if name.contains("Blender") {
                continue;
            }
            let steps = self.groups_steps();
            if objects == 2 {
                second_object(self);
            }
            let started = std::time::Instant::now();
            run(self);
            let took = started.elapsed().as_secs_f64() * 1000.0;
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            self.groups_watch();
            let mesh = &self.doc.mesh;
            // How far the border between the two groups has strayed from the plane it was on.
            let mut strayed = 0f32;
            let mut longest = 0f32;
            let neighbours = mesh
                .group_vertices(1)
                .iter()
                .zip(&mesh.group_vertices(2))
                .map(|(a, b)| *a && *b)
                .collect::<Vec<bool>>();
            for (v, _) in neighbours.iter().enumerate().filter(|(_, both)| **both) {
                strayed = strayed.max(mesh.positions[v].y.abs());
                longest = mesh
                    .neighbours(v as u32)
                    .iter()
                    .map(|&o| mesh.positions[o as usize].distance(mesh.positions[v]))
                    .fold(longest, f32::max);
            }
            println!(
                "     {name}: {} triangles, {:?}, border {strayed:.4} from where it was (an edge there is {longest:.4}), {took:.0} ms in all",
                mesh.triangle_count(),
                census(mesh),
            );
            let both = mesh.group_triangles(1, true).len() > 50 && mesh.group_triangles(2, true).len() > 50;
            say(
                &format!("{name} keeps the two groups, with the border where it was to within a triangle"),
                both && strayed <= longest.max(1e-4),
            );
            let mut undos = 0;
            while self.groups_steps() > steps && undos < 8 {
                self.act(Action::Undo);
                undos += 1;
            }
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            say(
                &format!("Undo of {name} brings back exactly the groups there were"),
                self.doc.mesh.groups_print() == before,
            );
        }

        // An array made real: every copy keeps the object's groups, as one object or as objects of their own.
        for (what, name) in [("array:made", "one object"), ("array:separate", "separate objects")] {
            let steps = self.groups_steps();
            self.tools_demo(what);
            self.act(Action::Tool(Tool::Sculpt));
            self.groups_watch();
            let scene = &self.doc.scene;
            let all: Vec<(String, Vec<(u16, usize)>)> = scene
                .objects
                .iter()
                .filter_map(|o| Some((o.name.clone(), census(scene.mesh(o.mesh, &self.doc.mesh)?))))
                .collect();
            println!("     an array made {name}: {all:?}");
            // Eight copies of a ball whose two groups have 9,600 faces each, and no face in no group.
            let (objects, each) = if what == "array:made" { (1, 8 * 9_600) } else { (8, 9_600) };
            let whole = all.iter().all(|(_, census)| *census == [(1, each), (2, each)]);
            say(
                &format!("an array made {name} has every copy in the object's two groups"),
                whole && all.len() == objects,
            );
            let mut undos = 0;
            while self.groups_steps() > steps && undos < 8 {
                self.act(Action::Undo);
                undos += 1;
            }
            self.doc.scene.select(ball, &mut self.doc.mesh);
            self.scene_changed();
            say(
                &format!("Undo of the array made {name} is the groups there were"),
                self.doc.mesh.groups_print() == before && self.doc.scene.objects.len() == 1,
            );
        }

        // Back from Blender: the same model keeps its groups, another shape starts without.
        let same = primitives::build(Primitive::Sphere, 9_000, 0.5);
        self.edit_scene(|scene, slot| {
            let mesh = crate::app::groups::taking_over(scene, slot, ball, same);
            scene.replace_mesh_of(ball, mesh, slot)
        });
        self.doc.mesh_replaced = true;
        self.status("\"Sphere\" came back from Blender.", true);
        self.groups_watch();
        say(
            "the same model back from Blender keeps its groups",
            self.doc.mesh.group_count() == 2 && !self.ui.status.0.contains("left behind"),
        );
        self.act(Action::Undo);
        let other = primitives::build(Primitive::Cone, 900, 0.2);
        let far: Vec<Vec3> = other.positions.iter().map(|&p| p + Vec3::new(0.0, 2.0, 0.0)).collect();
        let other = Mesh::new(far, other.indices.clone(), None);
        self.edit_scene(|scene, slot| {
            let mesh = crate::app::groups::taking_over(scene, slot, ball, other);
            scene.replace_mesh_of(ball, mesh, slot)
        });
        self.doc.mesh_replaced = true;
        self.status("\"Sphere\" came back from Blender.", true);
        self.groups_watch();
        println!("     {}", self.ui.status.0);
        say("another shape back from Blender starts without groups, and the status line says so", {
            !self.doc.mesh.has_groups() && self.ui.status.0.contains("Its face groups were left behind")
        });
        self.act(Action::Undo);
        say("Undo of that brings them back", self.doc.mesh.groups_print() == before);

        // A join keeps two objects' groups apart.
        self.act(Action::New(Primitive::Cube));
        self.doc.scene.active_mut().position = Vec3::new(1.5, 0.0, 0.0);
        let cube = self.doc.scene.active_id();
        self.groups_set(|p| if p.x > 0.0 { 1 } else { 2 });
        self.act(Action::SelectObject(ball));
        self.act(Action::SelectAlso(cube));
        self.act(Action::Join { remesh: false });
        let mesh = &self.doc.mesh;
        println!("     joined: {:?}", census(mesh));
        say("a join of two objects that both use groups 1 and 2 has four groups", mesh.group_count() == 4);
        say("the object joined into keeps its numbers, and with them its colours", {
            (0..mesh.triangle_count())
                .filter(|&t| centre(mesh, t).x < 0.8)
                .all(|t| mesh.group_of(t as u32) == if centre(mesh, t).y > 0.0 { 1 } else { 2 })
        });
        self.act(Action::Undo);
        self.act(Action::SelectObject(cube));
        self.act(Action::DeleteObject(cube));
        self.act(Action::SelectObject(ball));
    }

    /// What does not rebuild the triangles keeps every face in its group.
    pub(super) fn groups_play_keepers(&mut self) {
        self.groups_front();
        let ball = self.doc.scene.active_id();
        let (steps, print, was) = (self.groups_steps(), self.doc.mesh.groups_print(), census(&self.doc.mesh));
        let made = self.doc.mesh.positions.clone();
        self.levels_stroke(Brush::Clay, 0.8, 60.0, Vec2::new(-120.0, -60.0), Vec2::new(120.0, 80.0));
        self.act(Action::Mask(MaskOp::Invert));
        self.act(Action::Mask(MaskOp::Clear));
        say(
            "a stroke and the mask commands keep the groups",
            self.doc.mesh.groups_print() == print && self.doc.mesh.positions != made,
        );
        // Reset Shape after a stroke no pointer pressed for: the shape goes back, the groups stay.
        self.act(Action::ResetShape);
        println!("     {}", self.ui.status.0);
        say(
            "Reset Shape puts back the shape from before a scripted stroke, to the bit, and keeps the groups",
            self.doc.mesh.positions == made && self.doc.mesh.groups_print() == print,
        );
        self.act(Action::Undo);
        say("and Undo of it is the stroke again", self.doc.mesh.positions != made);
        let uid = self.doc.mesh.uid;
        // The mesh as it is, to look each face up in after the move.
        let old = groups::built_with(
            self.doc.mesh.positions.clone(),
            self.doc.mesh.indices.clone(),
            None,
            self.doc.mesh.group_ids(),
        );
        let shift = Vec3::new(0.3, 0.1, 0.0);
        self.edit_scene(|scene, slot| {
            scene.move_origin_by(shift, slot);
            true
        });
        self.doc.mesh_replaced = true;
        let mesh = &self.doc.mesh;
        // Every face, found again by where it was, is in the group it was in.
        let same = (0..mesh.triangle_count()).all(|t| {
            let was_at = centre(mesh, t) + shift;
            let found = old.bvh.nearest_triangle(&old.positions, &old.indices, was_at, f32::INFINITY, |_| false);
            found.is_some_and(|(face, apart)| apart < 1e-10 && old.group_of(face) == mesh.group_of(t as u32))
        });
        say(
            "Set Origin to Mesh rebuilds the mesh and keeps every face in its group",
            mesh.uid != uid && census(mesh) == was && same,
        );
        while self.groups_steps() > steps {
            self.act(Action::Undo);
        }
        self.doc.scene.select(ball, &mut self.doc.mesh);
        self.scene_changed();
        say("Undo of all that is the groups as they were", self.doc.mesh.groups_print() == print);

        self.act(Action::InstanceObject);
        say("an instance shares the groups", self.doc.mesh.groups_print() == print);
        self.groups_mask_where(|p| p.x > 0.2);
        self.act(Action::Groups(Ask::GroupMasked));
        self.act(Action::SelectObject(ball));
        say("a group made on the instance is on the original too", self.doc.mesh.group_count() == 3);
        self.act(Action::MakeUnique);
        say("made unique, it has its own, the same", census(&self.doc.mesh).len() == 3);
        self.act(Action::Groups(Ask::Clear));
        let copy = self.doc.scene.objects.iter().map(|o| o.id).find(|&id| id != ball);
        if let Some(copy) = copy {
            self.act(Action::SelectObject(copy));
        }
        say("clearing one's groups leaves the other's", self.doc.mesh.group_count() == 3);
        self.act(Action::CloneObject);
        say("a copy of the object keeps the groups", self.doc.mesh.group_count() == 3);
        while self.groups_steps() > steps {
            self.act(Action::Undo);
        }
        self.doc.scene.select(ball, &mut self.doc.mesh);
        self.scene_changed();
        say("the scene is as it was", self.doc.scene.objects.len() == 1 && self.doc.mesh.groups_print() == print);
    }

    /// Saving, a recovery copy, a damaged file, and OBJ out and back.
    pub(super) fn groups_play_files(&mut self, folder: &std::path::Path) {
        let _ = std::fs::create_dir_all(folder);
        let held = |mesh: &Mesh| mesh.groups_print_by_place();
        self.act(Action::Subdivide);
        for level in [1, 0] {
            self.act(Action::Level(level));
            let now = held(&self.doc.mesh);
            let file = folder.join(format!("groups{level}.sclp"));
            let saved = self.save_scene(&file);
            let opened = io::load_scene(&file);
            let same = opened.as_ref().is_ok_and(|scene| scene.meshes.len() == 1 && held(&scene.meshes[0]) == now);
            say(
                &format!("saved on level {} and opened again, every face is in the group it was in", level + 1),
                saved.is_ok() && same,
            );
            // What a recovery copy is: the same scene through the same writer (`keep_copy`).
            let copy = folder.join(format!("recovery{level}.sclp"));
            let written = self.scene_for_file().and_then(|(meshes, objects, active)| {
                io::save_scene_with(&meshes, &objects, active, &self.details(), &copy)
            });
            let restored = io::load_scene(&copy);
            let same = restored.as_ref().is_ok_and(|scene| held(&scene.meshes[0]) == now);
            say(&format!("a recovery copy made on level {} keeps them", level + 1), written.is_ok() && same);
            let bytes = (std::fs::read(&file).ok(), std::fs::read(&copy).ok());
            let block = |bytes: &Option<Vec<u8>>| {
                let bytes = bytes.as_ref()?;
                let at = bytes.windows(4).position(|tag| tag == b"GRPS")?;
                let length = u64::from_le_bytes(bytes[at + 4..at + 12].try_into().ok()?) as usize;
                Some(bytes[at + 12..at + 12 + length].to_vec())
            };
            say(
                "and its groups are the saved file's, byte for byte",
                block(&bytes.0).is_some() && block(&bytes.0) == block(&bytes.1),
            );
        }
        // A file whose groups cannot be read opens as the model, and says so.
        let file = folder.join("groups0.sclp");
        let damaged = folder.join("damaged.sclp");
        let (shape, census_was) = (self.doc.mesh.bounds(), census(&self.doc.mesh));
        let written = std::fs::read(&file).map(|mut bytes| {
            if let Some(at) = bytes.windows(4).position(|tag| tag == b"GRPS") {
                // The layout number of the block: one this version does not know.
                bytes[at + 16] = 99;
            }
            std::fs::write(&damaged, bytes)
        });
        self.load(damaged, true);
        println!("     {}", self.ui.status.0);
        let now = self.doc.mesh.bounds();
        say("a file whose groups cannot be read opens with the model whole and says so on the status line", {
            written.is_ok()
                && !self.doc.mesh.has_groups()
                && self.doc.mesh.level_count() == 2
                && (now.max - shape.max).abs().max_element() < 1e-6
                && self.ui.status.0.contains("The face groups saved with 1 object could not be read")
        });
        self.load(file, true);
        say("the file itself opens with its groups", census(&self.doc.mesh) == census_was && census_was.len() == 2);

        // OBJ: out as `g` groups, and back.
        let obj = folder.join("groups.obj");
        let exported = io::export_obj_placed(&self.doc.mesh, &[("Sphere", glam::Mat4::IDENTITY)], &obj);
        let text = std::fs::read_to_string(&obj).unwrap_or_default();
        let names: Vec<&str> = text.lines().filter(|line| line.starts_with("g ")).collect();
        say("an OBJ export writes each group as a g group", exported.is_ok() && names == ["g Group_1", "g Group_2"]);
        let back = io::import(&obj).map(io::imported_mesh);
        say("and an OBJ import reads them back as the same groups", back.is_ok_and(|mesh| census(&mesh) == census_was));
    }

    /// The bridge: a command for each thing the card does, and the groups in
    /// what it reports.
    pub(super) fn groups_play_bridge(&mut self) {
        self.levels_ball(40_000);
        let command = |app: &mut Running, args: Value| app.call("command", &args);
        let state = |app: &mut Running| {
            serde_json::from_str::<Value>(&app.call("get_state", &json!({})).unwrap_or_default()).unwrap_or_default()
        };
        let groups_of = |state: &Value| state["objects"][0]["groups"].clone();
        say("get_state reports an object without groups as having none", groups_of(&state(self))["count"] == 0);
        // A stroke drawn through the bridge is a stroke like any other: Reset Shape goes back past it.
        let shape = self.doc.mesh.positions.clone();
        let (from, to) = (self.groups_pixel(Vec3::new(-0.2, 0.1, 0.45)), self.groups_pixel(Vec3::new(0.2, 0.1, 0.45)));
        let (from, to) = (from / self.view.size, to / self.view.size);
        let _ = self.call("set_brush", &json!({ "brush": "Clay", "stay_in_group": false }));
        let drawn = self.call("stroke", &json!({ "points": [[from.x, from.y], [to.x, to.y]] }));
        let moved = self.doc.mesh.positions != shape;
        self.act(Action::ResetShape);
        say(
            "Reset Shape after a stroke drawn through the bridge is the shape from before it, to the bit",
            drawn.is_ok() && moved && self.doc.mesh.positions == shape,
        );
        let made = command(self, json!({ "name": "group_masked" }));
        say(
            "group_masked with nothing masked is an error that says why",
            made.is_err_and(|why| why.contains("masked")),
        );
        self.groups_mask_where(|p| p.x > 0.0);
        let made = command(self, json!({ "name": "group_masked", "keep_mask": true }));
        println!("     {made:?}");
        say(
            "group_masked makes the group and answers with the status line",
            made.is_ok_and(|said| said.contains("new group")) && self.doc.mesh.group_count() == 1,
        );
        say("keep_mask left the mask on", self.doc.mesh.mask.iter().any(|&m| m == 1.0));
        let _ = command(self, json!({ "name": "mask_clear" }));
        let all = command(self, json!({ "name": "group_visible" }));
        self.groups_mask_where(|p| p.x > 0.0);
        let again = command(self, json!({ "name": "group_masked" }));
        say(
            "group_visible and a second group_masked: two groups",
            all.is_ok() && again.is_ok() && self.doc.mesh.group_count() == 2,
        );
        let now = groups_of(&state(self));
        println!("     {now}");
        say("get_state reports the count, the colours and the group picked", {
            now["count"] == 2
                && now["colours_shown"] == true
                && now["picked"]["group"].as_u64().is_some_and(|group| group > 0)
        });
        let right = self.groups_pixel(Vec3::new(0.3, 0.0, 0.4)) / self.view.size;
        let left = self.groups_pixel(Vec3::new(-0.3, 0.0, 0.4)) / self.view.size;
        let picked = self
            .call("pick", &json!({ "x": right.x, "y": right.y }))
            .ok()
            .and_then(|text| serde_json::from_str::<Value>(&text).ok());
        let group = picked.as_ref().and_then(|picked| picked["group"].as_u64()).unwrap_or(0);
        say(
            "pick reports the group under a window position",
            group > 0 && group == self.doc.mesh.group_ids().iter().copied().max().unwrap_or(0) as u64,
        );
        let isolated = command(self, json!({ "name": "isolate_group", "x": right.x, "y": right.y }));
        println!("     {isolated:?}");
        say(
            "isolate_group at a position hides the rest",
            isolated.is_ok()
                && self.doc.mesh.visible_triangle_count() == self.doc.mesh.group_triangles(group as u16, true).len(),
        );
        let shown = command(self, json!({ "name": "show_all_faces" }));
        say("show_all_faces brings it back", shown.is_ok() && !self.doc.mesh.any_hidden());
        let hidden = command(self, json!({ "name": "hide_group", "group": group }));
        say(
            "hide_group by number hides that group",
            hidden.is_ok()
                && self.doc.mesh.hidden_triangle_count() == self.doc.mesh.group_triangles(group as u16, true).len(),
        );
        let _ = command(self, json!({ "name": "show_all_faces" }));
        let masked = command(self, json!({ "name": "mask_group" }));
        say(
            "mask_group with no group named works on the one picked",
            masked.is_ok() && self.doc.mesh.mask.iter().any(|&m| m == 1.0),
        );
        let others = command(self, json!({ "name": "mask_all_but_group", "x": left.x, "y": left.y }));
        say(
            "mask_all_but_group masks the rest",
            others.is_ok() && self.doc.mesh.mask.iter().filter(|&&m| m == 1.0).count() > 1000,
        );
        let _ = command(self, json!({ "name": "mask_clear" }));
        let none = command(self, json!({ "name": "hide_group", "group": 999 }));
        say(
            "a group that does not exist is an error that says so",
            none.is_err_and(|why| why.contains("no group 999")),
        );
        let merged = command(self, json!({ "name": "merge_groups", "x": left.x, "y": left.y, "into_group": group }));
        println!("     {merged:?}");
        say("merge_groups makes one group of the two", merged.is_ok() && self.doc.mesh.group_count() == 1);
        let _ = command(self, json!({ "name": "undo" }));
        let off = command(self, json!({ "name": "show_group_colours", "on": false }));
        say("show_group_colours switches the colours", off.is_ok() && !self.doc.opts.groups.colours);
        let parts = command(self, json!({ "name": "auto_groups" }));
        let sides = command(self, json!({ "name": "groups_by_angle", "angle": 1 }));
        println!("     {sides:?}");
        say(
            "auto_groups and groups_by_angle run and answer",
            parts.is_ok() && sides.is_ok() && self.doc.mesh.group_count() > 100,
        );
        let cleared = command(self, json!({ "name": "clear_groups" }));
        say("clear_groups takes them away", cleared.is_ok() && !self.doc.mesh.has_groups());
        let brush = self.call("set_brush", &json!({ "brush": "Clay", "stay_in_group": true }));
        say("set_brush sets Stay in Group on a brush and reports it", {
            brush.is_ok_and(|state| state.contains("\"stay_in_group\":true"))
                && self.doc.sculptor.settings.per_brush[Brush::Clay.index()].stay_in_group
        });
        self.doc.sculptor.settings.per_brush[Brush::Clay.index()].stay_in_group = false;
    }
}
