//! The SSphere blockout tool: sketch a form out of connected spheres, watch a
//! skin wrap over them, then turn that skin into an ordinary sculptable mesh.
//!
//! Split by concern:
//!   rig.rs    the armature itself (pure data)
//!   frame.rs  where the rig stands in the world, and attaching it to an object
//!   gizmo.rs  the transform gizmo's arrows on the selected sphere
//!   skin.rs   turning a rig into a mesh
//!   pose.rs   the two-bone solve behind posing a limb
//!   input.rs  picking and every pointer gesture
//!   view.rs   the armature as drawn
//!   mod.rs    the tool's state, its undo history and the live skin
//!
//! The rig is a scaffold, not a scene object: it keeps its own undo history
//! and is not written to save files. Make Skin is where the work becomes
//! real geometry.

pub mod frame;
mod gizmo;
mod input;
pub mod pose;
pub mod rig;
pub mod skin;
#[cfg(test)]
mod tests;
pub mod view;

use glam::{Quat, Vec2, Vec3};

use crate::camera::View;
use crate::mesh::bvh::Aabb;
pub use frame::{Body, Frame};
use rig::{Node, Rig};
use skin::{Quality, Skin, SkinSettings};

pub const MIN_NODE_RADIUS: f32 = 0.005;
pub const MAX_NODE_RADIUS: f32 = 5.0;

const MAX_UNDO_STEPS: usize = 128;
/// How long a streamed edit (a run of wheel notches, a slider being dragged)
/// stays open before it closes as one undo step.
const COALESCE_SECONDS: f64 = 0.4;
/// How long the rig must sit still before the settled skin replaces the
/// draft. Short enough to feel immediate on release, long enough that a pause
/// mid-gesture does not start a skin the next movement throws away.
const SETTLED_SKIN_DELAY: f64 = 0.18;
/// A draft that takes longer than this has its successor delayed in
/// proportion, so a huge rig drops to a lower skin frame rate instead of
/// dragging the whole viewport down.
const DRAFT_BUDGET_MS: f32 = 12.0;
const NOTE_SECONDS: f64 = 4.0;

/// What a drag on the armature does. Everything that works in every mode
/// (wheel to resize, right-click to delete, click to select) lives outside
/// this, so switching mode only ever changes what a drag does.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditMode {
    /// Drag off a sphere to grow a child that follows the cursor; drag a
    /// link to insert a sphere there and place it in the same motion.
    Draw,
    /// Drag a sphere to move it with everything below it. Drag a link to
    /// pose the limb.
    Move,
    /// Drag to resize a sphere.
    Scale,
    /// Swing a sphere and everything below it about its parent joint.
    Rotate,
}

impl EditMode {
    pub const ALL: [EditMode; 4] = [EditMode::Draw, EditMode::Move, EditMode::Scale, EditMode::Rotate];

    pub fn name(self) -> &'static str {
        match self {
            EditMode::Draw => "Draw",
            EditMode::Move => "Move",
            EditMode::Scale => "Scale",
            EditMode::Rotate => "Rotate",
        }
    }

    pub fn hotkey(self) -> char {
        match self {
            EditMode::Draw => '1',
            EditMode::Move => '2',
            EditMode::Scale => '3',
            EditMode::Rotate => '4',
        }
    }

    pub fn icon(self) -> &'static str {
        match self {
            EditMode::Draw => "tool_sspheres",
            EditMode::Move => "brush_move",
            EditMode::Scale => "prim_sphere",
            EditMode::Rotate => "redo",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            EditMode::Draw => {
                "Drag off a sphere to pull a new one out of it. With a sphere selected, click empty space to add the next one of its chain. Drag a link to add a joint and bend the limb there."
            }
            EditMode::Move => {
                "Drag a sphere to move it with everything below it; Shift moves just that sphere. Drag a link to pose the limb: it bends at the joint above, keeping every length. Drag an arrow of the selected sphere to move it straight along one axis."
            }
            EditMode::Scale => {
                "Drag right or up to grow a sphere, left or down to shrink it. Shift scales its whole branch."
            }
            EditMode::Rotate => {
                "Swing a sphere and everything below it around its parent joint. Shift twists the branch around its own bone."
            }
        }
    }
}

