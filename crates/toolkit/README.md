# How to write a tool

A tool is a crate that depends on `sculpt-toolkit` (plus `sculpt-geom`, `glam`,
`egui` and its own geometry crates) and implements one trait. It never sees an
app type. The app hosts any number of them: each gets a button, the pointer,
wheel and keys while it is up, a panel, and a place to draw.

Two tools to read before writing one:

- `crates/toolkit/src/example.rs`: **Studs**, about 200 lines, does one of
  everything (presses, own undo, ghost and instanced previews, the gizmo on
  a stud, a background job, a commit, a panel, a wheel, a demo, tests).
  `--demo-tool studs` shows it.
- `crates/measure`: **Measure**, a real port (rulers, labels, a card in the
  Scene panel, typed sizes as undo steps).

The tools this interface is for: Measure, Tube, Lathe, Array, Keys, Mask
Extract, Boolean, and the region tools (box / lasso / line: hide, mask, trim,
split).

## The trait

```rust
pub trait Tool {
    fn info(&self) -> Info;                                   // the only one you must write
    fn enter(&mut self, cx: &mut Ctx) {}                      // brought up
    fn leave(&mut self, cx: &mut Ctx) {}                      // put away: end any gesture
    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool { false }
    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {}     // once a frame: update, say what to draw
    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {}   // left panel while up
    fn card(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {}    // optional card in the Scene panel, always there
    fn radial(&self) -> Vec<RadialItem> { vec![] }            // hold-Space wheel tiles
    fn can_undo(&self, redo: bool) -> bool { false }          // own history, for the Undo / Redo buttons
    fn gizmo(&self) -> Option<GizmoTarget> { None }           // something of yours for the transform gizmo to sit on
    fn demo(&mut self, cx: &mut Ctx, what: &str) {}           // --demo-tool
}
```

`Info { id, name, icon, tip, hotkey }`: `id` is lower-case and never changes
(`--demo-tool` uses it); `icon` names a file in `assets/icons`.

`input` gets one `Event` at a time. Where the pointer is and what is held is in
`cx`. The returned flag means "that was mine":

| Event | When | Return true to |
| --- | --- | --- |
| `Press(Button)` | A button went down on the bare viewport (never on a panel, never with Alt) | take the drag. False passes it on: a left press then orbits the camera |
| `Drag` | Every pointer sample of a drag you took (`cx.pointer`, `cx.pressure`) | (ignored) |
| `Release` | The drag you took ended | (ignored) |
| `Click(Button)` | A press you passed on came up without moving | say it meant something (otherwise two of them select the object under the pointer, or frame the model) |
| `Wheel(notches)` | Wheel over the viewport | use it; false zooms |
| `KeyDown(Key)` / `KeyUp(Key)` | A key, no Ctrl. `Key` is `egui::Key` | keep it from the app. Esc you do not take puts the tool away |
| `Undo` / `Redo` | Z / Shift+Z | step your own history; false goes to the scene's |
| `Radial(index)` | A wheel tile was picked | (ignored) |
| `Gizmo(Placement, GizmoPhase)` | A handle of the gizmo on your target is being dragged (see "The transform gizmo" below) | (ignored) |

End the `match` with `_ => false`; more events may be added.

Rules the host keeps, so you do not have to:

- **A drag on empty background orbits**, as everywhere in the app. Pass a press
  that lands on nothing of yours. If a click on nothing should still do
  something (start a ruler in mid-air), do it in `Click`.
- Alt-drag, the middle button, and the wheel you do not use stay the camera's.
- While a tool is up the brush keys do nothing. Z, G (transform gizmo), Tab,
  F11, F F (frame) and Ctrl shortcuts stay the app's unless you take them.
- `frame` runs for **every** tool every frame, up or not (`cx.active` says), so
  a tool can land a job or keep live copies on screen after the user has moved
  on. It is not called during a brush stroke; what you drew last stays up.
