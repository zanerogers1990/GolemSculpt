# Fix: Remesh must never silently mirror the model

**Priority: do this before the renderer work.** This bug has destroyed real work more than once.

## What happened

On 2026-10-17, between the 12:40 and 12:48 saves of `D:\EsauCordona\Base mesh folder\hollowedwiththickness.sclp`, the Body object (one door sculpted on one side) was remeshed with the object's Mirror X switched on. The remesh threw away the -X half and rebuilt it as a copy of the +X half. The Body came out symmetric with two doors and the work on the other side gone. Nothing said so, and the damage was only found after more saves had gone on top of it.

## Why it happens (confirmed in the code)

`src/app.rs`, `Action::Remesh`:

```rust
// Exactly symmetric across the object's own mirror planes.
let mirrors = self.doc.scene.active_symmetry().exact_mirrors();
match remesh::voxel_remesh_noted(&self.doc.mesh, res, mirrors) {
```

- `exact_mirrors()` (`src/scene/symmetry.rs`) returns the object's **brush** mirror settings whenever they're in the object's own space.
- `sculpt_volume::remesh` with a mirror axis on "keeps the half on the positive side of each plane and reflects it" (`crates/volume/src/remesh.rs`).
- So the brush symmetry toggle (the **X** key flips it) silently decides whether Remesh deletes half the model.
- Remesh is a single bare key (**R**), and its tooltip doesn't mention mirroring at all.

"Sculpt with mirrored brushes" and "replace one side of my model with the other" are different decisions. Today one toggle makes both.

## The fix

### 1. Remesh rebuilds the whole mesh as it is, by default
- `Action::Remesh` passes `[false; 3]` unless the user has explicitly asked for a mirrored rebuild (step 2). Brush symmetry no longer feeds into it at all.
- Leave the `sculpt-volume` crate and its mirror support as they are. This is a change of policy in the app, not in the kernel.

### 2. Mirrored remesh becomes its own explicit choice
- Add a **"Mirror result"** option to the Remesh card, per axis (X/Y/Z), **off by default**, separate from the Symmetry card's brush mirrors.
- Don't keep it on between remeshes or save it with the scene. It turns itself back off after each remesh, so it can never be left on and forgotten.
- While it's on, the button says what it'll do, for example **"Remesh + mirror X → X"**.

### 3. Ask before mirroring a model whose sides differ
Even with Mirror result on, run the existing symmetry check (`check_symmetry` in the Symmetry card's repairs) on the active mesh before rebuilding. If the halves differ beyond a small tolerance, ask through the existing question UI (`ui/question.rs`):

> "Body's two sides are different. A mirrored remesh will **replace the −X side with a copy of the +X side**. Remesh without mirroring / Mirror anyway / Cancel"

The default (Enter) is **Remesh without mirroring**.

### 4. Say what happened, every time
The status line after a remesh states whether it mirrored, e.g. `Remeshed to 5.5M triangles (not mirrored).` or `Remeshed to 5.5M triangles, −X side replaced by +X. Undo (Z) brings it back.`

### 5. Audit every other rebuild for the same trap
Grep the whole repo (not only `app.rs`) for anything that feeds brush symmetry into an operation that rebuilds or discards geometry: `exact_mirrors`, `mirror:` passed into `sculpt_volume`, booleans (the `exact_mirrors` doc comment says "a remesh, a boolean"), decimate, quad remesh, subdivision levels, the Blender link import. The same rule applies to each one: brush symmetry never decides it. Mirroring geometry only happens through actions that say so (Cut and Mirror, Make Symmetric, or the explicit Mirror result option).

### 6. Hotkey
Keep **R** for Remesh: once it no longer mirrors, it's no more dangerous than any other undoable operation. (Zane's call if he wants it moved to Shift+R.)

### 7. Update the tooltip
"Rebuilds the surface with even triangles at this resolution. It keeps both sides as they are; tick Mirror result to make it exactly symmetric."

## Tests (in the style of the existing ones)

- Remesh of an asymmetric model with the object's Mirror X **on** and Mirror result **off** keeps the two sides different (the case that destroyed the door).
- Mirror result on, with a model whose sides already match: the result is exactly symmetric, as before.
- Mirror result on, with a model whose sides differ: the question is asked, and its default answer rebuilds without mirroring.
- Mirror result is off again after any remesh, and isn't written to the `.sclp` or the settings.
- Every operation found in step 5 gets one test proving brush symmetry doesn't change its result.

## Working rules

Zane may be sculpting while this is built. Follow the **Ground rules** at the top of `docs/renderer-plan.md`: work in a worktree only, never build into or touch `D:\Sculptingapp2\target` or a running `sculpt.exe`, use `SCULPT_SETTINGS_DIR` for the dev build, cap build jobs, and leave merging to Zane. After he merges, rebuild the main checkout, because `Sculpt.bat` won't.
