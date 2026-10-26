# Crates

The app is the `sculpt` package at the top of the repository. Everything that
can stand on its own lives here as a separate crate, so it can be built,
tested and timed without the app, and so several people (or several Claude
sessions) can work at once without one half-finished edit stopping everyone
else's build.

| Crate | Package | What it holds |
| --- | --- | --- |
| `geom` | `sculpt-geom` | Plain triangle meshes, bounds, single-triangle queries, symmetry operations, real-world units. Everything else builds on it. |
| `volume` | `sculpt-volume` | Signed distance fields, surface extraction, voxel remesh, booleans. |
| `cut` | `sculpt-cut` | Screen regions (box, lasso), trimming, splitting, cap triangulation. |
| `symmetry` | `sculpt-symmetry` | Symmetry maps, mirror and weld, symmetrize, mirror repeaters. |
| `lathe` | `sculpt-lathe` | Lathe profiles and the meshes turned from them. |
| `array` | `sculpt-array` | Copies laid out along lines, circles and curves. |
| `toolkit` | `sculpt-toolkit` | What a tool sees of the app: the interface tool crates are written against. |
| `measure` | `sculpt-measure` | The Measure tool: the first tool written against the toolkit. |
| `tube` | `sculpt-tube` | The Tube tool: tubes drawn along curves and fused into the model. |
| `hands` | `sculpt-hands` | The Hand & Foot tool: hands and feet built from sliders and poses, accepted as a base to sculpt. |
| `keys` | `sculpt-keys` | The Keys tool: joints that register the pieces of a split model. |
| `region` | `sculpt-region` | The region tools: box and lasso select, hide, mask, trim and split, and mask extract. |
| `uv` | `sculpt-uv` | UVs made for a mesh that has none: charts cut along normals and face groups, laid flat (LSCM, then ARAP, by the `uvunwrap` crate) and packed into the square, each polygon kept whole. |
| `multires` | `sculpt-multires` | Subdivision levels: Loop and Catmull-Clark subdivision, and a mesh held at several levels with the finer detail carried through edits of the coarser ones. Also carries the app's own per-vertex vectors (sculpt layers) through a new level and up from an edited one, and reads how a surface turned at a point off the faces round it. |

The Unity app these are ported from is `D:\Unity\sculptingapplication`
(read-only; ignore its `.claude\worktrees`, `Library`, `Temp` and `Logs`).
The comments in its code record why things are the way they are. Carry the
reasons that still apply over into the Rust; leave Unity-specific ones behind.

## Rules

1. **One owner per crate at a time.** Edit only the crate you were given.
   If you need something from another crate that it does not offer, say so
   in your report instead of adding it there.
2. **Do not edit any `Cargo.toml`.** A manifest that does not parse stops
   every build in the repository, including other sessions'. Ask for a
   dependency instead. `glam` and `rayon` are already there.
3. **Build with your own target directory**, so builds do not queue behind
   each other or behind the app:

   ```
   cargo test -p sculpt-<name> --target-dir target/w/<name>
   ```

   Add `--release` only for timing runs.
4. **Keep the crate compiling.** Other crates build against it while you
   work. Write a module out in full, then add its `mod` line and fix what
   the compiler finds straight away.
5. **If another crate fails to compile and it is not yours, wait a minute
   and try again.** Its owner is mid-edit.
6. **Plain data in, plain data out.** Take `&[Vec3]`, `&[u32]`, `&TriMesh`;
   return `Vec`s or a `TriMesh`. No globals, no I/O, no app types. Report a
   failure the user could cause as `Result<_, String>` with a message they
   could read; keep panics for bugs.
7. **Speed is a feature.** Meshes here run to tens of millions of triangles
   on a 32-thread machine. Use flat `Vec`s rather than maps of small
   allocations, `rayon` for anything per-vertex, per-triangle or per-cell,
   and avoid allocating inside hot loops. Where the Unity code used Burst
   jobs or `Parallel.For`, the Rust should be parallel too. Add an
   `#[ignore]`d timing test for anything that runs on a whole mesh, and say
   in your report what it measured.
8. **Tests.** Port the Unity tests for your files where there are any
   (`Assets\Tests\Editor`), and add tests of your own for anything with an
   answer that can be checked: volumes, counts, watertightness, symmetry,
   round trips.
9. **Clamp with `clamp`.** rustc 1.99.0 miscompiles
   `if x < 2 { 1 } else { x.min(MAX) }` on unsigned integers at opt-level 1
   and up: it returns 0 for 0. Two sessions reproduced it standalone for u8,
   u32, u64 and usize. It is the shape that is unsafe, not the spelling: an
   unsigned comparison against a small constant that yields a constant, with
   a `min` of the same value on the other path, whether written with `.min`,
   `std::cmp::min`, a reversed condition, an early return or a `let` first.
   The same source can be right for one type and wrong for another side by
   side, so a version that passes in one place is not cleared.
   `x.clamp(lo, hi)`, `x.max(lo).min(hi)` and an if / else-if chain have been
   right in every test. Write limits that way, and give any count, segment or
   resolution limit a test at its lowest input.
10. **Match the house style** (see `src/` and `crates/geom`): a `//!` header
   saying what the module is for, doc comments that explain why rather than
   what, short functions, no commented-out code, 4-space indent, lines to
   about 120 columns. Names are Rust names, not C# ones.
