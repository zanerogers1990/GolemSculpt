# Strap tool plan

Written 2026-11-09. Build only the phases Zane approves.

## Status (2026-11-09)

Phases 1 and 2 are built (Zane approved both). Nothing is committed yet. Zane has not tried it with
the pen.

- **Where the code is:** `crates/tube`. Strap is `Kind::Strap` of the Tube tool, on key E, in the
  Make group.
  - `strap.rs`: how a strap is laid on the body, and its swept mesh.
  - `wrap.rs`: cutting the model with the drawn line, and picking the loop to wrap.
  - `strap_demo.rs`: `--demo-tool strap:[wrap][-merged|-object|-selected]`.
  - `tests/strap.rs`: 10 tests.
- **Changes to app files:** one line each in `src/app/tools.rs`, `src/ui/tools.rs` and
  `src/ui/assets.rs`, plus the icon `assets/icons/tool_strap.png`.
- **Test build:** `target\w\strap\Try-Strap.bat`. Like other test builds, it contains whatever else
  was uncommitted in the tree when it was built.

Differences from the plan below:

- **Live straps are not saved in the file.** No live shape of this tool is, tubes included. Keep
  Separate or Merge before closing. The app asks first, as it does for tubes.
- **The wrap line stays on a belt** until one of its points is edited by hand. Dragging a line end
  tilts the belt; dragging the middle slides it. After a hand edit, re-tilting means wrapping again.
- **Merge sinks the strap a quarter of its thickness into the model and remeshes the two together**
  at the model's own density. On a low-detail model the result is coarse, and the status line says
  so. Keep Separate is the clean way.
- **New tooltips are in English only.** They have not been run through `tools/translations.py` yet.
- **Found along the way, not from this work:** debug builds crash at start in
  `src/app/wintab.rs:247`. There, `WTI_DEVICES + lc.device` overflows with the SuperDisplay
  Wintab32.dll. Release and dist builds do not check for overflow, so they run past it. A
  `wrapping_add`, or a range check on `lc.device`, would fix it.

## What it is, in plain words

A new tool, **Strap**, for belts, bands, straps, harnesses, collars, cuffs and
sandal straps. You get a flat band with an exact width and thickness, square
or softly rounded edges, sitting on the model without sinking into it. It
stays editable until you accept it, and then becomes its own object (or fuses
into the model if you ask).

It makes straps in two ways:

- **Draw** (over-the-shoulder straps, bandoliers, harness straps, sandal
  straps): draw a stroke on the model, like the Muscle tool. The strap lies
  along it. Drag its points to reshape it; finish the stroke on its own start
  to close it into a loop.
- **Wrap** (belts, waistbands, armbands, collars, wrist and ankle cuffs):
  drag one straight line across the model where the belt goes. The tool goes
  all the way round the body, including the back you cannot see, and lays a
  closed band there. Drag the line's ends to tilt it and slide it up or down.

Then the leather details, all optional, all live until you accept:
stitching along the edges, a pressed edge line, belt holes, studs or rivets,
and a buckle with a keeper loop.

## What it is not

- Not a cloth simulator: straps do not hang or fold. A strap lies on the
  surface it is drawn on.
- Not a replacement for Extract. Extract is still the way to make a strap
  that follows every detail of what you sculpted under it. Strap makes clean,
  even, manufactured-looking bands.

## Phases

### Phase 1: Draw a strap (the core)

- New Strap button and key. Use a key the key map has free, checked at build
  time, since Shortcuts lets Zane change it anyway.
- Draw on the model and a strap lies along the stroke. A rough stroke is
  cleaned into the line it meant, as Muscle does.
- Panel:
  - **Width** and **Thickness**, typed in mm like Extract.
  - **Edges** from square to fully round.
  - **Gap**, how far it stands off the skin (default a hair, so it never
    pokes through).
  - **Ends**: Square / Round / Point (a belt tip).
  - **Closed** loop switch.
- Point editing works as it does in Tube and Muscle: drag, add, delete, make
  sharp, and Shift-drag an end to extend.
- Follows brush symmetry: a strap on the left shoulder makes the right one.
- **Accept** makes it its own object named "Strap", with the body's symmetry
  settings. **Merge** fuses it into the model instead, the way Tube does.
  Undo hands it back live.
- Live straps are not kept in the file, the same as tubes and muscles. Keep Separate or Merge them
  first; the app asks before closing if any are live.

### Phase 2: Wrap a belt

- A Wrap / Draw switch at the top of the panel. In Wrap, a drag across the
  model draws the belt line.
