# GolemSculpt on the web

A browser version of GolemSculpt: a demo people can try without installing
anything, and a quick place to open a mesh, push it around and save it again.
It is the same sculpting engine as the desktop app, with a much smaller
interface and most of the tools left out.

## Status (2026-11-15)

Built and working in `web/` (how to build and host it: `web/README.md`).
Phases 1 to 3 are done; phase 4 (shipping) is left.

- **Sculpting:** the whole brush library, the Stroke and Falloff cards,
  alphas (Drag to Place too), pen pressure from every pen sample, the
  Space brush wheel, number keys and S/F/D hold keys, masking (brush,
  Masking card, box and tap gestures), X/Y/Z and radial symmetry, undo.
- **Mirror tools:** the desktop's Symmetry Repair (Check Symmetry, Make
  Symmetric, Cut & Mirror, Symmetry Cleanup, Match Tolerance), folded under
  the Symmetry card; and Live Mirror in the Objects card (live copies across
  the world X/Y/Z planes, strokes on either side, Validate, Remove; kept with
  the scene and in saved OBJ/STL). Not ported: the gizmo on a copy.
- **Mesh:** dynamic topology, Remesh, Decimate; subdivision levels (the
  desktop's Subdivision card: Subdivide, step between levels with Page Up /
  Page Down, Delete Lower / Higher) and sculpt layers (the desktop's Layers
  card, and the Erase Layer brush). Both are kept with the scene in the
  browser (store layout W03, the desktop's own LVLS/LAYR file blocks).
- **Pose, Paint, Face Groups (2026-11-16):** the Pose brush (key 7, the
  desktop's Pose card via `ui/pose.rs`, the white chain preview, Works From
  Brush Ring / Face Group); the Paint brush (P, I eyedropper, Sample mode,
  swatches, Zadd) with the Paint Layers card (new, duplicate, merge, flatten,
  reorder, eye, lock, opacity, blend, mask source, fill, clear) and Mask by
  Paint; the Face Groups card (Group Masked/Visible, Auto, by Angle, Pick,
  Merge, Isolate, Hide, Mask Group, Ctrl+Shift+click) and Stay in Group.
  Paint and groups are kept with the scene (layout W04: the desktop's PNTL
  block and the group ids), follow Remesh and Decimate, and go to and come
  from OBJ (vertex colours, `g Group_N` lines). Colour only: the desktop's
  painted materials, roughness and metallic are left out (the web has no
  Materials card or lit renderer for them). Group colours are blended per
  vertex (WebGL2 has no storage buffers), so a border is soft by one
  triangle. Paint shows on objects that are not active; group colours only
  on the active one.
- **Objects:** several objects (Objects card: add, copy, hide, delete), each
  with its own place, turn and size; the desktop's transform gizmo (W),
  with masked posing (the desktop's `app/masked.rs`); brushes and tools
  work on the active object in its own space.
- **Shaping (2026-11-16):** the Deform card under the Transform card (Bend,
  Bend Arc, Twist, Taper, Flatten, Inflate, Stretch, Lattice, with their
  handles; the Lattice mirrors and repeats round a radial axis with the
  brushes' symmetry); Edit Pivot, Pivot to Centre and Align to World in the
  Transform card; Apply Scale and Set Origin to Mesh in the Objects card
  (Remesh evens a stretched object first); Subtract, Union, Intersect and
  Join in the Objects card for two or more selected (resolution up to 512,
  face groups not carried); Project Detail under Subdivision; Quad Remesh in
  the Remesh card (on a worker thread in the many-threads build, otherwise
  the page says it is working and is still until it is done). Files:
  `web/src/deformers.rs`, `placement.rs`, `combine.rs`, `project_detail.rs`,
  `quad_remesh.rs`, `shaping.rs`.
- **Tools:** Tube, Muscle, Strap, Lathe, Hand & Foot, Array, Extract, Mask,
  Hide, Trim, Split, Measure, Mask Fit (2026-11-16: it sees the other
  objects shown, to pick the head), hosted through the tool kit; what they
  make becomes a new object.
- **SSpheres** (2026-11-16): the desktop's blockout tool (`src/ssphere/`,
  compiled in unchanged, drawn by the desktop's own `render/spheres.rs`):
  Draw, Move (with the gizmo's arrows on the selected sphere), Scale and
  Rotate, the live see-through skin and Solid Preview, X symmetry, Attach,
  its own undo history, Make Skin into a new object, and New SSphere Rig in
  the New Shape card. Not ported: its part of the hold-Space wheel.
- **Look:** the desktop's interface and its eight colour themes, matcaps,
  the Clay, Metal and Wood finishes, with the desktop's soft key-light
  shadow (Lighting card) and ambient occlusion (Effects card) in the WebGPU
  builds (WebGL2 keeps the lighter look, the two switches greyed out; no
  softboxes, HDRI or contact shadows), cavity, wireframe, backdrop, lens,
  Save Picture.
- **Files:** open OBJ/STL/PLY, save OBJ (each object named) and STL; the
  scene is kept in the browser across a refresh. Save Scene downloads the
  whole scene as a `.golem` file (the same bytes the browser keeps: objects,
  places, masks, hidden faces, levels, layers, symmetry, view); Open or a
  drop takes one back after asking, since it replaces the scene.
  FBX (2026-11-16): open and Save FBX with the desktop's own reader and
  writer (`web/src/fbx.rs`); each object in an FBX comes in as its own,
  quads, UVs and colours kept; Save FBX writes every object shown with its
  live copies. No glTF, .sclp or STEP, at Zane's word.
- **Reference pictures (2026-11-16):** the desktop's Reference Images card
  (last on the Scene tab), Add Picture... or a .png/.jpg dropped on the page;
  facing, size, place, opacity, Flip, In Front, Show; kept with the scene
  (browser and `.golem`), drawn in the viewport only.
- **Shortcuts and languages (2026-11-16):** the desktop's key map
  (`src/keymap.rs`) and its Keyboard Shortcuts card on the Settings tab
  (GolemSculpt and ZBrush sets), kept in the browser; the keypad keys cannot
  be told apart in a browser. Tooltips in the desktop's 11 other languages,
  with the first-visit language question; a language's file (and for
  Japanese, Chinese and Korean a cut-down Noto Sans CJK, `web/fonts/`) is
  fetched only when it is picked.
- **Speed:** three builds, picked by the page: many threads (WebGPU, needs
  the two headers in `web/README.md`), one thread WebGPU, one thread WebGL2.
  Only the changed parts of the mesh are sent to the graphics card, and a
  big mesh is drawn only where it is in view.

- **First visit:** a welcome card with the essentials (F1 or Help brings it
  back); Settings > About opens `THIRD-PARTY-LICENSES.txt`, which the build
  writes (`web/licences.py`).

Not done: testing with a real pen and in Firefox/Safari, and cavity and
mask tint on objects other than the active one. Lure and resin finishes were left out at Zane's word.

## What was checked (2026-11-15)

A test crate in `web/` compiles the desktop app's own core files for the
browser (`wasm32-unknown-unknown`) without changing a line of them:

- `src/mesh/` (mesh, spatial index, dyntopo, levels, masks, groups, paint data)
- `src/sculpt/` (every brush, stroke spacing, symmetry, pressure curves, alphas)
- `src/camera.rs`, `src/util.rs`
- the crates `geom`, `multires`, `cut`, `symmetry`, `volume`, `array`

The only gap was five references to the file module (`crate::io`), filled by a
40-line stand-in, `web/src/io.rs`.

So the web version does **not** need the sculpting rewritten. It reuses the
desktop code directly, and a brush fixed on the desktop is fixed on the web
too. The new work is the part around it: a small renderer, a small
interface, and getting meshes in and out through the browser.

## What the web version keeps

| Area | Kept |
| --- | --- |
| Start | Sphere, cube, plane; open OBJ / STL / PLY (pick a file or drop it on the page) |
| Brushes | Clay Strips, Clay, Standard, Dam Standard, Inflate, Smooth, Move, Snake Hook, Crease, Pinch, Flatten, Mask; brush feel and presets from the brush library |
| Stroke | Size, strength, falloff, pen pressure, lazy mouse, X/Y/Z symmetry |
| Mesh | Undo/redo, Remesh (capped resolution), Subdivide, Decimate (phase 2) |
| Look | Matcap shading (GolemSculpt Teal Matte, Ben Simonds), cavity, wireframe toggle |
| Save | Download OBJ / STL; the scene is kept in the browser so a refresh loses nothing |

## What is left, sorted (2026-11-16)

Checked by comparing the desktop's modules with `web/src` and looking at what
each leftover one reaches for (other app files, the disk, other programs).

**Can come over (pure engine code plus a card):** Pose brush and Pose by face
groups; vertex Paint; the Groups card; Deform (Lattice, Bend, Twist; only
needs `crate::mesh`); Apply Scale, Edit Pivot, Origin; Project All; booleans;
Keys and Mask Fit (tool kit tools like Tube); Quad Remesh; Unwrap UVs and
Bake Maps (maps download as PNGs); FBX / glTF / .sclp / STEP in and out
(through file pick and download); reference images (photo drop); the shortcut
editor; tooltip languages. Most only need `std::time::Instant` swapped and
their file writes turned into downloads.

**Partly:** ray-traced Render Picture (WebGPU only, no WebGL2 fallback;
finish shadows/AO are done, 2026-11-16); turntable/timelapse (as a WebM through the browser's
recorder, not MP4); older versions and recovery copies (browser storage,
size-capped); Save over the same file (Chrome/Edge only; others download a
new copy). Spheres is done (2026-11-16).

**Cannot:** Blender link (starts a local Blender), Claude bridge / MCP (a page
cannot listen for connections), Unity import (reads project folders), WinTab
(the browser reads pen pressure itself, so not needed), 10M+ meshes (4 GB page
memory cap), full 32-core speed.

## What the first version left out

Blender link, Claude bridge (MCP), STEP / FBX / glTF / .sclp, bake and UVs,
ray-traced renders, the Lure and Resin finishes, timelapse and turntable,
Unity import, the Keys and Mask Fit tools, vertex paint, Pose, reference
images, recovery copies and older versions, the shortcut editor, tooltip
languages, WinTab.

Any of these can come back later where the browser allows it; the list is
about keeping the first version small and fast.

## How it is built

- **One engine, two programs.** `web/` is its own cargo workspace (so the
  app's `Cargo.toml` is never touched) and pulls the desktop files in with
  `#[path = "../../src/..."]`. Nothing is copied.
- **Interface:** eframe (the web runner for egui, the same interface library
  the desktop uses). One side panel: brush buttons, size, strength,
  symmetry, open/save. Keyboard shortcuts match the desktop's defaults.
- **Renderer:** written new and small (a few hundred lines plus one matcap
  shader), drawn inside the egui page through wgpu, the same graphics library
  as the desktop. It uses WebGPU where the browser has it (Chrome, Edge,
  Safari 26, recent Firefox) and falls back to WebGL2. It uploads only the
  parts of the mesh a stroke touched, and can reuse the desktop's
  `render/chunks.rs` to skip parts out of view.
- **Pen pressure:** browsers report pen pressure on every pointer event,
  through Windows Ink on the Huion, and from an Apple Pencil on an iPad.
  The web version reads it from the page directly.
- **Speed, step 1:** one thread. `rayon` already falls back to running on the
  calling thread in the browser, so the engine works unchanged, just without
  the 32 cores.
- **Speed, step 2:** many threads, with `wasm-bindgen-rayon`. This needs the
  nightly compiler for the browser build only, and the page must be served
  with two security headers (itch.io has a "SharedArrayBuffer support"
  checkbox for exactly this; GitHub Pages needs a small service-worker
  shim). Expected 4-8x on brushes and Remesh on an 8-core laptop.
- **Memory:** a browser page can address up to 4 GB. That is plenty for
  demo-sized models (a few million triangles), not for the 10M+ the desktop
  handles.

### Changes needed in the desktop's files

Small and harmless to the desktop build:

- `std::time::Instant` panics in a browser. In `src/sculpt/mod.rs` and
  `src/sculpt/pace.rs` it becomes `web_time::Instant` for the browser build
  only (`#[cfg(target_arch = "wasm32")]`), and the 5 ms `thread::sleep` in
  `pace.rs` is skipped there.
- Nothing else so far. If a desktop change makes `src/mesh` or `src/sculpt`
  reach into the rest of the app (`crate::scene`, `crate::app` ...), the web
  build stops compiling. Check with:

  ```
  cd web
  cargo check --target wasm32-unknown-unknown --target-dir ../target/w/web
  ```

## Phases

1. **Prototype.** A sphere in the browser: orbit, pan, zoom; Clay Strips,
   Smooth, Move and Mask with X symmetry, pen pressure and undo; matcap
   shading. Runs locally. Measure frames per second at 250k and 1M
   triangles, single-threaded.
2. **Usable demo.** All the kept brushes and settings, open and save, Remesh
   and Subdivide, the scene kept in the browser across refreshes, a short
   help overlay.
3. **Fast.** The many-threads build, out-of-view culling, and a size and
   start-up pass (target: page usable in a few seconds, download around
   3-5 MB compressed).
4. **Ship.** An HTML5 upload for the GolemSculpt itch.io page and/or GitHub
   Pages; check on Chrome, Edge, Firefox, Safari, and an iPad with a pencil.

## Targets

- Sculpting at 60 frames a second on a 500k-triangle model, single-threaded,
  on an ordinary laptop; 1-2M with threads.
- The same brush feel as the desktop at the same settings (same code).