/// Where a ray met a model: the point, the surface normal there, and the
/// object that was hit.
#[derive(Clone, Copy, Debug)]
pub struct Hit {
    pub point: Vec3,
    pub normal: Vec3,
    pub body: Body,
}

/// What a gesture needs from the app: the camera, where it is looking, a way
/// to find the model surface under the cursor, and the selected object. All
/// in world space; the gestures move it into the rig's own space themselves
/// (see `SSpheres::in_rig`).
pub struct Ctx<'a> {
    pub view: &'a View,
    /// The camera's orbit pivot: where the artist is already looking.
    pub pivot: Vec3,
    /// Nearest model surface along a ray (origin, direction).
    pub surface: &'a dyn Fn(Vec3, Vec3) -> Option<Hit>,
    /// The selected object. A rig started in the open stands on it, so the
    /// two share a mirror plane instead of each having one of its own.
    pub body: Option<Body>,
}

/// What a press on the viewport turned out to be.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Press {
    /// The tool owns the pointer until release.
    Drag,
    /// Dealt with on the spot (a delete); there is nothing to drag.
    Done,
    /// Nothing of the rig is under the cursor: the camera may have the drag.
    /// If the pointer comes up again without moving, it was a click on empty
    /// space.
    Empty,
}

struct UndoStep {
    rig: Vec<Node>,
    selection: Option<usize>,
    /// Symmetry is part of the rig's shape (a derived reflection appears and
    /// disappears with it), so it is restored with the nodes.
    symmetry: bool,
    /// Where the rig stood, and the object it was attached to as that object
    /// stood then. Kept as a pair: restored together, the next
    /// [`SSpheres::follow`] carries the rig to wherever the object is now.
    frame: Frame,
    attached: Option<Body>,
    label: &'static str,
}

/// An undo step that has been opened but not closed yet.
struct Pending {
    step: UndoStep,
    version: u64,
    seat_version: u64,
    /// For a streamed edit: when it closes if nothing extends it.
    commit_at: Option<f64>,
}

