//! Telling the user when a model will not print, on the app's side.
//!
//! What "will not print" means and how it is worded are in `printable.rs`.
//! Here are the places the running app asks: after anything that rebuilt the
//! selected object's mesh ([`Running::status_checked`]; the hosted tools get
//! the same from `app/tools.rs`), after a stroke that added triangles, when
//! Check for Printing is pressed, and before an STL for printing is written,
//! where a model that is not closed is exported only after the user has said
//! so.
//!
//! Nothing here repairs a mesh. A Remesh does that, and the words say so.

use std::path::{Path, PathBuf};

use glam::{Vec2, Vec3};
use winit::event::MouseButton;
use winit::keyboard::{KeyCode, ModifiersState};

use super::Running;
use crate::doc::Action;
use crate::io;
use crate::mesh::Mesh;
use crate::printable::{self, Cure};
use crate::sculpt::Brush;
use crate::ui::question::Question;

/// Which triangles a mesh has. Two stamps that are equal name the same
/// triangles: a mesh's number is its own for good, and its topology version
/// goes up every time dynamic topology, or an undo or redo of it, adds,
/// rewrites or takes away a triangle, which are the only things that change
/// a mesh's triangles in place. (The triangle count will not do for this: an
/// undo and a different stroke can come back to the same count.)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Stamp {
    uid: u64,
    version: u64,
}

impl Stamp {
    fn of(mesh: &Mesh) -> Stamp {
        Stamp { uid: mesh.uid, version: mesh.topology_version() }
    }
}

/// How closed a mesh was when it had the triangles `stamp` names.
#[derive(Clone, Copy, Debug)]
struct Baseline {
    stamp: Stamp,
    census: printable::EdgeCensus,
}

/// Holds strokes that add triangles to leaving the mesh no more open than
/// they found it.
///
/// Counting a mesh's edges is a pass over all of it (6 ms for a million
/// triangles, 22 ms for four), and pen-down is the worst place for one. So
/// what was counted is kept for as long as the mesh has the same triangles,
/// and a stroke costs:
///
///   - nothing, when it cannot add triangles: dynamic topology off, a brush
///     that does not refine, mask painting, or a mesh at the triangle limit;
///   - one count when it ends, if it did change the triangles;
///   - one more at pen-down only when the triangles are not the ones last
///     counted: the first such stroke on a mesh, and the first after an undo
///     or redo of a stroke that had changed them. Undo and redo of anything
///     else leave what is known standing.
#[derive(Default)]
pub(super) struct StrokeWatch {
    known: Option<Baseline>,
    /// The triangles the stroke under way began on.
    from: Option<Stamp>,
    /// How many times a mesh has been counted, for the checks.
    counted: u32,
}

impl StrokeWatch {
    fn count(&mut self, mesh: &Mesh) -> Baseline {
        self.counted += 1;
        Baseline { stamp: Stamp::of(mesh), census: printable::census(mesh) }
    }

    /// A stroke is about to begin on `mesh`. `may_grow` says whether it can
    /// change the triangles; only then is the mesh counted, and only if what
    /// is known is not of these triangles.
    fn begin(&mut self, mesh: &Mesh, may_grow: bool) {
        let now = Stamp::of(mesh);
        self.from = Some(now);
        if may_grow && !self.known.is_some_and(|known| known.stamp == now) {
            self.known = Some(self.count(mesh));
        }
    }

    /// The stroke has ended. If it changed the triangles of the mesh it
    /// began on, and that mesh was counted then, counts it again and returns
    /// how closed it was before and is now.
    fn end(&mut self, mesh: &Mesh) -> Option<(printable::EdgeCensus, printable::EdgeCensus)> {
        let (from, now) = (self.from.take()?, Stamp::of(mesh));
        if from.uid != now.uid || from == now {
            return None;
        }
        let before = self.known.filter(|known| known.stamp == from)?;
        let after = self.count(mesh);
        self.known = Some(after);
        Some((before.census, after.census))
    }

    /// Whether `mesh` as it is now has been counted, and came to `census`.
    fn knows(&self, mesh: &Mesh, census: &printable::EdgeCensus) -> bool {
        self.known.is_some_and(|known| known.stamp == Stamp::of(mesh) && known.census == *census)
    }