- Hotkeys follow the Unity app: **T Tube, J Keys, K Split, L Measure, Y
  Array**. A hotkey brings the tool up from Sculpt, Transform or another hosted
  tool, and puts it away again. Keys the app uses itself (Z S F D M X R G,
  digits, Space, Tab, Esc, Delete, F11) cannot be hotkeys.

## The context

`cx` reads as plain facts (`cx.view`, `cx.pointer`...) and has the methods that
change things. Nothing is kept between calls; hold ids, not references.

| Read | |
| --- | --- |
| `cx.view` | The camera: `ray(px)`, `project(world)`, `depth_of`, `world_per_pixel(depth)`, `on_facing_plane(px, through)`, `right / up / forward`, `size`, `pixels_per_point` |
| `cx.pointer`, `cx.hovering`, `cx.pressure`, `cx.mods` | Pointer in pixels; whether it is free over the viewport (show ghosts and hover only then); pen pressure; shift / ctrl / alt |
| `cx.time`, `cx.dt`, `cx.active` | Seconds; whether this tool is the one up |
| `cx.mirror`, `cx.unit`, `cx.tips`, `cx.clean_view` | Brush mirror axes; mm or inches (`sculpt_geom::units` formats and parses); tooltips on; panels hidden (draw no handles) |
| `cx.radial_count`, `cx.radial_axis`, `cx.symmetry()` | Radial symmetry (repeats round an axis; 1 is off); every copy a stroke is made as, mirrors and turns together, as a `SymmetryGroup` |
| `cx.scene()` | `objects()`, `active()`, `selected()` (every selected object, the active one first), `info(id)` (name, visible, placement, live mirror, mesh id, counts), `mesh(id)` (positions, normals, indices, mask, hidden-face flags **as slices**, object space), `raycast(&ray)`, `raycast_object(id, &ray)`, `raycast_placed(id, &model, &ray)`, `bounds(id)` |
| `cx.pick(px)` | The visible surface under a pixel: object, position, normal, distance |

Screen positions are physical pixels, y down. Sizes meant in interface points
(pick radii) want `* cx.view.pixels_per_point`.

Mesh slices are the app's own arrays: reading costs nothing, at any size. The
app sorts vertices its own way, so do not expect the order you sent. Copy
(`MeshView::to_tri_mesh`) only when starting a job.

| Change | |
| --- | --- |
| `cx.commit(label, commands)` | Change the scene: **one undo step**. Returns a `Receipt` with the ids of what was added |
| `cx.spawn(label, work)` | Run work on a background thread; returns a `Job<T>` |
| `cx.status(text, ok)`, `cx.toast(text)` | The status line of the Scene panel; the fading word over the viewport |
| `cx.finish()`, `cx.start()` | Put this tool away; bring it up (for a button on its card) |
| `cx.frame_view(aabb)`, `cx.set_unit(u)`, `cx.set_mirror(axes)`, `cx.repaint()` | Camera glide; app-wide unit; brush mirror; ask for another frame |
| `cx.set_radial(RadialChange { on, count, axis })` | The brushes' radial symmetry: on or off, how many repeats, which axis (in the selected object's own space). What is left `None` stays as the Symmetry card has it. The Lathe turns it on round the object it made |
| `cx.icon(name)` | An icon for `ButtonStyle::icon` / `theme::tile` |

## Commands and undo

```rust
let receipt = cx.commit("Split", vec![
    Command::Replace { object: target, mesh: kept.into() },      // instances follow
    Command::Add(NewObject::new("Piece", cut_off).placed(placement)),
    Command::Show { object: cutter, visible: false },
]);
let piece = receipt.added[0];
```

`Add`, `Replace`, `Place`, `Mirror`, `Show`, `Rename`, `Delete`, `Select`. They
run in order, at once; the scene read on the next line is the changed one.
However many are in one commit, **Z takes them all back together**. `Select`
alone leaves no undo step. `Mask { object, mask }` is the exception: a mask is
undone like a brush stroke, so each is a step of its own, applied after the
rest. So are the three commands for hidden faces, below.

