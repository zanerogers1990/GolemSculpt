//! `SCULPT_DEMO_SYMMETRY=repair`: the Symmetry card's repairs pressed in
//! turn on a model made lopsided for them, through the same actions the
//! buttons send, with a line per check.

use glam::{Quat, Vec3};
use sculpt_symmetry::Axis;

use super::symmetry_of;
use crate::app::Running;
use crate::doc::Action;
use crate::mesh::Mesh;
use crate::mesh::primitives::{self, Primitive};

/// A model that is its own mirror image across X, as two halves that were
/// joined and never welded: each has its own copy of every vertex of the
/// seam, a hair to its own side of the plane.
fn two_halves(positions: &[Vec3], indices: &[u32]) -> (Vec<Vec3>, Vec<u32>) {
    let (mut halves, mut triangles) = (Vec::new(), Vec::new());
    for positive in [true, false] {
        let mut slot = vec![u32::MAX; positions.len()];
        for triangle in indices.chunks_exact(3) {
            let across: f32 = triangle.iter().map(|&v| positions[v as usize].x).sum();
            if (across > 0.0) != positive {
                continue;
            }
            for &v in triangle {
                if slot[v as usize] == u32::MAX {
                    slot[v as usize] = halves.len() as u32;
                    let p = positions[v as usize];
                    let gap = if p.x != 0.0 { 0.0 } else if positive { 2e-5 } else { -2e-5 };
                    halves.push(p + Vec3::X * gap);
                }
                triangles.push(slot[v as usize]);
            }
        }
    }
    (halves, triangles)
}

