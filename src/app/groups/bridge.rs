//! Face groups through the Claude bridge: a command for each thing the card
//! does, the groups in what `get_state` reports, and the group under a
//! window position in what `pick` reports.
//!
//! A command that works on one group takes it either as a window position
//! (`x` and `y`, each 0 to 1, as `pick` takes them: the group under that
//! point of the active object) or as its number (`group`, from `pick`), and
//! with neither works on the group picked last, as the card's buttons do.
//! Every command runs through the same `Action` the card pushes, so it is
//! the same undo step and says the same thing on the status line, which is
//! what comes back.

use glam::Vec2;
use serde_json::{Value, json};

use crate::app::Running;
use crate::doc::Action;
use crate::mesh::Mesh;
use crate::scene::{Object, Scene};
use crate::ui::groups::{Armed, Ask};

/// What `get_state` says of an object's face groups: how many it has,
/// whether their colours are showing and, for the active object, the group
/// picked.
pub(in crate::app) fn state_of(
    scene: &Scene,
    card: &crate::ui::groups::Card,
    object: &Object,
    mesh: Option<&Mesh>,
) -> Value {
    let picked = card.picked.filter(|picked| picked.mesh == object.mesh && object.id == scene.active_id());
    json!({
        "count": mesh.map_or(0, Mesh::group_count),
        "colours_shown": card.colours,
        "picked": picked.map(|picked| json!({ "group": picked.group, "faces": picked.faces })),
    })
}

/// The face group under the point `pos` of object `object`'s own placement
/// (`copy` 0), for `pick`: 0 is surface in no group. `None` for a hit on a
/// mirror copy, whose faces are the object's seen through a reflection.
pub(in crate::app) fn group_at(scene: &Scene, slot: &Mesh, object: u64, copy: u16, pos: glam::Vec3) -> Value {
    let found = (copy == 0).then(|| scene.get(object)).flatten().and_then(|o| {
        let mesh = scene.mesh(o.mesh, slot)?;
        let local = o.matrix().inverse().transform_point3(pos);
        let hidden = |t: u32| mesh.is_triangle_hidden(t);
        let (t, _) = mesh.bvh.nearest_triangle(&mesh.positions, &mesh.indices, local, f32::INFINITY, hidden)?;
        Some(mesh.group_of(t))
    });
    found.map_or(Value::Null, |group| json!(group))
}

fn number(args: &Value, name: &str) -> Option<f64> {
    args.get(name).and_then(Value::as_f64).filter(|v| v.is_finite())
}

impl Running {
    /// The group a command names: by `x` and `y`, by `group`, or with
    /// neither the group picked. `prefix` is put before each name ("into_"
    /// for the group a merge goes into).
    fn group_asked(&mut self, args: &Value, prefix: &str) -> Result<u16, String> {
        let name = |what: &str| format!("{prefix}{what}");
        if let (Some(x), Some(y)) = (number(args, &name("x")), number(args, &name("y"))) {
            let pos = Vec2::new(x as f32, y as f32) * self.view.size;
            return self.group_under(pos).ok_or_else(|| "The active object is not under that position.".to_owned());
        }
        if let Some(group) = number(args, &name("group")) {
            let group = group.round().clamp(0.0, u16::MAX as f64) as u16;
            if self.doc.mesh.group_triangles(group, true).is_empty() {
                return Err(format!("The active object has no group {group}."));
            }
            return Ok(group);
        }
        match self.group_picked().filter(|_| prefix.is_empty()) {
            Some(picked) => Ok(picked.group),
            None => Err(format!(
                "This command needs a group: {} and {} (a window position, as pick takes), or {} (a group's number, from pick).",
                name("x"),
                name("y"),
                name("group")
            )),
        }
    }

    /// The bridge's commands for face groups, by the name its `command` tool
    /// was given with the underscores taken out. `None` if `name` is not one
    /// of them.
    pub(in crate::app) fn groups_command(&mut self, name: &str, args: &Value) -> Option<Result<String, String>> {
        let on_group = |app: &mut Running, ask: Ask| -> Result<String, String> {
            let group = app.group_asked(args, "")?;
            app.group_pick(group);
            app.said(|app| app.groups_ask(ask))
        };
        Some(match name {
            "groupmasked" => {
                if let Some(keep) = args.get("keep_mask").and_then(Value::as_bool) {
                    self.doc.opts.groups.keep_mask = keep;
                }
                self.said(|app| app.groups_ask(Ask::GroupMasked))
            }
            "groupvisible" => self.said(|app| app.groups_ask(Ask::GroupVisible)),
            "autogroups" => self.said(|app| app.groups_ask(Ask::Auto)),
            "groupsbyangle" => {
                if let Some(angle) = number(args, "angle") {
                    self.doc.opts.groups.angle = (angle as f32).clamp(1.0, 180.0);
                }
                self.said(|app| app.groups_ask(Ask::ByAngle))
            }
            "cleargroups" => self.said(|app| app.groups_ask(Ask::Clear)),
            "pickgroup" => self.group_asked(args, "").map(|group| {
                let picked = self.group_pick(group);
                json!({ "group": group, "faces": picked.faces }).to_string()
            }),
            "isolategroup" => on_group(self, Ask::Isolate),
            "hidegroup" => on_group(self, Ask::Hide),
            "maskgroup" => on_group(self, Ask::Mask { others: false }),
            "maskallbutgroup" => on_group(self, Ask::Mask { others: true }),
            "mergegroups" => self.group_asked(args, "").and_then(|from| {
                let into = self.group_asked(args, "into_")?;
                self.group_pick(from);
                // As the card does it: Merge Into armed, then the click on the other group.
                self.groups_ask(Ask::Arm(Some(Armed::Merge)));
                self.said(|app| {
                    let armed = app.doc.opts.groups.armed.take();
                    app.groups_clicked(armed, Some(into));
                })
            }),
            "showallfaces" => self.said(|app| app.act(Action::ShowAllFaces)),
            "showgroupcolours" => {
                let on = args.get("on").and_then(Value::as_bool).unwrap_or(true);
                self.doc.opts.groups.colours = on;
                self.redraw = true;
                Ok(if on { "Group colours are showing." } else { "Group colours are off." }.to_owned())
            }
            _ => return None,
        })
    }
}