**Live mirror copies.** An object can have live copies of itself across the
world's planes (the Mirror section of the Objects card). They are not objects:
each is the object's own mesh drawn again through a reflected matrix, so there
is nothing to keep in step. A tool meets them in three places:

```rust
let info = cx.scene().info(object)?;
info.mirror                          // Option<MirrorRepeater>: the axes (X = 1, Y = 2, Z = 4) and the centre
info.placements()                    // [(0, the object's matrix), (flip mask, the copy's matrix)..]
cx.commit("Mirror", vec![Command::Mirror { object, mirror: MirrorRepeater::new(1, Vec3::ZERO) }]);  // None takes it off
NewObject::new("Piece", mesh).mirrored(info.mirror)                 // a new object that starts with the same
```

A copy's matrix is the world reflection after the object's own, so for an odd
number of axes its determinant is negative and the mesh seen through it is
inside out. The app draws that right (`draw.copies` goes the way the mirror
copies themselves do); a tool that takes triangles out to world space has to
turn the faces back. Everything that is a question about the mesh, such as
which vertices a shape on screen covers, is answered by projecting the
object's own vertices through the copy's matrix: `sculpt_cut` takes the
matrix as it is. To find
which copy a click landed on, ask `cx.scene().raycast_placed(id, &model, &ray)`
for each place in `placements()` and keep the nearest hit; it takes any matrix,
so it picks a copy of the tool's own (one copy of an array) the same way.
A clone, an instance or a piece cut from a mirrored object is not mirrored
unless the tool says so with `.mirrored(..)`.

**Hidden faces.** Part of a mesh can be switched off: not drawn, not hit by the
pointer (`cx.pick`, `raycast`), not touched by brushes. Nothing is removed:
`MeshView::indices` and every per-vertex array keep the whole mesh, so vertex
and triangle numbers mean the same before and after.

```rust
let view = cx.scene().mesh(object)?;
view.hidden_triangles    // Option<&[bool]>, one per triangle in the order of `indices`; None = nothing hidden
view.hidden_vertices     // Option<&[bool]>, one per vertex: hidden only when every triangle on it is
view.visible_triangle_count()    is_triangle_hidden(t)    is_vertex_hidden(v)

cx.commit("Hide", vec![Command::HideFaces { object, triangles, hide: true }]);   // hide: false shows them again
cx.commit("Show All", vec![Command::ShowAllFaces { object }]);
cx.commit("Invert", vec![Command::InvertFaces { object }]);
```

`triangles` are triangle numbers (triangle `t` is `indices[3 * t..3 * t + 3]`).
The two flag slices are what `sculpt_cut::select::plan_hide` and
`mask_targets` take. Each of the three commands is one undo step and makes the
object the selected one. One that does nothing leaves no step and is counted in
`receipt.failed` (`receipt.changed` stays false): hiding what is already
hidden, showing or inverting when nothing is hidden, and **a hide that would
leave none of the object showing**, which the app refuses because an object
that has vanished gives no sign that it is only hidden. Check
`visible_triangle_count()` first if you want to say why.

There is no way to change the scene except a commit, so a tool cannot leave the
document or its history half-edited. Live feedback during a drag is a preview
(below), not a commit per mouse move; commit once on release.

**A tool's own history.** A scaffold edited in many small steps (points of a
tube, a lathe profile) keeps its own undo stack: answer `Event::Undo` / `Redo`
and `can_undo`. When that stack is empty the press falls through to the scene.

**Noticing that your commit was undone.** Object ids survive undo and redo, and
`info(id).mesh` changes when a mesh is replaced and changes back when that is
undone. Keep the ids from your commit; in `frame`, an added object that no
longer `exists`, or a mesh id that is back to the old one, means the user pressed
Z: hand the scaffold back.

## Drawing

Fill `draw` in `frame`. Flat shapes go over the viewport (under the panels):

```rust
draw.overlay.line(a, b, 2.0, color);          dashed(a, b, w, color, dash, gap)
draw.overlay.polyline(&points, 2.0, color, closed);
draw.overlay.dot(p, 4.0, fill, outline);      square(..)      fill(&convex, color)
draw.overlay.text(p, "12 studs");             label(Label { at, push, gap, runs, background })
draw.overlay.line_3d(&cx.view, a, b, w, color);   polyline_3d(..)
```

