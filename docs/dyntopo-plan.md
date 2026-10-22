# Dynamic topology: what the others do, and a plan

Written 2026-10-21 after reading Blender's dyntopo source (main branch:
`blenkernel/intern/pbvh_bmesh.cc`, `editors/sculpt_paint/mesh/sculpt_dyntopo.cc`,
`sculpt_detail.cc`, `bmesh/intern/bmesh_log.cc`), Joe Eagar's unmerged
sculpt-dev and temp-sculpt-dyntopo branches, Botsch & Kobbelt's 2004 remeshing
paper with the pmp-library code, and the manuals and forums of ZBrush
(Sculptris Pro), Nomad Sculpt and 3DCoat. Nothing here is built yet.

## Where ours stands

`src/mesh/dyntopo.rs` (the mesh side) and `src/sculpt/dyntopo.rs` (the brush
side), switched on in the Remesh card.

- **Splits only.** Long edges under the brush are bisected along each
  triangle's longest edge, with Delaunay flips before and after. Nothing is
  ever merged back: a region stays as fine as the finest brush that touched it.
- **Detail is a share of the brush radius** (3% to 50%, default 15%). There is
  no screen-based or fixed-size detail, and no way to even out a whole object
  to one size short of a voxel Remesh.
- **Stops at 4 million triangles.**
- **Only Standard, Clay, Crease, Inflate, Flatten and Layer refine.** Snake
  Hook, Move, Pose, Smooth and mask painting do not.
- What it does well, and the others mostly do not: triangle shape. Bisection
  along the longest edge plus Delaunay flips keeps slivers out where Blender
  gets them (Snake Hook in Blender is known for long skinny faces). Undo is one
  record per stroke and keeps face groups, mask and hidden faces exactly.
  Blender's undo resets face sets.

## What the others do

| | Blender | ZBrush Sculptris Pro | Nomad | Ours |
|---|---|---|---|---|
| Detail set by | screen pixels (default, 12 px), fixed size, % of brush (default 25%), or manual | brush size (Adaptive Size, default) or two sliders | screen, brush radius, or fixed | % of brush radius |
| Merges small triangles | yes, default (Subdivide Collapse) | yes ("Combined", UnDivide Ratio) | yes, default (Uniformisation); also add-only, remove-only | no |
| Even out a whole object | Detail Flood Fill (Ctrl R) | no | Remesh at a detail | voxel Remesh only |
| Pick the detail off the surface | yes (pipette) | yes | no | no |
| Pen pressure on detail | through brush size | not documented | "Use pressure on radius" | no (refines for the full ring) |
| Snake Hook refines | yes | yes (Move does not) | Drag yes, Move no | no |
| Smooth | excluded ("users preferred") | decimates by UnDivide Ratio | needs Relax | excluded |
| Per-brush on/off | no | yes, with "Use Global" | yes, an icon per tool | no |
| Cap | none | about 3.5M (user report) | not stated | 4M |

The numbers that matter, from Blender's source:

- Split above the detail length, **collapse below 0.4 of it** (`EDGE_LENGTH_MIN_FACTOR`).
  Botsch & Kobbelt use 4/3 and 4/5 of a target; pmp refuses a collapse that
  would make an edge longer than the split length, which stops the two from
  undoing each other. Our bisection makes children of at least about half the
  detail length, so a fresh split can never be collapsed at 0.4.
- Collapse runs **before** subdividing, shortest edge first; a border vertex
  survives where it is, otherwise the survivor moves to the midpoint.
- Blender main does **none** of the usual collapse safety checks (link
  condition, flipped faces, fan size) and repairs the folds afterwards; Joe
  Eagar's branch added face-set borders and mask pinning. Ours should check
  first (below).
- Relative detail: edge = (radius / pixel radius) x detail px / 0.4, so the
  default is about 30 pixels on screen.

What sculptors ask for most, across all of them: detail that is steady when
zooming (Nomad users asked for Constant), merging that does not wreck creases
and borders, Snake Hook that keeps a pulled tube round, a way to see the
density before the stroke, and speed past a million triangles.

## The plan, in the order I would build it

### 1. Merge small triangles (collapse)

**Built 2026-10-21** (see the README's Dynamic topology section and the
module notes of `src/mesh/dyntopo.rs`). Where it differs from the plan below:

- The vertex that stays does not move to the middle of the edge (Blender's
  way); it stays where it is, as in Botsch & Kobbelt and pmp. The one folded
  away is parked on it, with a zero normal, so that nothing that measures the
  model or sums normals under a brush finds it.