    /// For the scripted check: as if a stroke under way had begun on `mesh`
    /// before its last change of triangles, and had found it closed then.
    fn suppose_closed_before(&mut self, mesh: &Mesh) {
        let now = Stamp::of(mesh);
        let stamp = Stamp { version: now.version.wrapping_sub(1), ..now };
        let closed = printable::EdgeCensus { holes: Some(0), ..Default::default() };
        (self.known, self.from) = (Some(Baseline { stamp, census: closed }), Some(stamp));
    }
}

impl Running {
    /// Call before a stroke on the selected object begins, with whether Shift
    /// makes it a smoothing one. See [`StrokeWatch`] for what it costs.
    pub(super) fn stroke_baseline(&mut self, smooth_held: bool) {
        let doc = &self.doc;
        self.print_watch.begin(&doc.mesh, doc.sculptor.may_refine(&doc.mesh, smooth_held));
    }

    /// Call when a stroke has ended. If it changed the mesh's triangles, the
    /// mesh is counted again, and the status line says so if the stroke left
    /// it with more holes or crowded edges than it found. Dynamic topology is
    /// built and tested never to do that; this is the check that it has not.
    pub(super) fn stroke_check(&mut self) {
        let Some((before, after)) = self.print_watch.end(&self.doc.mesh) else { return };
        let name = self.doc.scene.active().name.clone();
        if let Some(text) = printable::torn_by_stroke(&name, &before, &after) {
            self.status(text, false);
        }
    }

    /// Puts up the status of an operation that rebuilt the selected object,
    /// with a warning after it if the object is not a closed surface now. The
    /// warning turns the line the colour of bad news: the operation worked,
    /// but what it left may not print, and that is the part to read.
    pub(super) fn status_checked(&mut self, text: impl Into<String>, cure: Cure) {
        let text = text.into();
        let name = self.doc.scene.active().name.clone();
        match printable::warning(&[(&name, printable::census(&self.doc.mesh))], cure) {
            Some(warning) => self.status(format!("{text} {warning}"), false),
            None => self.status(text, true),
        }
    }

    /// Check for Printing: says whether the selected object is a closed
    /// surface, and if not, what is wrong with it and what closes it.
    pub(super) fn check_for_printing(&mut self) {
        let name = self.doc.scene.active().name.clone();
        let (text, ok) = printable::verdict(&name, &self.doc.mesh);
        self.toast("Check for Printing");
        self.status(text, ok);
    }

    /// Export STL for Printing. A closed object goes straight to the save
    /// box. One that is not closed is asked about first, with what is wrong
    /// with it: the answer Export Anyway comes back as `export_stl_now`.
    /// (So in a scripted run, press this only on an object that is not
    /// closed: the save box waits for a person.)
    pub(super) fn export_stl_asking(&mut self) {
        let scene = &self.doc.scene;
        let (object, name) = (scene.active_id(), scene.active().name.clone());
        match printable::export_question(&name, &printable::census(&self.doc.mesh)) {
            Some(text) => {
                self.ui.graph.ask(Question::ExportNotClosed { object, text });
                self.redraw = true;
            }
            None => self.export_stl_now(),
        }
    }

    /// Asks where to put the STL, and writes it.
    pub(super) fn export_stl_now(&mut self) {
        let file = io::export_file_name(&self.doc.scene.active().name, "stl");
        let pick = rfd::FileDialog::new().add_filter("STL", &["stl"]).set_file_name(file).save_file();
        if let Some(path) = pick {
            if let Err(e) = self.export_stl_to(&path) {
                self.status(e, false);
            }
        }
    }