- The line and the way you are looking set a slicing plane. The tool cuts the
  model with it and takes the outline loop the line crossed, ignoring arms or
  a tail the same plane also cuts.
- **Hug** slider: at 0 the belt follows every dip, such as the small of the
  back. Higher values make it bridge the hollows the way a tight belt does.
  The default bridges hollows, because that is what real belts do.
- Handles: drag either end of the line to tilt the belt. Drag the band itself
  to slide it along the body.
- Everything in Phase 1 (Width, Thickness, Edges, Gap, accept/merge) applies.

### Phase 3: Leather details

Each detail is a switch with its own few sliders, and is real geometry so it
prints and exports:

- **Stitches**: a row of small slanted dents along one or both edges. Sliders
  for Inset from the edge, Spacing and Stitch length.
- **Edge line**: a groove pressed in along each edge.
- **Holes**: a row of belt holes near one end. Count and Spacing. They are
  real holes cut through the strap, made with the app's booleans, so a
  printed belt has them.
- **Studs**: domed rivets along the middle or both edges. Count or Spacing,
  and Size.

### Phase 4: Buckles, keepers, UVs

- **Buckle** at either end. Built-in shapes: square frame, rounded frame,
  D-ring, and plain ring. Each is sized to the strap's width and comes with a
  tongue. The buckle is its own object so it can be painted, printed or
  swapped.
- **Keeper**: the small loop that holds the belt's free end.
- **UVs** on the strap: along its length and across its width. This makes a
  leather texture in Blender lay straight with no unwrapping.

## Decisions made (for the builder)

1. **The strap is a swept mesh, not a distance field.** Tube and Muscle are
   distance fields meshed on a voxel lattice. That rounds every edge, gives
   uneven thickness at belt sizes, and needs a fine lattice for a thin band.
   A strap wants crisp, even edges. So it is built as rings of a
   rounded-rectangle cross-section swept along the path. The ring's "up" is
   the smoothed surface normal and its "across" is that normal crossed with
   the path direction.

   A field avoids two problems for tubes, and the strap handles them its own
   way:
   - Tight bends folding the inner wall through itself: smooth the path
     until no bend is tighter than half the width, and say so in the status
     line when it had to.
   - A strap crossing itself: this is allowed, and it stays a separate
     object, so nothing breaks.

   Merge still goes through the voxel union in `crates/tube/src/merge.rs`.
   Its edges round a little there, so tell Zane on the Merge button tip.
2. **Strap is a third kind of the Tube tool, not a new crate.** It is
   `Kind::Strap`, beside `Kind::Muscle`, the way Muscle was done. Point
   editing, symmetry, undo, saving and Merge already live there (`input.rs`
   is 1100 lines of it) and come free. The new parts go in new files:
   - `strap.rs`: the sweep, cross-section, ends and gap push-out.
   - `wrap.rs`: Phase 2's slice, pick-the-loop and Hug.
   - `details.rs`: Phase 3.

   The surface laying is Muscle's `Ground` (`crates/tube/src/muscle.rs`),
   reused as it is. Buckles (Phase 4) are their own small module that only
   makes meshes.
3. **No poke-through.** After the sweep, every inside-corner point is checked
   against the model's surface (the BVH closest-point query in
   `sculpt-volume`). Anything under the skin is pushed out to Gap.
4. **Accept = separate object by default.** Belts are normally kept apart for
   painting, printing and Blender. Merge is the second button.
5. **Phase 4 UVs need one toolkit addition.** A tool hands the app a plain
   `TriMesh` with no UVs, so `MeshData` needs an optional UV list. That is a
   shared crate: make the change small and on its own, and tell any session
   working in `toolkit` first.

## Before building

- The Tube crate has uncommitted changes in the main tree: `input.rs`,
  `lib.rs`, `panel.rs`, `tool.rs`, `tests/tool.rs`. These are the
  10-30 Collapse Points on Drop work. Get them committed, or build in a
  worktree that starts from them. Otherwise Phase 1 would be written on top of
  half-finished edits.
- Build in `target\w\strap` and test on monitor 2.
- Check whether another session is in the Tube crate before starting.

## How each phase is checked

- Tests in the tube crate:
  - the strap's thickness is even;
  - its width is exact;
  - no point is under the model's surface;
  - a closed loop has no seam;
  - mirrored straps match;
  - for Wrap, the right loop is picked on a body-with-arms test model.
- A scripted run (no dialogs) that draws a shoulder strap and wraps a belt on
  a saved body, with screenshots for Zane.
- Zane tries it with the pen.
