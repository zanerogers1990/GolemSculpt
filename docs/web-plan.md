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
- **Mesh:** dynamic topology, Remesh, Decimate; subdivision levels (the
  desktop's Subdivision card: Subdivide, step between levels with Page Up /
  Page Down, Delete Lower / Higher) and sculpt layers (the desktop's Layers
  card, and the Erase Layer brush). Both are kept with the scene in the
  browser (store layout W03, the desktop's own LVLS/LAYR file blocks).
- **Objects:** several objects (Objects card: add, copy, hide, delete), each
  with its own place, turn and size; the desktop's transform gizmo (W),
  with masked posing (the desktop's `app/masked.rs`); brushes and tools
  work on the active object in its own space.
- **Tools:** Tube, Muscle, Strap, Lathe, Hand & Foot, Array, Extract, Mask,
  Hide, Trim, Split, Measure, hosted through the tool kit; what they make
  becomes a new object.
- **Look:** the desktop's interface and its eight colour themes, matcaps,
  the Clay, Metal and Wood finishes (lighter than the desktop's: no shadows
  or ambient occlusion), cavity, wireframe, backdrop, lens, Save Picture.
- **Files:** open OBJ/STL/PLY, save OBJ (each object named) and STL; the
  scene is kept in the browser across a refresh.
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

## What it leaves out

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