Positions in pixels (`cx.view.project`), widths, radii and text sizes in points.
A `Label` with `push` set is moved off its point until it clears it, whatever
size the text turns out to be.

Meshes go inside the viewport, depth-tested against the scene. Keep a `Preview`
in the tool; `set(tri_mesh)` only when it changes (each `set` is a new version,
and the GPU copy is remade only then):

```rust
draw.mesh(&self.preview, Style::solid(color));             // lit, opaque
draw.mesh(&self.preview, Style::shell(color));             // see-through with a rim
draw.instances(&self.peg, Style::solid(c).flat(), &mats);  // one mesh, many transforms
draw.copies(object, &world_matrices);  // a scene object drawn again, with the scene's own material
draw.hide(object);                     // leave a scene object out this frame (a preview stands in)
```

`Look::Unlit` draws the bare colour (sheets, cut planes); `.on_top()` ignores
depth; `.flat()` shades by face (sharp-edged shapes with shared vertices).

## The transform gizmo

The app's gizmo (arrows to move, rings to turn, cubes to resize) can sit on
something of the tool's own: a control point, one copy of an array, a joint.
Say what from `gizmo`, and take the drags from `input`:

```rust
fn gizmo(&self) -> Option<GizmoTarget> {
    let point = self.points.get(self.selected?)?;
    Some(GizmoTarget::point(point.position))            // arrows only
    // GizmoTarget::body(placement, radius)             // arrows, rings and cubes
}

fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
    match event {
        Event::Gizmo(placement, phase) => {
            if phase == GizmoPhase::Begin { self.before = Some(self.points.clone()); }   // for your undo step
            self.points[self.selected.unwrap()].position = placement.position;
            if phase == GizmoPhase::End { self.push_undo_step(); }                       // one step per drag
            true
        }
        ...
```

- `gizmo` is asked every frame while the tool is up, so answer from the
  tool's state as it is now; `None` puts the gizmo away. The gizmo sits at
  `placement.position`, turned to `placement.rotation`. Set `moves`, `turns`
  and `scales` for the handles you want; `point` and `body` are the two usual
  mixes.
- A press on a handle is the gizmo's: the tool gets no `Press` for it, and
  `cx.hovering` is false while the pointer is on a handle. Every drag is
  `Begin` (the placement you offered, nothing moved yet), a `Move` per
  pointer sample, and always an `End` with the last placement, also when the
  tool is put away mid-drag. A press let go without moving ends where it
  began: compare before recording a step.
- Each placement is worked out from where the target was at `Begin`, not
  added to the last one. Put the target exactly there; do not add it up.
- Only what a handle can change does change: a `point` never gets a new
  rotation or scale.
- The middle is yours. A press within `radius` of the target's position (at
  least a fingertip on screen) is an ordinary `Press`, so a point can still
  be grabbed and dragged freely on the overlay, with the arrows there for a
  straight pull along one axis. A target that scales has the white resize
  cube in the middle, which is the gizmo's.
- Several things selected in your tool: offer one target at their middle and
  move them all by the change in its placement.
- The gizmo is not undoable by the app for a tool's own target: it is your
  scaffold. If the target is a scene object, commit `Command::Place` at
  `End`.

In tests, `bench.gizmo(&mut tool, placement)` drags the target there as the
app would report it (`Begin`, two `Move`s, `End`).

## Background jobs

Anything that can take more than a frame or two (remesh, boolean, fusing).

```rust
let source = cx.scene().active_mesh().map(|m| m.to_tri_mesh());   // copy in: a job cannot borrow
self.job = Some(cx.spawn("Boolean", move |job| {
    job.progress("Voxelizing");
    if job.cancelled() { return Err("Cancelled.".into()); }
    let mesh = heavy(source)?;                 // Result<_, String>, rayon is fine
    Ok(job.bake(mesh, None))                   // builds the sculpt structures here, off the UI thread
}));
// in frame():
if let Some(result) = self.job.as_mut().and_then(Job::poll) {
    self.job = None;
    match result {
        Ok(mesh) => { cx.commit("Boolean", vec![Command::Replace { object, mesh }]); }
        Err(e) => cx.status(e, false),
    }
}
```