    /// Writes the selected object, where it stands and with its live copies,
    /// as an STL for a slicer. The status line says where it went, and says
    /// again if it was not closed: the file is out of the app's hands now.
    /// Returns that line, or why nothing was written.
    pub(super) fn export_stl_to(&mut self, path: &Path) -> Result<String, String> {
        let name = self.doc.scene.active().name.clone();
        let models: Vec<glam::Mat4> = self.export_places().into_iter().map(|(_, m)| m).collect();
        io::export_stl_placed(&self.doc.mesh, &models, path)?;
        let note = self.export_note();
        let mut text = format!("Exported \"{name}\"{note} to {} (millimetres, Z up).", path.display());
        let faults = printable::faults(&printable::census(&self.doc.mesh));
        if let Some(faults) = &faults {
            text += &format!(" It has {faults}, so it may not print.");
        }
        self.status(text.clone(), faults.is_none());
        Ok(text)
    }

    /// `--demo-print`, for a look at the printing check with `--screenshot`.
    /// The selected object first has two holes cut in it and one face
    /// doubled, so there is something to find (`closed` leaves it whole):
    ///
    ///   - `check` and `closed` press Check for Printing;
    ///   - `export` presses Export STL for Printing, which asks;
    ///   - `remesh` and `quads` run that on the damaged object, for what the
    ///     status line says afterwards;
    ///   - `damaged` only does the damage, for a hosted tool to work on
    ///     (`--demo-print damaged --demo-tool trim`);
    ///   - `play` works through all of it and prints a line per check.
    pub(super) fn print_demo(&mut self, what: &str) {
        // The Objects card shut, so the File card and its buttons are in view.
        let shut = std::collections::BTreeMap::from([("Objects".to_owned(), false)]);
        crate::prefs::cards::restore(&self.egui_ctx, &shut);
        if what != "closed" {
            let damaged = damaged(&self.doc.mesh);
            self.replace_mesh(damaged, true);
        }
        match what {
            "export" => self.act(Action::ExportStl),
            "remesh" => self.act(Action::Remesh),
            "quads" => self.act(Action::QuadRemesh),
            "play" => self.print_play(),
            "damaged" => {}
            _ => self.act(Action::CheckForPrinting),
        }
    }

    /// Works the printing check the way a person would and prints what came
    /// of each step. The selected object comes in damaged (`print_demo`).
    fn print_play(&mut self) {
        let folder = demo_folder();
        let census = printable::census(&self.doc.mesh);
        println!("     the damaged object: {census:?}");
        say("the damage is two holes and a doubled face", census.holes == Some(2) && census.crowded == 3);

        self.act(Action::CheckForPrinting);
        println!("     {}", self.ui.status.0);
        say("Check for Printing says it may not print, in the colour of bad news", self.says("may not print", false));
        say("and names a Remesh as the way to close it", self.ui.status.0.contains("A Remesh (R) usually closes it."));

        // Hidden faces are part of the model: hiding the damage hides nothing from the check.
        self.demo_hide();
        let hidden = self.doc.mesh.hidden_triangle_count();
        self.act(Action::CheckForPrinting);
        let same = printable::census(&self.doc.mesh) == census;
        say("with half of it hidden the check finds the same", hidden > 0 && same);
        self.act(Action::ShowAllFaces);

        // Export asks first, and Esc writes nothing.
        self.act(Action::ExportStl);
        say("Export STL for Printing asks before writing a model that is not closed", self.ui.graph.asking());
        self.confirm_key(KeyCode::Escape);
        say("Esc takes the question down", !self.ui.graph.asking());
        self.act(Action::ExportStl);
        let answer = self.ui.graph.answer(true);
        say("and its first answer is to export anyway", answer == Some(Action::ExportStlAnyway));
        // That answer opens the save box; the scripted run goes straight to what the box leads to.
        let path = folder.join("holed.stl");
        let exported = self.export_stl_to(&path);
        let wrote = std::fs::metadata(&path).map_or(0, |m| m.len());
        let expected = 84 + 50 * self.doc.mesh.triangle_count() as u64;
        println!("     {}", self.ui.status.0);
        say("exporting anyway writes every triangle", exported.is_ok() && wrote == expected);
        say("and the status line says once more that it may not print", self.says("so it may not print", false));

        // A stroke that adds triangles is checked, and not blamed for holes that were there already.
        let (triangles, said) = (self.doc.mesh.triangle_count(), self.ui.status.0.clone());
        self.play_refining_stroke();
        let added = self.doc.mesh.triangle_count() - triangles;
        let now = printable::census(&self.doc.mesh);
        println!("     a stroke with dynamic topology on added {added} triangles: {now:?}");
        say("a stroke with dynamic topology on adds triangles", added > 0);
        say("the mesh is counted again when the stroke ends", self.print_watch.knows(&self.doc.mesh, &now));
        say("it made no hole, and is not said to have", now.holes == census.holes && self.ui.status.0 == said);
        // Had the model been closed before it, the same stroke would be answered for.
        self.print_watch.suppose_closed_before(&self.doc.mesh);
        self.stroke_check();
        println!("     {}", self.ui.status.0);
        say("a stroke that did open the surface is told so", self.says("That stroke opened the surface of", false));
        self.play_stroke_costs();

        // A remesh closes it, and says that it did.
        self.act(Action::Remesh);
        println!("     {}", self.ui.status.0);
        say("a Remesh says the holes were closed over", self.says("had holes, which were closed over flat", true));
        say("and leaves a closed surface", printable::census(&self.doc.mesh).is_closed());
        self.act(Action::CheckForPrinting);
        println!("     {}", self.ui.status.0);
        say("which Check for Printing passes", self.says("is a closed surface", true));
        // Not pressed here: with nothing to ask, the button opens the save box, and a
        // scripted run must not put one on the screen of whoever is at the machine.
        let name = self.doc.scene.active().name.clone();
        let question = printable::export_question(&name, &printable::census(&self.doc.mesh));
        say("and Export has nothing to ask about it", question.is_none());
        // The remesh left a mesh no stroke has been on yet.
        self.play_strokes_that_cannot_refine();
    }