/// A finished skin and where it goes in the scene.
pub struct MadeSkin {
    /// Measured from its own middle, which is where the new object's pivot
    /// goes.
    pub skin: Skin,
    /// The new object's place, turn and size.
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

/// What a skin was built from.
type SkinKey = (u64, SkinSettings, bool);

#[derive(Default)]
struct LiveSkin {
    skin: Option<Skin>,
    /// Bumped whenever `skin` is replaced, so the renderer re-uploads it.
    generation: u64,
    error: Option<String>,
    is_draft: bool,
    built: Option<SkinKey>,
    attempt_settled: bool,
    observed: Option<SkinKey>,
    last_change: f64,
    next_draft_at: f64,
}

pub struct SSpheres {
    pub rig: Rig,
    pub selected: Option<usize>,
    pub hovered: Option<usize>,
    hovered_link: Option<usize>,
    mode: EditMode,
    /// Mirror across the rig's own x = 0 plane. On by default: creatures and
    /// characters are overwhelmingly bilateral, and building one side twice
    /// is the tedium this tool removes.
    symmetry: bool,
    /// Where the rig stands in the world. Everything in `rig` is measured in
    /// it, and its x = 0 plane is the mirror plane.
    frame: Frame,
    /// The object the rig follows, as it stood when the rig last caught up
    /// with it (see `frame.rs`).
    attached: Option<Body>,
    /// Bumped whenever the rig is attached, detached or re-seated on
    /// purpose, so that counts as an edit even when no sphere changed.
    /// Following the object does not bump it.
    seat_version: u64,
    /// Radius of the sphere most recently placed or resized; 0 until there is
    /// one. Every new sphere starts at this size, so after sizing one sphere
    /// the rest of a limb comes out matching it.
    last_radius: f32,
    /// Size of the first sphere of a rig in world units, set by the app from
    /// what is already in the scene so it is neither a speck nor a planet
    /// beside it.
    pub default_radius: f32,
    /// Draw the skin as a see-through shell over the armature while editing.
    pub show_skin: bool,
    /// Solid skin with the armature hidden, for judging the actual shape.
    /// Editing still works: picking is against the spheres inside the skin.
    pub preview: bool,
    /// Leave the spheres in place after Make Skin, so the shape can be
    /// changed and skinned again.
    pub keep_rig: bool,
    /// Rest the first sphere on the model surface under the cursor.
    pub snap_to_surface: bool,
    /// The scene object the last Make Skin produced, while its spheres are
    /// still here to be changed. The live skin stands in for it while the
    /// tool is up, and the next Make Skin supersedes it.
    pub skin_object: Option<u64>,
    /// The spheres as Make Skin last used them: the rig's version and whether
    /// it was mirrored. While they are still that, and the object it made is
    /// still in the scene, the shape is in the scene and the spheres are only
    /// what it was made from (see [`Self::unskinned`]).
    skinned: Option<(u64, bool)>,
    pub settings: SkinSettings,
    drag: input::Drag,
    /// Press position of a press on empty space, until the pointer moves away.
    tap: Option<Vec2>,
    /// The sphere a click would place: centre and radius.
    ghost: Option<(Vec3, f32)>,
    /// Hold-S sizing of the next sphere: where S went down, and the size then.
    gauge: Option<(Vec2, f32)>,
    undo: Vec<UndoStep>,
    redo: Vec<UndoStep>,
    pending: Option<Pending>,
    live: LiveSkin,
    /// One-off result of an action, shown on the status line for a while.
    note: Option<(String, f64)>,
    /// Seconds since the app started, as of the last [`Self::tick`].
    now: f64,
}

impl Default for SSpheres {
    fn default() -> Self {
        Self {
            rig: Rig::default(),
            selected: None,
            hovered: None,
            hovered_link: None,
            mode: EditMode::Draw,
            symmetry: true,
            frame: Frame::IDENTITY,
            attached: None,
            seat_version: 0,
            last_radius: 0.0,
            default_radius: 0.25,
            show_skin: true,
            preview: false,
            keep_rig: true,
            snap_to_surface: true,
            skin_object: None,
            skinned: None,
            settings: SkinSettings::default(),
            drag: input::Drag::default(),
            tap: None,
            ghost: None,
            gauge: None,
            undo: Vec::new(),
            redo: Vec::new(),
            pending: None,
            live: LiveSkin::default(),
            note: None,
            now: 0.0,
        }
    }
}

impl SSpheres {
    pub fn mode(&self) -> EditMode {
        self.mode
    }

    pub fn set_mode(&mut self, mode: EditMode) {
        if self.mode != mode {
            self.end_drag();
            self.mode = mode;
        }
    }

    pub fn symmetry(&self) -> bool {
        self.symmetry
    }

    /// Advances the clock and closes a streamed edit whose window has run
    /// out (but never one belonging to a drag, which can sit still longer).
    pub fn tick(&mut self, time: f64) {
        self.now = time;
        let due = self.pending.as_ref().and_then(|p| p.commit_at).is_some_and(|at| time >= at);
        if due && !self.is_dragging() {
            self.commit_edit();
        }
        if self.note.as_ref().is_some_and(|(_, until)| time >= *until) {
            self.note = None;
        }
    }

    /// Puts the tool away: ends any gesture and clears what only makes sense
    /// while the pointer is over the viewport.
    pub fn leave(&mut self) {
        self.end_drag();
        self.commit_edit();
        self.tap = None;
        self.gauge = None;
        self.ghost = None;
        self.hovered = None;
        self.hovered_link = None;
    }

    /// Shows a one-off result on the status line for a few seconds.
    pub fn say(&mut self, text: impl Into<String>) {
        self.note = Some((text.into(), self.now + NOTE_SECONDS));
    }

    pub fn note(&self) -> Option<&str> {
        self.note.as_ref().map(|(text, _)| text.as_str())
    }