The app keeps drawing while a job runs and shows each job's name, progress
text and a Cancel button. Dropping the `Job` cancels it. `job.wait()` blocks:
for `demo` and tests only. Check in `frame` that the object you meant to change
still `exists` before committing a result that took seconds.

## Panels

Draw with `sculpt_toolkit::theme`, never egui's stock widgets. The app's own
panels use the same module (`src/ui/theme.rs` re-exports it), so they match.

`category(ui, title, accent, summary, default_open, |ui| ..)` is the card
everything sits in. Inside: `heading`, `hint`, `line(ui, text, OK | ERROR |
HINT)`, `button(ui, width, text, ButtonStyle { icon, active, badge, height })`,
`tile`, `slider(ui, label, &mut f32, range, |v| text)`, `checkbox`,
`text_field` / `labelled_field`, `shortcuts(ui, &[(key, what)])`,
`row_width(ui, n, gap)` for a row of equal buttons, `tip(response, cx.tips,
text)`, `count(n)`, and the colours (`ACCENT`, `TEXT`, `HINT`, `OK`, `ERROR`,
`MASK`, `AXIS`...). `open_category` opens a card from code.

`panel` replaces the brush settings while the tool is up. `card` is for the
few tools with something worth having to hand in any mode.

## Registering

1. `pub fn tool() -> Box<dyn sculpt_toolkit::Tool>` in your crate.
2. One line in `registry()` in `src/app/tools.rs`: `sculpt_yours::tool(),`.
3. The crate as a dependency of the app in the root `Cargo.toml`. Do not edit
   it yourself (see `crates/README.md`): ask.
4. Icons must be listed in `ICONS` in `src/ui/assets.rs`. The tool, boolean
   and region icons from the Unity project are already there.

That is all the app needs. Buttons appear in registry order, three to a row,
under Sculpt / Transform / SSpheres.

## Testing and `--demo-tool`

`sculpt_toolkit::testing::Bench` is a headless app: a scene with real undo, a
camera, and the host's press / drag / click routing.

```rust
let mut bench = Bench::new();                       // camera on +z looking at the origin, 800 x 600
let cube = bench.add("Cube", testing::cube(0.5));
let mut tool = MyTool::default();
bench.enter(&mut tool);
bench.drag(&mut tool, bench.px(Vec3::new(-0.4, 0.0, 0.5)), bench.px(Vec3::new(0.4, 0.0, 0.5)));
bench.env.mods.shift = true;                        // set the moment directly
let draw = bench.frame(&mut tool);                  // inspect draw.overlay.shapes, draw.previews()
assert!(bench.undo(&mut tool));                     // Z as the app routes it
bench.select(&[cube, other]);                       // several selected, the first active
bench.mirror(cube, 1);                              // live mirror across X (Y = 2, Z = 4); Scene::raycast meets the copies
let twin = bench.instance(cube, "Cube Instance", Placement::at(Vec3::X));   // another object showing the same mesh
bench.gizmo(&mut tool, Placement::at(Vec3::X));     // a drag of the gizmo on the tool's target
```

`cargo test -p sculpt-yours --target-dir target/w/yours`. A panel can be built
headless too (`crates/toolkit/tests/readme.rs` does it): call
`theme::install(&ctx)` first, or the bold face is missing.

Every tool implements `demo`: put the tool in a state worth a screenshot by
calling the same functions the gestures call, with `what` choosing a variant.