    /// What the check costs a stroke, by how many times the mesh is counted.
    /// Dynamic topology is on for every stroke here; the mesh comes in
    /// counted as the refining stroke left it.
    fn play_stroke_costs(&mut self) {
        let counted = |app: &Running| app.print_watch.counted;
        let before = counted(self);
        self.print_stroke(Brush::Smooth, 0.0);
        say("a Smooth stroke with dynamic topology on does not count the mesh", counted(self) == before);
        // Undo of a stroke that changed no triangles: what was counted still stands.
        self.act(Action::Undo);
        let triangles = self.doc.mesh.triangle_count();
        self.print_brush(Brush::Clay);
        self.print_press(60.0);
        say("nor does the next pen-down, after undoing that stroke", counted(self) == before);
        self.print_drag(60.0);
        let grew = self.doc.mesh.triangle_count() > triangles;
        say("a stroke that adds triangles counts it once, when it ends", grew && counted(self) == before + 1);
        // Undo of one that did: the triangles are others now, and are counted once more, at pen-down.
        self.act(Action::Undo);
        self.print_press(60.0);
        say("and the pen-down after undoing that one counts it once more", counted(self) == before + 2);
        self.print_drag(60.0);
    }

    /// Strokes that cannot add triangles, on a mesh that has not been
    /// counted, with dynamic topology on: none of them is a reason to start.
    fn play_strokes_that_cannot_refine(&mut self) {
        let before = self.print_watch.counted;
        self.print_stroke(Brush::Smooth, 0.0);
        self.print_stroke(Brush::Move, 0.0);
        self.print_brush(Brush::Clay);
        self.input.mods = ModifiersState::SHIFT;
        self.print_press(0.0);
        self.print_drag(0.0);
        self.print_brush(Brush::Clay);
        self.doc.sculptor.settings.mask_mode = true;
        self.print_press(0.0);
        self.print_drag(0.0);
        self.doc.sculptor.settings.mask_mode = false;
        let same = self.print_watch.counted == before;
        say("Smooth, Move, Shift and mask strokes do not count a mesh that was never counted", same);
    }

    /// One Clay stroke across the middle of the view with dynamic topology
    /// on and set fine, through the routines the pointer drives.
    fn play_refining_stroke(&mut self) {
        self.print_stroke(Brush::Clay, 0.0);
    }

    /// One stroke of `brush`, `below` pixels under the middle of the view,
    /// with dynamic topology on and set fine.
    fn print_stroke(&mut self, brush: Brush, below: f32) {
        self.print_brush(brush);
        self.print_press(below);
        self.print_drag(below);
    }