    /// Bounds of the rig as drawn, in the world.
    pub fn bounds(&self) -> Aabb {
        let mut bounds = Aabb::EMPTY;
        for sphere in self.rig.geometry(self.symmetry).0 {
            let centre = self.frame.to_world(sphere.centre);
            let reach = Vec3::splat(sphere.radius * self.frame.scale);
            bounds.grow(centre - reach);
            bounds.grow(centre + reach);
        }
        bounds
    }

    // ---- rig operations ---------------------------------------------------

    /// Toggles symmetry without ever making spheres vanish or double up, and
    /// says what happened.
    ///
    /// Off bakes the derived reflection into real spheres, or half the
    /// blockout would disappear with the toggle. On folds exact reflections
    /// back up, so off-then-on with no edits in between changes nothing.
    /// Either way it is one undo step.
    pub fn set_symmetry(&mut self, on: bool) -> String {
        if on == self.symmetry {
            return if on { "Symmetry is already on." } else { "Symmetry is already off." }.into();
        }
        self.end_drag();
        self.commit_edit();
        self.begin_edit(if on { "Symmetry On" } else { "Symmetry Off" });
        let report = if on {
            let folded = self.rig.merge_mirror_pairs();
            self.rig.snap_central_to_axis();
            self.symmetry = true;
            if folded > 0 {
                format!("Symmetry on. Folded {folded} mirrored sphere(s) back into one half.")
            } else {
                "Symmetry on. The rig now mirrors across the red plane.".into()
            }
        } else {
            let baked = self.rig.bake_symmetry();
            self.symmetry = false;
            if baked > 0 {
                format!("Symmetry off. The mirrored half is now {baked} real sphere(s) you can edit separately.")
            } else {
                "Symmetry off.".into()
            }
        };
        self.selected = self.selected.filter(|&s| self.rig.is_alive(s));
        self.commit_edit();
        report
    }

    /// Deletes a sphere and everything below it. Under symmetry the
    /// reflection goes too: it was never a separate sphere.
    pub fn delete_node(&mut self, index: usize) {
        if !self.rig.is_alive(index) {
            return;
        }
        self.begin_edit("Delete Sphere");
        self.rig.remove(index);
        self.selected = self.selected.filter(|&s| self.rig.is_alive(s));
        self.hovered = self.hovered.filter(|&s| self.rig.is_alive(s));
        self.commit_edit();
    }

    pub fn delete_selected(&mut self) -> bool {
        match self.selected.filter(|&s| self.rig.is_alive(s)) {
            Some(node) => {
                self.delete_node(node);
                true
            }
            None => false,
        }
    }

    /// Throws the whole blockout away. Undoable: it is the most destructive
    /// thing in the tool.
    pub fn clear(&mut self) {
        if self.rig.is_empty() {
            return;
        }
        self.end_drag();
        self.begin_edit("Clear Spheres");
        self.clear_rig();
        self.commit_edit();
    }

    fn clear_rig(&mut self) {
        self.drag = input::Drag::default();
        self.rig.clear();
        // A fresh blockout is not the one that object was made from.
        self.skin_object = None;
        self.selected = None;
        self.hovered = None;
        self.hovered_link = None;
    }

    /// Radius of the selected sphere, for the panel's slider.
    pub fn selected_radius(&self) -> Option<f32> {
        self.selected.and_then(|s| self.rig.get(s)).map(|n| n.radius)
    }

    pub fn set_selected_radius(&mut self, radius: f32) {
        if let Some(node) = self.selected.filter(|&s| self.rig.is_alive(s)) {
            // Streamed: a slider fires continuously while dragged.
            self.begin_stream_edit("Resize Sphere");
            self.set_node_radius(node, radius);
        }
    }

    /// Resizes a sphere as a direct edit, and remembers the size for the
    /// next new sphere.
    fn set_node_radius(&mut self, node: usize, radius: f32) {
        let radius = radius.clamp(MIN_NODE_RADIUS, MAX_NODE_RADIUS);
        self.rig.set_radius(node, radius);
        self.last_radius = radius;
    }