```
sculpt.exe --screenshot out.png --size 1600x960 --demo-tool measure
sculpt.exe --screenshot out.png --demo-tool measure:inches     # id:what
sculpt.exe --screenshot out.png --demo-tool studs:job          # a job in progress
sculpt.exe --demo-tool measure:play     # any tool: scripted presses, drags, wheel, Esc and its hotkey
                                        # through the window's handlers; prints what the host did
sculpt.exe --demo-tool measure:check    # any tool: every kind of command against the real document,
                                        # then undo and redo; prints the scene at each point
```

`play` and `check` are the host's own (in `src/app/tools.rs`); a tool does not
write them, and should not use those two words for its own variants.

## A complete minimal tool

```rust
use sculpt_toolkit::{theme, Button, Color32, Command, Ctx, Draw, Event, Info, Key, NewObject, Preview, Style, Tool};
use sculpt_toolkit::testing::cube;

#[derive(Default)]
pub struct Dropper { ghost: Preview, at: Option<glam::Mat4> }

pub fn tool() -> Box<dyn Tool> { Box::new(Dropper::default()) }

impl Tool for Dropper {
    fn info(&self) -> Info {
        Info { id: "dropper", name: "Dropper", icon: "prim_cube", tip: "Click the model to drop a cube on it.", hotkey: None }
    }
    fn enter(&mut self, _cx: &mut Ctx) {
        self.ghost.set(cube(0.05));
    }
    fn input(&mut self, cx: &mut Ctx, event: Event) -> bool {
        match event {
            Event::Press(Button::Left) => {
                let Some(hit) = cx.pick(cx.pointer) else { return false };      // not on the model: orbit
                let mut mesh = cube(0.05);
                mesh.transform(&glam::Mat4::from_translation(hit.position));
                cx.commit("Drop Cube", vec![Command::Add(NewObject::new("Cube", mesh).unselected())]);
                true
            }
            Event::KeyDown(Key::Enter) => { cx.finish(); true }
            _ => false,
        }
    }
    fn frame(&mut self, cx: &mut Ctx, draw: &mut Draw) {
        if !cx.active { return; }
        self.at = cx.pick(cx.pointer).filter(|_| cx.hovering).map(|hit| glam::Mat4::from_translation(hit.position));
        if let Some(at) = self.at {
            draw.instances(&self.ghost, Style::shell(Color32::from_rgba_unmultiplied(120, 170, 255, 110)), &[at]);
        }
    }
    fn panel(&mut self, ui: &mut egui::Ui, cx: &mut Ctx) {
        theme::category(ui, "Dropper", theme::ACCENT, "", true, |ui| {
            theme::hint(ui, "Click the model to drop a cube. Each is one undo step.");
            if theme::button(ui, ui.available_width(), "Done", Default::default()).clicked() { cx.finish(); }
        });
    }
}
```

## Not covered yet

- **Changing the selection of several objects.** `cx.scene().selected()`
  reads it (the active object first, then the others in the order they were
  added), so a tool that needs two operands (Boolean, Keys) takes the first
  two. There is no command that adds to it: `Command::Select` selects one
  object alone. The user builds it with Ctrl + click in the object list.
- **The gizmo on several targets of a tool's own, or with some axes left
  out.** One target at a time, all three axes; offer the middle of a set and
  spread the change yourself.
- **Hidden faces on a new or replaced mesh.** `Command::HideFaces` hides
  faces of an object that is already in the scene; `Add` and `Replace` take
  no flags, so a mesh arrives with everything showing.
- **Knowing a mesh was sculpted.** Mesh ids change on replace, not on a brush
  stroke. Anything cached from vertex positions has to be refreshed on a timer
  (Measure does) or when the tool comes up.
- **Previews in the scene's own material.** Preview meshes are lit clay or a
  shell; only `draw.copies` of a real object uses the scene material.
- **Tool state in `.sclp` files and in the scene's undo history.** A scaffold
  lives in the tool, is not saved, and has its own Z while the tool is up.
- **The Claude bridge** (`src/mcp.rs`) does not know hosted tools.

One compiler rule from `crates/README.md` worth repeating: rustc 1.99 miscompiles
`if x < 2 { 1 } else { x.min(MAX) }` on unsigned integers; limit counts and
indices with `x.clamp(lo, hi)`.