- Merges are made only on surface the stroke has not yet moved. Merging in
  ground a stroke had already shaped changed the stroke-start surface Clay
  and Standard build from, differently in each copy of a mirrored or radial
  stroke (one vertex 3.9% of a Clay stroke off its partners, against 0.7%).
  The merge pass runs just ahead of each dab, so a big brush still coarsens
  everything it reaches.
- A merge may turn a triangle 30 degrees at most (the plan said 60): at 60 a
  big brush along a thin pulled shape left it a lumpy polygon end on.
- Ties between the two ways of folding an edge, and the order merges are
  made in, are decided by distance from the object's middle, which mirror
  images and radial repeats share.
- Hand-offs (Export OBJ, STL, PLY, FBX, Send to Blender, Decimate,
  Subdivide, Check Symmetry) get a tidied copy (`scene::tidied`); Make
  Symmetric, Symmetry Cleanup and Groups by Angle first replace the mesh
  with one, as an undo step. Shown counts are of the live surface.
- Still to do: reuse the places merges leave for the triangles splits make.
  A fine pass then a coarse one left 17,698 triangles in the surface and
  37,934 in the lists (`--brush-hash`); they are cleared on save and reopen
  or any rebuild, never drawn or counted, but take memory meanwhile.

What was planned:

The one big thing missing. With it, detail follows the brush both ways: a big
brush over a fine area makes it coarse again, the triangle count stays
bounded, and the 4 million cap is rarely met.

- A **Method** setting: Add and Remove (the default, as in Blender and Nomad),
  Add Only (today's behaviour), Remove Only (a simplify brush).
- **How, without breaking undo.** A merged triangle is not removed: it is
  rewritten to a point (`[k, k, k]`), which draws nothing, is never hit, and is
  already skipped by every dyntopo scan and dropped by `Mesh::new` when a file
  is opened or the mesh rebuilt. The merged-away vertex keeps all its data and
  simply has no triangles. So a collapse is triangle rewrites (which the stroke
  record already keeps) plus one vertex move (which the ordinary stroke record
  keeps), and undo needs no new machinery. Dead slots are cleared out whenever
  the mesh is rebuilt anyway (save and open, Remesh, Decimate).
- **Checks before every collapse** (ported from our own decimator,
  `crates/geom/src/decimate.rs`, `Work::sound`): one or two triangles on the
  edge with matching winding and nothing hidden; the link condition (the two
  ends share no neighbour but the one across each side); every fan keeps at
  least three triangles; never across a mesh border or a face-group border, and
  a border vertex stays put; no triangle around it turns over or flattens; no
  new edge longer than the split length; never where both ends are frozen by the
  mask, and a frozen end stays put; never with subdivision levels.
- Everything outside dyntopo that walks every triangle or vertex has to cope
  with the point triangles and vertices that have none: the triangle count shown,
  export, the closed-surface check, smoothing next to such a vertex, Send to
  Blender, the symmetry tools. That audit is most of the work.

### 2. Detail by screen size or fixed size, and Detail Fill

- **Detail based on**: Brush (today's), Screen (triangles a set number of
  pixels across, so zooming in gives finer detail, Blender's default), Fixed
  (one size on the model, steady whatever the zoom or brush).
- **Detail Fill**: brings the whole object to the fixed size, adding and
  removing as needed, without the voxel Remesh's change of surface.
- **Pick Detail**: click the model to take the size of the triangles there.
- **Pen pressure on detail**, for Brush mode: a light touch refines as finely as
  the dab it makes rather than the full ring.

### 3. Snake Hook refines

The classic Blender use: pull out a horn and the tube keeps adding even
triangles as it stretches. Ours bisects along the longest edge and flips, so it
should stay free of the slivers Blender is known for here. Move and Pose stay
out (they hold the vertices they took at the press). Smooth stays out by
default, as Blender settled on.

### 4. Polish

- A small marker in the brush ring at the triangle size the brush will leave
  (ZBrush colours its cursor, Blender draws a grid).
- A light sideways relax of the new vertices, so new triangles come out even
  without changing the shape (Botsch & Kobbelt's tangential smoothing).
- Raise the 4 million cap once collapse keeps counts down: the cost that grows
  with the whole mesh is undoing a refining stroke, which rebuilds the
  adjacency of the whole mesh; to be measured before choosing a number.
- Maybe later: dynamic topology on or off per brush.

## Rough size

Phase 1 is the big one (a day of careful work and tests). Phases 2 and 3 are
smaller and could follow the same day. Each phase lands on its own, tested,
with the README updated, and Zane tries it with the pen before the next.