    /// The size the next sphere starts at (what the ghost shows): the last
    /// one placed or resized, else one sized off the rig or the scene.
    pub fn next_radius(&self) -> f32 {
        let radius = if self.last_radius > 0.0 {
            self.last_radius
        } else if !self.rig.is_empty() {
            self.rig.mean_radius()
        } else {
            // Sized off the scene in world units; radii are the rig's own.
            self.default_radius / self.frame.scale
        };
        radius.clamp(MIN_NODE_RADIUS, MAX_NODE_RADIUS)
    }

    /// Not a rig edit, so not an undo step.
    pub fn set_next_radius(&mut self, radius: f32) {
        self.last_radius = radius.clamp(MIN_NODE_RADIUS, MAX_NODE_RADIUS);
    }

    fn snap_if_symmetric(&self, position: Vec3, radius: f32) -> Vec3 {
        if self.symmetry { rig::snap_to_axis(position, radius) } else { position }
    }

    // ---- undo -------------------------------------------------------------

    /// Snapshotted and actually changed, but not closed yet: a drag in
    /// flight, or a resize in its window. To the artist it has happened.
    fn has_open_edit(&self) -> bool {
        self.pending.as_ref().is_some_and(|p| {
            self.rig.version() != p.version
                || self.symmetry != p.step.symmetry
                || self.seat_version != p.seat_version
        })
    }

    /// Everything an undo step puts back, as it is now.
    fn snapshot(&self, label: &'static str) -> UndoStep {
        UndoStep {
            rig: self.rig.snapshot(),
            selection: self.selected,
            symmetry: self.symmetry,
            frame: self.frame,
            attached: self.attached,
            label,
        }
    }