impl Running {
    /// A lumpy model that is its own mirror image across X, then given a
    /// swelling on its +X side only. Check Symmetry is pressed on it, then
    /// Make Symmetric -X to +X, which is undone and redone; the row of
    /// vertices on the plane is then knocked off it and Symmetry Cleanup
    /// pressed. Last come the two that end another way: Make Symmetric on a
    /// model with an odd part on one side, which it must refuse, and Symmetry
    /// Cleanup on two halves joined and never welded, which it must weld into
    /// one closed surface. Prints the status line each press left and a line per check
    /// (ok / FAIL). The picture is the welded model from the front, in
    /// wireframe, with the cards above the Symmetry card shut.
    pub(in crate::app) fn symmetry_repair_demo(&mut self) {
        let shut = ["Brushes", "Clay Settings", "Move Settings"].map(|card| (card.to_owned(), false));
        crate::prefs::cards::restore(&self.egui_ctx, &shut.into_iter().collect());
        let ball = primitives::build(Primitive::Sphere, 6_000, 0.5);
        let lumpy = |p: Vec3| {
            let dir = p.normalize();
            let lumps = (dir.y * 7.0 + 1.0).sin() * (dir.z * 6.0 + 0.5).sin() + 0.5 * (dir.x * 9.0 + dir.y * 5.0).sin();
            Quat::from_rotation_y(0.4) * (dir * (0.5 + 0.11 * lumps))
        };
        let positions: Vec<Vec3> = ball.positions.iter().map(|&p| lumpy(p)).collect();
        self.replace_mesh(Mesh::new(positions, ball.indices.clone(), None), false);
        // Exactly symmetric to begin with, by the button that makes a model so.
        self.act(Action::CutAndMirror { axis: Axis::X, from_positive: true });

        let centre = Vec3::new(0.35, 0.2, 0.3);
        let swelling = |p: Vec3| 0.06 * (-(p - centre).length_squared() / (2.0 * 0.12 * 0.12)).exp();
        let mesh = &self.doc.mesh;
        let swollen = mesh.positions.iter().map(|&p| if p.x > 0.0 { p + p.normalize() * swelling(p) } else { p });
        let swollen = Mesh::new(swollen.collect(), mesh.indices.clone(), None);
        self.replace_mesh(swollen, false);
        let triangles = self.doc.mesh.indices.clone();
        let symmetric = |app: &Running| symmetry_of(&app.doc.mesh, "", Axis::X, app.match_tolerance()).1;
        let good_news = |app: &Running| app.ui.status.1 == crate::ui::theme::OK;
        let press = |app: &mut Running, action: Action| {
            app.act(action);
            println!("{}", app.ui.status.0);
        };
        let mut checks = Vec::new();

        press(self, Action::CheckSymmetry { axis: Axis::X });
        checks.push(("Check Symmetry says the swollen model is not symmetric", !good_news(self)));
        checks.push(("and changes nothing", self.doc.history.steps() == (0, 0) && self.doc.mesh.indices == triangles));

        press(self, Action::MakeSymmetric { axis: Axis::X, from_positive: false });
        checks.push(("Make Symmetric makes it symmetric", good_news(self) && symmetric(self)));
        checks.push(("with every triangle as it was", self.doc.mesh.indices == triangles));
        checks.push(("as one undo step", self.doc.history.steps() == (1, 0)));
        self.act(Action::Undo);
        checks.push(("one Undo brings the swelling back", !symmetric(self) && self.doc.history.steps() == (0, 1)));
        self.act(Action::Redo);
        checks.push(("and Redo repairs it again", symmetric(self)));
        press(self, Action::CheckSymmetry { axis: Axis::X });
        checks.push(("Check Symmetry now says it is symmetric", good_news(self)));
        press(self, Action::MakeSymmetric { axis: Axis::X, from_positive: false });
        checks.push(("pressed again there is nothing to do, and no step", self.doc.history.steps() == (1, 0)));

        // The row on the plane, each vertex a little to one side of it or the other.
        let mesh = &self.doc.mesh;
        let mut knocked = mesh.positions.clone();
        let mut row = 0;
        for p in knocked.iter_mut().filter(|p| p.x == 0.0) {
            p.x = if row % 2 == 0 { 4e-4 } else { -3e-4 };
            row += 1;
        }
        let knocked = Mesh::new(knocked, mesh.indices.clone(), None);
        self.replace_mesh(knocked, false);
        let on_plane = |app: &Running| app.doc.mesh.positions.iter().filter(|p| p.x == 0.0).count();
        checks.push(("the row on the plane is knocked off it", row > 20 && on_plane(self) == 0));
        press(self, Action::SymmetryCleanup { axis: Axis::X });
        checks.push(("Symmetry Cleanup puts the row back on the plane", good_news(self) && on_plane(self) == row));
        checks.push(("as one undo step", self.doc.history.steps() == (1, 0)));
        press(self, Action::SymmetryCleanup { axis: Axis::X });
        checks.push(("pressed again the model is clean, and no step", self.doc.history.steps() == (1, 0)));

        // The two cases that end another way: a model Make Symmetric must refuse, and a seam Cleanup must weld.
        let (clean, clean_triangles) = (self.doc.mesh.positions.clone(), self.doc.mesh.indices.clone());
        let part = primitives::build(Primitive::Sphere, 2_000, 0.2);
        let mut positions = clean.clone();
        positions.extend(part.positions.iter().map(|&p| p + Vec3::new(1.3, 0.0, 0.0)));
        let mut indices = clean_triangles.clone();
        indices.extend(part.indices.iter().map(|&i| i + clean.len() as u32));
        self.replace_mesh(Mesh::new(positions, indices, None), false);
        let before = self.doc.mesh.positions.clone();
        press(self, Action::MakeSymmetric { axis: Axis::X, from_positive: true });
        let refused = !good_news(self) && self.ui.status.0.starts_with("Too asymmetric to match up");
        checks.push(("Make Symmetric refuses a model with an odd part on one side", refused));
        let names_it = self.ui.status.0.contains("use Cut & Mirror +X to -X instead");
        checks.push(("and names the Cut & Mirror button", names_it));
        let untouched = self.doc.mesh.positions == before && self.doc.history.steps() == (0, 0);
        checks.push(("nothing moved, and no step", untouched));

        let (positions, indices) = two_halves(&clean, &clean_triangles);
        let doubled = positions.len();
        self.replace_mesh(Mesh::new(positions, indices, None), false);
        let closed = |app: &Running| crate::printable::census(&app.doc.mesh).is_closed();
        let open = doubled == clean.len() + row && !closed(self);
        checks.push(("two halves joined and not welded are an open seam", open));
        press(self, Action::SymmetryCleanup { axis: Axis::X });
        let vertices = |app: &Running| app.doc.mesh.vertex_count();
        checks.push(("Symmetry Cleanup welds them into one closed surface", good_news(self) && closed(self)));
        checks.push(("with one vertex where each pair of the seam was", vertices(self) == clean.len()));
        checks.push(("as one undo step", self.doc.history.steps() == (1, 0)));
        self.act(Action::Undo);
        checks.push(("one Undo brings the two halves back", vertices(self) == doubled && !closed(self)));
        self.act(Action::Redo);
        checks.push(("and Redo welds them again", vertices(self) == clean.len() && closed(self)));

        for (what, ok) in checks {
            println!("{} {what}", if ok { "ok  " } else { "FAIL" });
        }
        self.doc.opts.wireframe = true;
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, false);
        (self.doc.camera.yaw, self.doc.camera.pitch) = (0.0, 12.0);
        self.input.inside = false;
        self.sync_view();
    }
}