    /// Takes up `brush` for the strokes to come, with no key held, and turns
    /// dynamic topology on, set fine.
    fn print_brush(&mut self, brush: Brush) {
        let settings = &mut self.doc.sculptor.settings;
        (settings.brush, settings.mask_mode) = (brush, false);
        settings.radius_px = 0.05 * self.view.size.y;
        settings.per_brush[brush.index()].strength = 0.5;
        self.doc.sculptor.dyntopo.detail = crate::sculpt::dyntopo::MIN_DETAIL;
        self.doc.edit_symmetry(|symmetry| symmetry.mirror = [false; 3]);
        self.sync_view();
        (self.input.inside, self.input.mods) = (true, ModifiersState::empty());
    }

    /// Pen-down `below` pixels under the middle of the view, dynamic
    /// topology on.
    fn print_press(&mut self, below: f32) {
        self.doc.sculptor.dyntopo.enabled = true;
        self.pointer_moved(self.print_start(below), None);
        self.pointer_pressed(MouseButton::Left, None);
    }

    /// The rest of the stroke `print_press` began: a short drag and the
    /// release. Dynamic topology goes off again.
    fn print_drag(&mut self, below: f32) {
        let from = self.print_start(below);
        for step in 1..=20 {
            self.pointer_moved(from + Vec2::new(step as f32 * 3.0, 0.0), None);
        }
        self.pointer_released(MouseButton::Left);
        self.doc.sculptor.dyntopo.enabled = false;
    }

    fn print_start(&self, below: f32) -> Vec2 {
        self.view.origin + self.view.size * 0.5 + Vec2::new(0.0, below)
    }

    /// Whether the status line holds `words`, in the colour of good news or bad.
    fn says(&self, words: &str, good: bool) -> bool {
        let colour = if good { crate::ui::theme::OK } else { crate::ui::theme::ERROR };
        self.ui.status.0.contains(words) && self.ui.status.1 == colour
    }
}

/// A line of the scripted run: what was checked and how it came out.
fn say(what: &str, ok: bool) {
    println!("{} {what}", if ok { "ok  " } else { "FAIL" });
}

/// Where the scripted run writes its STL: never beside the user's own files.
fn demo_folder() -> PathBuf {
    let folder = std::env::temp_dir().join("sculpt_print_demo");
    let _ = std::fs::remove_dir_all(&folder);
    if let Err(e) = std::fs::create_dir_all(&folder) {
        log::error!("{e}");
    }
    folder
}

