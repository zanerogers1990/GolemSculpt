//! `--demo-bake`: UVs & Maps worked through the way a person goes: a lumpy
//! sculpt, a quad remesh of it as the low model, Unwrap UVs on that, then
//! Bake Maps from the sculpt into a scratch folder; prints `ok` or `FAIL`
//! for each thing that must then be true. Nothing here opens a file box, and
//! everything written goes in the run's own scratch folder (`prefs::scratch`).

use super::super::Running;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

/// A ball covered in fine ripples, finer than a low model of it can follow:
/// the detail a normal map is for.
fn lumpy() -> Mesh {
    let ball = primitives::build(Primitive::Sphere, 200_000, 0.5);
    let r = ball.positions[0].length();
    let positions = ball
        .positions
        .iter()
        .map(|&p| {
            let q = p / r * 40.0;
            let rise = 0.012 * r * q.x.sin() * q.y.sin() * q.z.sin();
            p + p.normalize() * rise
        })
        .collect();
    Mesh::new(positions, ball.indices.clone(), None)
}

impl Running {
    pub(in crate::app) fn bake_play(&mut self) {
        let folder = crate::prefs::scratch("sculpt_bake_play");
        let _ = std::fs::remove_dir_all(&folder);
        let sculpt = lumpy();
        let low = match crate::quadremesh::quad_remesh_mesh(&sculpt, 1_500) {
            Ok((low, _)) => low,
            Err(why) => return say(&format!("the quad remesh of the sculpt: {why}"), false),
        };
        let sculpt_id = self.add_object("Lumpy Sculpt", sculpt);
        let low_id = self.add_object("Low", low);
        say("the low model starts without UVs", !self.doc.mesh.has_uvs());

        // Bake before unwrapping: refused, with the way out named.
        self.bake_maps(folder.clone());
        say("baking onto a model without UVs is refused", self.bake.is_none() && self.ui.status.0.contains("Unwrap UVs"));

        self.unwrap_uvs();
        say("Unwrap UVs gives it UVs", self.doc.mesh.has_uvs() && self.doc.mesh.uvs().len() == self.doc.mesh.indices.len());
        say("as one undo step named for it", self.doc.history.next_name(true) == Some("Unwrap UVs"));

        let c = &mut self.doc.opts.bake;
        (c.from, c.size, c.normal, c.displacement, c.ao, c.colour) = (Some(sculpt_id), 1024, true, true, true, false);
        self.bake_maps(folder.clone());
        say("Bake Maps starts in the background", self.bake.is_some());
        self.bake_wait();
        say(&format!("and says where the maps went ({})", self.ui.status.0), self.ui.status.0.starts_with("Baked 3 maps"));
        let file = |end: &str| folder.join(format!("Low_{end}"));
        let written = ["normal.png", "displacement.exr", "displacement.png", "ao.png"].iter().all(|f| file(f).is_file());
        say("the normal, displacement and ambient occlusion files are there", written);
        let normal = image::open(file("normal.png")).map(|i| i.to_rgb8());
        let tilted = normal.as_ref().is_ok_and(|n| {
            n.pixels().filter(|p| (p.0[0] as i32 - 128).abs() > 24 || (p.0[1] as i32 - 128).abs() > 24).count() > 500
        });
        say("the normal map shows the ripples the low model does not have", tilted);
        let flat = normal.as_ref().is_ok_and(|n| n.pixels().filter(|p| p.0 == [128, 128, 255]).count() > 1000);
        say("and is flat where the two surfaces agree or nothing is laid out", flat);

        // The low model left as it was by the bake, and the unwrap taken back by Undo.
        say("the bake left the low model selected", self.doc.scene.active().id == low_id);
        self.act(crate::doc::Action::Undo);
        say("Undo takes the UVs away again", !self.doc.mesh.has_uvs());
    }
}