    fn restore(&mut self, step: &UndoStep) {
        self.rig.restore(&step.rig);
        self.symmetry = step.symmetry;
        self.frame = step.frame;
        self.attached = step.attached;
        // Indices survive a restore, but the stored selection may be a
        // sphere that only exists in the other direction.
        self.selected = step.selection.filter(|&s| self.rig.is_alive(s));
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty() || self.has_open_edit()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// What the next undo would reverse, and how many steps are held.
    pub fn undo_summary(&self) -> Option<(&'static str, usize)> {
        let open = self.has_open_edit();
        let label = if open { self.pending.as_ref().map(|p| p.step.label) } else { self.undo.last().map(|s| s.label) };
        label.map(|label| (label, self.undo.len() + open as usize))
    }

    fn open_edit(&mut self, label: &'static str, commit_at: Option<f64>) {
        if let Some(pending) = &mut self.pending {
            // A streamed edit left open must not absorb a discrete one that
            // follows inside its window: a drag would share the resize's
            // snapshot, and cancelling the drag would revert the resize too.
            if pending.commit_at.is_none() || commit_at.is_some() {
                if commit_at.is_some() {
                    pending.commit_at = commit_at;
                }
                return;
            }
            self.commit_edit();
        }
        self.pending = Some(Pending {
            step: self.snapshot(label),
            version: self.rig.version(),
            seat_version: self.seat_version,
            commit_at,
        });
    }

    /// Opens an undo step: snapshots now, kept by [`Self::commit_edit`] only
    /// if the rig actually changed in between. That makes it safe to open on
    /// every press, most of which are selection clicks. A nested call extends
    /// the open step.
    fn begin_edit(&mut self, label: &'static str) {
        self.open_edit(label, None);
    }

    /// Opens (or extends) a step that closes by itself once the edits stop
    /// arriving, so a run of them is one undo.
    fn begin_stream_edit(&mut self, label: &'static str) {
        self.open_edit(label, Some(self.now + COALESCE_SECONDS));
    }

    fn commit_edit(&mut self) {
        let changed = self.has_open_edit();
        let Some(pending) = self.pending.take() else { return };
        if changed {
            self.undo.push(pending.step);
            if self.undo.len() > MAX_UNDO_STEPS {
                self.undo.remove(0);
            }
            self.redo.clear();
        }
    }

    /// Throws the open step away and puts the rig back to its snapshot.
    fn cancel_edit(&mut self) {
        let changed = self.has_open_edit();
        let Some(pending) = self.pending.take() else { return };
        if changed {
            self.restore(&pending.step);
        }
    }

    pub fn undo(&mut self) -> bool {
        // The operation just finished is what gets reversed, not the one
        // before it.
        self.end_drag();
        self.commit_edit();
        self.take_step(false)
    }

    pub fn redo(&mut self) -> bool {
        self.end_drag();
        // An open edit is newer than anything on the redo stack, and
        // committing it clears that stack.
        self.commit_edit();
        self.take_step(true)
    }

    fn take_step(&mut self, redo: bool) -> bool {
        let Some(step) = (if redo { self.redo.pop() } else { self.undo.pop() }) else { return false };
        let here = self.snapshot(step.label);
        if redo { self.undo.push(here) } else { self.redo.push(here) }
        self.restore(&step);
        self.hovered = None;
        self.hovered_link = None;
        true
    }

    // ---- the live skin ----------------------------------------------------

    fn skin_wanted(&self) -> bool {
        (self.show_skin || self.preview) && !self.rig.is_empty()
    }

    fn skin_key(&self) -> SkinKey {
        (self.rig.version(), self.settings, self.symmetry)
    }

    /// Keeps the skin in step with the rig. Three tiers, each paying only for
    /// what its moment needs: a draft while the artist drags, a settled skin
    /// once the rig has been still for a moment (or at once, with
    /// `settle_now`), and the full one only for Make Skin.
    pub fn refresh_skin(&mut self, settle_now: bool) {
        if !self.skin_wanted() {
            if self.rig.is_empty() && (self.live.skin.is_some() || self.live.error.is_some()) {
                self.live.skin = None;
                self.live.error = None;
                self.live.generation += 1;
            }
            return;
        }
        let key = self.skin_key();
        if self.live.observed != Some(key) {
            self.live.observed = Some(key);
            self.live.last_change = self.now;
        }
        let current = self.live.built == Some(key);
        let settled =
            settle_now || (!self.is_dragging() && self.now - self.live.last_change >= SETTLED_SKIN_DELAY);
        if settled && (!current || !self.live.attempt_settled) {
            self.run_skin(Quality::Preview, key);
        } else if !settled && !current && self.now >= self.live.next_draft_at {
            self.run_skin(Quality::Draft, key);
        }
    }

    fn run_skin(&mut self, quality: Quality, key: SkinKey) {
        let result = skin::skin(&self.rig, self.symmetry, &self.settings, quality);
        // Recorded even on failure, so a rig that cannot skin is not retried
        // every frame.
        self.live.built = Some(key);
        self.live.attempt_settled = quality != Quality::Draft;
        let mut ms = 0.0;
        match result {
            Ok(skin) => {
                ms = skin.stats.ms;
                self.live.is_draft = quality == Quality::Draft;
                self.live.skin = Some(skin);
                self.live.generation += 1;
                self.live.error = None;
            }
            Err(e) => self.live.error = Some(e),
        }
        if quality == Quality::Draft {
            let wait = if ms > DRAFT_BUDGET_MS { ms as f64 * 0.0015 } else { 0.0 };
            self.live.next_draft_at = self.now + wait;
        }
    }

    /// True while a frame is owed without new input: the settled skin has
    /// not landed yet, or a streamed edit is waiting to close.
    pub fn is_settling(&self) -> bool {
        let skin_due =
            self.skin_wanted() && (self.live.built != Some(self.skin_key()) || !self.live.attempt_settled);
        skin_due || self.pending.as_ref().is_some_and(|p| p.commit_at.is_some())
    }

    /// The skin to draw this frame, with a number that changes whenever it
    /// does.
    pub fn skin(&self) -> Option<(&Skin, u64)> {
        if !self.skin_wanted() {
            return None;
        }
        self.live.skin.as_ref().map(|s| (s, self.live.generation))
    }

    /// How far the see-through shell is drawn toward the eye. The skin lies
    /// exactly on a sphere wherever one shows through it, and its flat
    /// triangles dip a sliver inside (at most about cell squared over four
    /// radii); lifted by a few times that, the shell cleanly wins against
    /// the spheres under it instead of flickering with them.
    pub fn shell_lift(&self) -> f32 {
        let Some(skin) = &self.live.skin else { return 0.0 };
        let cell = skin.stats.cell;
        // Worked out in the rig's own units, handed over in the world's.
        (cell * cell / self.rig.min_radius().max(1e-6)).min(cell) * self.frame.scale
    }

    pub fn skin_is_draft(&self) -> bool {
        self.live.is_draft
    }

    pub fn skin_error(&self) -> Option<&str> {
        self.live.error.as_deref()
    }

    /// Roughly how many triangles Make Skin will produce, scaled up from the
    /// skin on screen.
    pub fn final_triangles(&self) -> Option<usize> {
        let shown = self.live.skin.as_ref()?;
        let wanted = skin::final_resolution(&self.rig, self.symmetry, &self.settings) as f64;
        let ratio = wanted / shown.stats.resolution.max(1) as f64;
        Some((shown.stats.triangles as f64 * ratio * ratio) as usize)
    }

    /// How many spheres would be lost for good if the app closed now: the
    /// spheres on screen, mirrored twins included, when they are work that is
    /// in no object. `skin_in_scene` says whether the object the last Make
    /// Skin made is still in the scene.
    ///
    /// None for a single sphere, which is one click (New SSphere Rig places
    /// it by itself). None once Make Skin has used the spheres as they stand,
    /// for as long as that object is there: the shape is in the scene then,
    /// and in its file. Changed since, or with the object undone or deleted,
    /// they are the only place the shape is. Undo back to the skinned shape
    /// still counts, which errs on the side of asking.
    pub fn unskinned(&self, skin_in_scene: bool) -> usize {
        let in_scene = skin_in_scene && self.skinned == Some((self.rig.version(), self.symmetry));
        if self.rig.alive_count() < 2 || in_scene {
            return 0;
        }
        self.rig.geometry(self.symmetry).0.len()
    }

    /// The spheres as a question about closing names them, or `None` when
    /// closing would lose nothing of them (see [`Self::unskinned`]). A scene
    /// opened in place of this one does not come here: the spheres stand in
    /// the world and stay, let go of the object they were attached to.
    pub fn at_risk(&self, skin_in_scene: bool) -> Option<String> {
        match self.unskinned(skin_in_scene) {
            0 => None,
            count => Some(format!("an SSphere rig of {count} spheres (Make Skin first)")),
        }
    }

    /// Skins the rig at full detail, for turning into a real mesh, and says
    /// where in the scene it goes. Unless the spheres are being kept, they
    /// are cleared as an undo step of their own, so Undo here brings them
    /// back.
    pub fn make_skin(&mut self) -> Result<MadeSkin, String> {
        self.end_drag();
        self.commit_edit();
        let mut skin = skin::skin(&self.rig, self.symmetry, &self.settings, Quality::Final)?;

        // The new object's pivot goes to the middle of the skin, not to
        // wherever the rig's origin happens to be: an object whose shape
        // sits far from its pivot turns, resizes and mirrors about the wrong
        // place. Under symmetry the pivot stays on the mirror plane, so the
        // object mirrors across the very plane the rig was built about.
        let mut bounds = Aabb::EMPTY;
        for &p in &skin.positions {
            bounds.grow(p);
        }
        let mut centre = bounds.center();
        if self.symmetry {
            centre.x = 0.0;
        }
        for p in &mut skin.positions {
            *p -= centre;
        }
        let made = MadeSkin {
            skin,
            position: self.frame.to_world(centre),
            rotation: self.frame.rotation,
            scale: Vec3::splat(self.frame.scale),
        };
        self.skinned = Some((self.rig.version(), self.symmetry));

        if !self.keep_rig {
            self.begin_edit("Make Skin");
            self.clear_rig();
            self.commit_edit();
        }
        self.preview = false;
        Ok(made)
    }
}