/// A copy of `mesh` that will not print, for the demo: two holes on the side
/// facing the camera, and one face listed twice, which gives each of its
/// three edges a third face.
fn damaged(mesh: &Mesh) -> Mesh {
    let bounds = mesh.bounds();
    let (centre, reach) = (bounds.center(), bounds.size().max_element() * 0.5);
    let holes = [Vec3::new(0.45, 0.35, 0.82), Vec3::new(-0.3, -0.2, 0.93)].map(|d| centre + d.normalize() * reach);
    let radius = reach * 0.16;
    let middle = |t: &[u32]| t.iter().map(|&v| mesh.positions[v as usize]).sum::<Vec3>() / 3.0;
    let kept = mesh.indices.chunks_exact(3).filter(|t| holes.iter().all(|&hole| middle(t).distance(hole) > radius));
    let mut indices: Vec<u32> = kept.flatten().copied().collect();
    let again = [indices[0], indices[1], indices[2]];
    indices.extend_from_slice(&again);
    Mesh::new(mesh.positions.clone(), indices, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::History;
    use crate::mesh::primitives::{self, Primitive};
    use crate::sculpt::Sculptor;

    fn ball() -> Mesh {
        primitives::build(Primitive::Sphere, 20_000, 0.5)
    }

    /// A stroke that splits exactly `splits` edges of the ball around the
    /// point of it toward `toward`, and so adds twice as many triangles, as
    /// an undo step.
    fn refining_stroke(mesh: &mut Mesh, history: &mut History, toward: Vec3, splits: usize) {
        let triangles = mesh.triangle_count();
        mesh.begin_stroke();
        let stats = mesh.refine(&[toward * 0.5], 0.2, 0.002, splits, usize::MAX);
        assert_eq!(stats.splits, splits);
        history.push_stroke(mesh.end_stroke(false).expect("the stroke changed the mesh"));
        assert_eq!(mesh.triangle_count(), triangles + 2 * splits);
    }

    /// A stroke that moves vertices and changes no triangle, as an undo step.
    fn plain_stroke(mesh: &mut Mesh, history: &mut History) {
        let mut leaves = Vec::new();
        mesh.bvh.leaves_in_sphere(Vec3::Y * 0.5, 0.1, &mut leaves);
        leaves.sort_unstable();
        mesh.begin_stroke();
        mesh.move_vertices(&leaves, |_, vertex| (vertex.pos.y > 0.45).then(|| vertex.pos * 1.01));
        mesh.positions_changed(&leaves);
        history.push_stroke(mesh.end_stroke(false).expect("the stroke moved vertices"));
    }

    #[test]
    fn a_stroke_that_cannot_add_triangles_costs_no_count() {
        let (mut mesh, mut history, mut watch) = (ball(), History::default(), StrokeWatch::default());
        watch.begin(&mesh, false);
        plain_stroke(&mut mesh, &mut history);
        assert_eq!(watch.end(&mesh), None);
        assert_eq!(watch.counted, 0);
    }

    #[test]
    fn a_refining_stroke_counts_at_its_end_and_only_the_first_at_pen_down() {
        let (mut mesh, mut history, mut watch) = (ball(), History::default(), StrokeWatch::default());
        watch.begin(&mesh, true);
        assert_eq!(watch.counted, 1, "the first stroke that may add triangles counts the mesh it finds");
        refining_stroke(&mut mesh, &mut history, Vec3::Z, 30);
        let closed = printable::census(&mesh);
        assert_eq!(watch.end(&mesh), Some((closed, closed)));
        assert_eq!(watch.counted, 2);
        // The next one starts from what the last left: nothing at pen-down.
        watch.begin(&mesh, true);
        assert_eq!(watch.counted, 2);
        // And one that may refine but adds nothing costs nothing at all.
        assert_eq!(watch.end(&mesh), None);
        assert_eq!(watch.counted, 2);
    }

    #[test]
    fn undo_and_redo_of_a_stroke_that_changed_no_triangle_leave_what_is_known() {
        let (mut mesh, mut history, mut watch) = (ball(), History::default(), StrokeWatch::default());
        watch.begin(&mesh, true);
        refining_stroke(&mut mesh, &mut history, Vec3::Z, 30);
        watch.end(&mesh);
        watch.begin(&mesh, false);
        plain_stroke(&mut mesh, &mut history);
        watch.end(&mesh);
        let counted = watch.counted;
        for undo in [true, false, true] {
            let stepped = if undo { history.undo(&mut mesh) } else { history.redo(&mut mesh) };
            assert_eq!(stepped, Some(false));
            watch.begin(&mesh, true);
            assert_eq!(watch.counted, counted, "pen-down counted the mesh again after a step that left its triangles");
        }
    }

    #[test]
    fn undo_of_a_stroke_that_added_triangles_is_counted_once_at_the_next_pen_down() {
        let (mut mesh, mut history, mut watch) = (ball(), History::default(), StrokeWatch::default());
        watch.begin(&mesh, true);
        refining_stroke(&mut mesh, &mut history, Vec3::Z, 30);
        watch.end(&mesh);
        assert_eq!(history.undo(&mut mesh), Some(false));
        // A stroke that cannot refine does not ask, whatever was undone.
        watch.begin(&mesh, false);
        assert_eq!(watch.counted, 2);
        watch.begin(&mesh, true);
        assert_eq!(watch.counted, 3);
        watch.begin(&mesh, true);
        assert_eq!(watch.counted, 3);
    }

    /// Undo, then another stroke, can bring a mesh back to the triangle count
    /// that was counted with other triangles: that is not the mesh counted.
    #[test]
    fn the_same_count_of_other_triangles_is_not_taken_for_what_was_counted() {
        let (mut mesh, mut history, mut watch) = (ball(), History::default(), StrokeWatch::default());
        watch.begin(&mesh, true);
        refining_stroke(&mut mesh, &mut history, Vec3::Z, 30);
        watch.end(&mesh);
        let (triangles, indices) = (mesh.triangle_count(), mesh.indices.clone());
        // Back, and forward another way, by strokes the watch is not told of.
        assert_eq!(history.undo(&mut mesh), Some(false));
        refining_stroke(&mut mesh, &mut history, Vec3::X, 30);
        assert_eq!(mesh.triangle_count(), triangles);
        assert_ne!(mesh.indices, indices);
        let counted = watch.counted;
        watch.begin(&mesh, true);
        assert_eq!(watch.counted, counted + 1, "a mesh of other triangles passed for the one that was counted");
    }

    #[test]
    fn a_stroke_is_answered_for_only_against_the_mesh_it_began_on() {
        let (mut mesh, mut history, mut watch) = (ball(), History::default(), StrokeWatch::default());
        // Not expected to refine, and it did: there is no before to hold it to.
        watch.begin(&mesh, false);
        refining_stroke(&mut mesh, &mut history, Vec3::Z, 30);
        assert_eq!(watch.end(&mesh), None);
        // No stroke under way: nothing to end.
        assert_eq!(watch.end(&mesh), None);
        assert_eq!(watch.counted, 0);
        // Another mesh in hand when it ends (nothing in the app does this).
        watch.begin(&mesh, true);
        assert_eq!(watch.end(&ball()), None);
    }

    /// Whether a stroke may add triangles is asked of the brush it will use.
    #[test]
    fn only_a_stroke_that_can_add_triangles_is_said_to() {
        let mesh = ball();
        let mut sculptor = Sculptor::default();
        sculptor.settings.brush = Brush::Clay;
        assert!(!sculptor.may_refine(&mesh, false), "dynamic topology is off");
        sculptor.dyntopo.enabled = true;
        assert!(sculptor.may_refine(&mesh, false));
        assert!(!sculptor.may_refine(&mesh, true), "Shift makes it a Smooth stroke");
        sculptor.settings.mask_mode = true;
        assert!(!sculptor.may_refine(&mesh, false), "mask painting");
        assert!(!sculptor.may_refine(&mesh, true), "mask painting, smoothing the mask");
        sculptor.settings.mask_mode = false;
        for brush in Brush::ALL {
            sculptor.settings.brush = brush;
            assert_eq!(sculptor.may_refine(&mesh, false), crate::sculpt::dyntopo::refines(brush), "{brush:?}");
        }
    }

    /// At the triangle limit a refine turns back before it touches anything,
    /// so there is nothing to count the mesh for; one pair of triangles under
    /// it there is. Each answer is checked against what a refine then does.
    #[test]
    fn a_mesh_at_the_triangle_limit_is_not_counted_for_a_stroke_that_cannot_grow_it() {
        let mut mesh = ball();
        let triangles = mesh.triangle_count();
        let mut sculptor = Sculptor::default();
        (sculptor.settings.brush, sculptor.dyntopo.enabled) = (Brush::Clay, true);
        for (limit, may) in [(triangles, false), (triangles + 1, false), (triangles + 2, true), (triangles + 4, true)] {
            sculptor.dyntopo.limit_triangles(limit);
            assert_eq!(sculptor.may_refine(&mesh, false), may, "limit {limit} on {triangles} triangles");
            let version = mesh.topology_version();
            mesh.refine(&[Vec3::Z * 0.5], 0.2, 0.002, 1, limit);
            let changed = mesh.topology_version() != version;
            assert!(may || !changed, "a refine said to be impossible changed the mesh at limit {limit}");
            // With room for one split there is one, so the answers above are not all vacuous.
            assert!(changed || limit < triangles + 4, "nothing was refined with room to");
        }
    }

    #[test]
    fn the_demo_damage_is_two_holes_and_a_doubled_face() {
        let sphere = primitives::build(Primitive::Sphere, 20_000, 0.5);
        let found = printable::census(&damaged(&sphere));
        assert_eq!((found.holes, found.crowded), (Some(2), 3));
        assert!(found.open > 20, "{found:?}");
    }
}
