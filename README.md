# Sculpt

A native sculpting app written in Rust, modelled on the Unity sculpting app in
`D:\Unity\sculptingapplication`: same controls, same brush behaviour, same look.

## Run it

Double-click `Sculpt.bat`. The first run builds the app (a minute or two);
after that it starts instantly. The program itself is
`target\release\sculpt.exe`.

For pen pressure, tick **Enable Windows Ink** in the Huion driver.

## Controls

| Input | Action |
| --- | --- |
| Left drag on the model | Sculpt |
| Ctrl + drag, or right drag | Sculpt inverted |
| Shift + drag | Smooth |
| Left drag on the background | Orbit |
| Alt + drag | Orbit |
| Middle drag, or Alt + Shift + drag | Pan |
| Ctrl + Alt + drag | Zoom |
| Scroll over the model | Brush size |
| Scroll off the model | Zoom |
| Double-click background, or F F | Frame the selection |
| Double-click another object | Select it |
| Ctrl + click a row of the object list | Add that object to the selection |
| H / Shift + H / Alt + H | Hide the selection / hide everything else / show everything |
| / | Local View: only the selection, framed; again to bring the rest and the view back |
| Delete | Delete the selected object (asks first) |
| 1 2 3 4 5 6 8 9 0 | Move, Clay, Smooth, Crease, Inflate, Flatten, Standard, Layer, Snake Hook |
| S / F / D + drag | Brush size / strength / focal shift |
| Space (hold) | Brush wheel |
| M | Mask painting |
| X | Mirror X |
| R | Voxel remesh |
| G | Transform gizmo on / off |
| L | Measure tool on / off |
| Y | Array tool on / off |
| (Lathe button) | Lathe tool on / off: the Unity app gives it no key; Enter creates, Esc puts it away |
| N | Mask tool: on (box), again for lasso, again for off |
| K | Split tool: on (line), again for lasso, box, then off |
| J | Keys tool on / off |
| Z, Shift + Z (also Ctrl + Z, Ctrl + Y) | Undo, redo |
| Ctrl + S, Ctrl + Shift + S, Ctrl + O | Save, save as, open |
| Tab | Clean view: nothing on screen but the model (Tab or Esc brings the panels back) |
| O | Spin the view round the model (turntable) |
| F11 | Fullscreen |
| Ctrl + = / Ctrl + - | Interface size |

Dragging on the background to orbit is the one addition to the Unity app's
controls; everything else matches it.

## Brushes

Each brush keeps its own Strength, Focal Shift and falloff; Size is shared by
them all.

- **Focal Shift** moves a brush's soft edge toward its centre (softer,
  pointier) or toward its rim (harder). The cursor's inner ring shows where
  the brush is at half strength, for every brush.
- **Custom Falloff**, at the bottom of a brush's settings, swaps that brush's
  own soft edge for a curve you draw: left is the brush centre, right is its
  edge, up is stronger. Drag a point; click to add one; right-click one, or
  drag it off the graph, to remove it. **Smooth**, **Sharp**, **Plateau** and
  **Linear** are shapes to start from. Focal Shift still applies on top (the
  amber line is what the brush then uses), and the inner ring follows.
- **Screen-Space Size**, under Focal Shift, starts on: the brush stays the
  same size on screen as you zoom. Off, the brush keeps one size on the model;
  the Size slider, S + drag and the scroll wheel then set that size, over a
  range that follows how big the model is.
- **Connected Only** (Move) grabs only the piece of surface under the pointer,
  so moving one finger leaves the finger next to it alone.
- Mask painting has **Mask Hardness** for its edge; the selected brush's
  falloff and Front Facing Only do not apply to it.

## Symmetry

The **Symmetry** card of the Sculpt panel makes every stroke in several
places at once: on both sides of a plane, all the way round an axis, or both.

- **Mirror strokes across** X, Y and Z, in any combination. **X** on the
  keyboard is Mirror X.
- **Radial** repeats every stroke evenly round an axis through the object's
  centre. **Radial Repeats** is how many times the stroke is made, itself
  included, from 2 to 32. **Radial Axis** is the object's own X, Y or Z (Y to
  start with: the way up of a vase or a bust, and the axis a lathed object is
  turned round), or **Custom**: a direction typed in as `x, y, z`, or taken
  with **From View**, which points the axis straight down the view, so that
  seen from where you are the repeats land evenly round the object's centre.
- Mirrors and Radial combine. Mirror X with six repeats about Y makes each
  repeat and its mirror image, twelve strokes in all; a mirror that the
  repeats already make (Z on top of X with an even count about Y) is not
  made twice.
- Where copies of a stroke land on top of each other, on a mirror plane or
  close to the radial axis, they are shared out rather than stacked: a stroke
  straight through the axis builds up about as much as the same stroke with
  no symmetry, however many repeats there are, and a Move there slides along
  the axis instead of being pulled every way at once.
- Every brush follows it, mask painting included, and so do the Mask, Hide,
  Trim and Split tools.
- **Show Symmetry Guides** draws a thin line on the model where each mirror
  plane cuts it and along the edge of each radial sector, the spokes meeting
  where the axis comes through the surface. They are in the axis's colour
  (red, green, blue; amber for a custom axis), and Tab hides them.

The settings are the scene's, in the selected object's own space: they move
and turn with the object. They are saved in the `.sclp` file.

## Transform gizmo

Press **G** (or the Transform button) to move, turn and resize the selected
object as a whole. Drag an arrow to move along it, a ring to turn about it, the
small cube on an arm to stretch that way, or the white cube in the middle to
resize evenly (up grows, down shrinks). Click another object to select it
(Shift or Ctrl + click adds it to the selection, or takes it out again);
dragging anywhere else orbits. Each drag is one undo step. **Reset Rotation
and Scale** in the Scene panel puts an object back square.

With several objects selected the gizmo sits at their shared centre, and they
move, turn and resize together about it. That too is one undo step.

**Gizmo axes** in the Transform card sets which way the arrows and rings
point: **Auto** (the object's own axes, or the world's when several objects
are selected), **Local** (always the bright object's own) or **Global**
(always the world's). The scale cubes stay on the object's own axes whatever
is chosen; when the arrows point another way, each cube stands on a thin stem
of its own.

If part of the object is masked, the gizmo reshapes the unmasked part instead
of moving the object: the masked part stays exactly where it is, and the soft
edge of the mask follows part of the way. Mask the body, then pull, turn or
stretch a limb out of it. It undoes like a brush stroke. This is for one
object at a time; several selected objects always move whole.

A tool can put the same gizmo on something of its own (a control point, a
copy): see `crates/toolkit/README.md`.

The view gizmo in the top-right corner turns the camera to the front, back,
left, right, top or bottom when a bubble is clicked (click the same one again
for the opposite side). Top and bottom always come up straight.

## SSpheres

Press the **SSpheres** button to block out a form with connected spheres, then
turn it into an object to sculpt.

1. Click in the viewport to place the first sphere (drag to size it). Over a
   model it rests on the surface; the ghost shows where it will go. **New
   SSphere Rig** (Objects card) does the same with one click, placing the
   first sphere where the camera is looking.
2. Drag off a sphere to pull a new one out of it. With a sphere selected, a
   click on empty space adds the next one of its chain. Each sphere's size
   sets how thick the form is there, and a see-through skin shows the surface
   the spheres make as you go.
3. **Make Skin** wraps that skin over the spheres and adds it to the scene as
   a new object, ready to sculpt, with its pivot in the middle of the skin.
   **Skin Detail** sets how many surface points it gets: more is smoother,
   and heavier to sculpt.

The skin does not follow the spheres afterwards. Change them and you need to
make a new skin; the earlier one is hidden, not deleted. So settle the overall
shape before sculpting detail.

| Input (SSpheres up) | Action |
| --- | --- |
| 1 2 3 4 | Draw, Move, Scale, Rotate: what a drag on a sphere does |
| Drag off a sphere (Draw) | Pull out a new sphere |
| Click empty space (Draw) | Add a sphere to the selected one |
| Drag a link (Draw) | Add a joint there and bend the limb |
| Drag a sphere (Move) | Move it with everything below it; Shift moves it alone |
| Drag a link (Move) | Pose the limb: it bends at the joint above, keeping every length |
| Drag (Scale) | Resize a sphere; Shift resizes its whole branch |
| Drag (Rotate) | Swing a branch about its parent joint; Shift twists it |
| Scroll over a sphere | Resize it |
| S + drag | Size of the next sphere |
| Right-click, or Ctrl + click, a sphere | Delete it and everything below it |
| Delete | Delete the selected sphere |
| Esc | Cancel a drag, else deselect |
| A | Solid preview of the skin |
| X | Symmetry across the red plane |
| Space (hold) | SSphere wheel, with the size of the next sphere under it |
| Z, Shift + Z | Undo, redo the spheres (they keep their own history) |
| F F | Frame the spheres |

Dragging on the background orbits, as everywhere else. The spheres mirror
across X by default: one placed on the red plane is shared by both sides, one
beside it gets a twin. They are a scaffold rather than an object, so they are
not written to `.sclp` files: make the skin before closing the app.

The spheres can be attached to an object (the **Attach** card). Attached,
they mirror across that object's own plane and go with it when it is moved,
turned or resized evenly; stretching it along one axis leaves them as they
are, since a sphere cannot stretch. A first sphere placed on a model attaches
to that model by itself; one placed in the open takes the selected object's
mirror plane without being attached. **Attach to Selected** and **Re-centre
Mirror Plane** move the red plane without moving any sphere, and **Detach**
lets go, leaving everything where it is. Each is one undo step.

## Measure

Press **L** (or the Measure button) to lay rulers on the model. They read real
lengths: one unit is 10 mm, so the starting sphere is a 10 mm ball.

| Input (Measure up) | Action |
| --- | --- |
| Drag on the model | New ruler from the press to the release |
| Click, then click | New ruler, one end per click (either may be off the model) |
| Drag an end | Re-measure from that end |
| Click a ruler | Select it: its X / Y / Z breakdown shows |
| Right-click a ruler, or Delete | Delete it |
| Shift while dragging | Lock the ruler to the nearest world axis |
| Ctrl while dragging | Do not snap to the surface |
| U | Millimetres / inches |
| Esc | Cancel, then deselect, then put the tool away |

Dragging on the background orbits. Rulers are a scaffold: not saved, and not
on the undo history. The **Measure & Size** card in the Scene panel is there in
every tool: it shows the selected object's real width, height and depth, and
typing a size into one ("50", "2in", "1 1/4in") resizes the object as one undo
step, evenly or along that axis alone.

## Array

Press **Y** (or the Array button) to repeat the selected object along a line,
round a circle or along a curve you draw. The copies are live: each is the
object's own mesh drawn again, so every brush stroke shows on all of them, and
moving, turning or resizing the object carries the whole array. They stay on
screen when you go back to sculpting.

1. **Line** or **Circle** makes an array in one click, spaced so the copies sit
   side by side. **Draw Curve**, then a drag in the viewport, makes the copies
   follow the curve you draw (a spine, a chain, a bracelet); end the stroke
   where it began to close it into a loop.
2. Reshape it with the handles. **Copies** sets how many, **End Size** tapers
   or grows the run, **Middle Size** swells or pinches it, and the ticks
   choose whether copies turn to follow the path, are spaced by size, or close
   into a loop. A circle also has the world axis it goes round and a Sweep.
3. **One Object** turns the copies into real geometry inside the object (a
   remesh then fuses them); **Separate Objects** makes each copy an object of
   its own. Either is one undo step, and Z gives the live array back. Large
   meshes are built on a background job, with the array on screen until it lands.

| Input (Array up) | Action |
| --- | --- |
| Drag a point | Move it, on the plane facing you |
| Drag the square start point | Move the object, and the whole array with it |
| Click the path | Add a point there, and keep dragging it |
| Shift + drag the last point | Extend the path from there |
| Right-click a point, or Delete | Delete it |
| Click another object | Switch to it (each object keeps its own array) |
| Esc | Cancel, then stop drawing, then deselect, then put the tool away |

Dragging on the background orbits (except while Draw Curve is on). Arrays are
the tool's, not the scene's: they are not saved in `.sclp` files yet, and while
the tool is up Z steps the array's own edits first, then the scene's. Not
here yet: clicking a copy to turn or move them all with the gizmo, **Copies
Back onto the Path**, and mirroring the array across the scene's centre.

## Lathe

Press the **Lathe** button to turn a solid like clay on a wheel. A starting
vase stands beside the model with its outline drawn over it: a curve through
round handles on both sides of a dashed axis. Drag a handle and the solid
follows live. **Create Mesh** (Enter) makes it a new object to sculpt, selected
and ready, in one undo step; Z right after takes it away again and brings the
lathe back with its profile. The profile is kept, so the next Create can be a
variation.

| Input (Lathe up) | Action |
| --- | --- |
| Drag a handle | Move that point (Shift: only along the dominant direction) |
| Drag the curve, or a flat end cap | Add a point there and keep dragging it |
| Click on empty space | Extend the outline from its nearer end (dropped near the axis, an end closes onto it) |
| Ctrl + drag | Sketch a whole new outline in one stroke (back to its start makes a ring) |
| Double-click a handle | Corner / smooth |
| Right-click a handle, or Delete | Delete it |
| Drag the dashed axis | Move the lathe |
| Enter | Create the mesh |
| Esc | Cancel the drag, then deselect, then put the tool away |

The panel starts from a preset (vase, bowl, cup, bottle, egg, ring) and holds
closed loop, cap open ends, even triangles, and the Density, Radius, Height and
Scale sliders; the preview is drawn at up to 256 around and Create always uses
the full Density. Dragging on the background orbits. The profile is a scaffold:
not saved, with its own Z while the tool is up.

## Mask, Trim, Split and Extract

Three tools act on what a shape drawn over the viewport covers, front and
back, straight through the model, and a fourth turns a masked area into a
piece of its own:

- **Mask** (**N**): drag a box or draw a lasso, and what it covers is masked.
- **Trim**: a box, a lasso or a straight line. What it covers is cut away and
  the hole closed with a face you can sculpt on; a line leaves a flat one.
- **Split** (**K**): the same cut, keeping both sides. The covered side becomes
  a new object ("Sphere Part") in the same place, and one undo brings the whole
  object back.
- **Extract**: the masked area as a new solid with thickness (a plate, a
  strap). A see-through shell shows what you will get while you set its
  thickness, offset and smoothing; **Accept** (Enter) makes it an object, Esc
  cancels.

| Input (Mask, Trim or Split up) | Action |
| --- | --- |
| Drag, anywhere on the viewport | Draw the shape; it acts when you let go |
| Ctrl + drag, or right drag | The reverse: unmask; trim keeps what the shape covers |
| Shift | Box, lasso: everything outside the shape. Line: turn it in steps of 15 degrees |
| Click | Mask: clear the mask |
| N / K | The next shape, then put the tool away |
| Space (hold) | Shape wheel |
| Esc | Cancel the drag, else put the tool away |
| 1 to 9 | Back to sculpting |

While Mask, Trim or Split is up, a drag on the background draws a shape
instead of orbiting, because a box or a cut line has to be able to start
beside the model: orbit with Alt + drag, pan with the middle button. (With
Extract up a drag orbits as usual.) A cut made with a mirror axis on repeats
on the other side, and with radial symmetry on, all the way round; a box or
lasso mask does the same. Each mask, trim, split and extract is one undo
step, and a mask is carried across a cut. On a large model a cut runs in the
background: its progress shows at the bottom of the viewport, with a Cancel
button.

## Keys

Press **J** (or the Keys button) to give two pieces of a split model joints
that register them for gluing: a peg on one piece and a matching socket in the
other, the socket bigger all round by a printing clearance (0.15 mm a side
unless changed). The tool takes the two selected objects, or the selected one
and the piece whose cut face touches it, and says which in the status line. It
plans the joints itself (two round keys, or one keyed shape, where the face is
widest and the piece behind it deep enough), shows them as orange pegs on the
cut face, and leaves them to be changed before anything is built.

| Input (Keys up) | Action |
| --- | --- |
| Click the cut face | Add a joint (keep the button down to place it) |
| Drag a joint | Move it; red means it will not fit there and is left out |
| Right-click a joint, or Delete over it | Remove it |
| Scroll over a joint | Turn it 15 degrees (Shift: 5); R turns it 90 |
| V | Show the half with the pegs, the half with the sockets, or both |
| Enter | Build the joints into both pieces |
| Z, Shift + Z | Undo, redo a joint edit |
| Esc | Let go of a joint, then stop a build, then put the tool away |

Dragging off the cut face orbits. The panel picks the key's shape (circle,
square, rectangle, hexagon, triangle, D, cross, star, dovetail, puzzle tab),
its size and height (or Auto, from the face), how many per cut, which piece
gets the pegs, and under **Fit** the clearance, extra socket depth, chamfers,
draft and minimum wall, all in millimetres. **Build Keys** changes both pieces
in one undo step, in the background on a large model. The planned joints are a
scaffold: not saved, kept when the tool is put away, and handed back when the
build is undone, so Z then J is how built keys are changed. The pieces must
still be touching when the tool first comes up, since that is how it finds the
pair; after that they can be moved apart and the joints stay on them.

## Files

- **Save / Open** use `.sclp`, which keeps the whole sculpt: every object
  (its exact mesh and mask, hidden faces, quads, name, position and
  visibility; instances stay linked) and how it was left: the material and
  finish, lighting, HDRI, background and effects, the camera view, the
  mirror axes and radial symmetry. Reopening a file puts all of it back. (An
  HDRI is kept as the path of its picture, not the picture.)
- **Saving never harms the file already there.** The new file is written in
  full beside the old one and only then takes its place, so a full disk or a
  crash part-way leaves the old file as it was.
- **Open** asks how the file should come in: in place of the scene, or with
  its objects added to it. Either is one undo step. If the scene has changes
  that are not saved it says so, and offers to save first. So does closing the
  app (the window's close button, Alt + F4 or Ctrl + Q); with nothing unsaved
  it just closes. "Unsaved" means something was sculpted, added, moved or
  undone since the last save or open; changing only the look or the view does
  not count.
- **Recent**: the File card shows the last six sculpts saved or opened, each
  with a picture of the model taken when it was saved. Click one to open it.
  A file that has been moved or deleted drops out of the tiles.
- **Import** adds an `.obj` or `.stl` model to the scene, centred, or the
  objects of another `.sclp` where they were saved. An STL comes
  in at its real size (millimetres, Z up; one unit is 10 mm, so a 10 mm ball is
  one unit across), welded into one connected surface, with inside-out
  triangles turned the right way. An OBJ is fitted to the size of the first
  object, as in the Unity app.
- **Export OBJ / Export STL for Printing** write the selected object where it
  stands in the scene, with its position, rotation and scale baked in, so an
  instance exports as its own placed copy. The STL is binary, in millimetres,
  turned upright for a slicer (the model's up is the bed's Z, its front faces
  the front of the bed, nothing mirrored). The OBJ keeps the app's own units
  and axes, and a quad-remeshed object goes out as its quads. The save box
  offers `<object name>_<date>_<time>` as the file name. The size conversion
  lives in one place, `crates/geom/src/units.rs`.
- **The app remembers how you left it**: interface size, tooltips on or off,
  which cards are open, fullscreen, millimetres or inches, the remesh
  settings and every brush's tuning. They are kept in
  `%APPDATA%\Sculpt\settings.json`, with the recent files; deleting that file
  puts everything back to how the app first starts.

A `.sclp` is the meshes as raw arrays (so a heavy scene saves and loads as
fast as the disk allows) followed by one JSON section for everything else.
Files from earlier versions of the app still open. The layout, and how to
keep something new in it without changing the format, are set out in
`src/io.rs`.

## Look

How the model is shaded is in three cards of the Scene panel. The shaders and
numbers are the Unity app's, so the same settings give the same picture.

- **Material**: a matcap, or (Matcap off) a lit surface. Lit surfaces are a
  plain base colour with Metallic and Smoothness, or a **Surface Finish**:
  Lure Plastic (translucent, with glitter), Metal (rust, patina, antique
  silver and gold, worn steel, pewter), Clay or Wood. Pick a preset, then
  adjust it with the sliders. **Glow** adds light the surface gives off (lava
  cracks, neon, veins and so on) over any of them, matcaps included.
  **Import Matcap...** adds a matcap picture of your own (.png or .jpg; it is
  copied into `assets/matcaps/Imported`), and **Rescan Folder** finds pictures
  you have put in `assets/matcaps` by hand.
- **Lighting**: eight presets (Studio, Soft Clay, High Key, Vibrant,
  Cinematic, Golden Hour, Moody, Dramatic) as a 3-point or 5-point rig, with
  Brightness, Rotation, Follow Camera and Shadows. **Load...** lights the
  scene from an HDRI picture (.hdr, .exr, .png, .jpg) instead of the preset's
  sky colours; the preset's lights still shape the form on top of it.
- **Backdrop & Effects**: a flat, gradient or HDRI background, bloom,
  vignette, depth of field, saturation and contrast, the tone curve and
  ambient occlusion. **Depth of Field** keeps what is at the Focus Distance
  sharp and blurs what is nearer or farther, as a camera lens does; a lower
  Aperture blurs more. It is a close-up effect: with the focus far away
  (it starts at 10) there is next to nothing to see, as in the Unity app.

A matcap carries its own lighting, so the lights, shadows and finishes only
show with Matcap off. All of these settings are saved in the `.sclp` file and
come back when it is opened (see Files).

## Objects

The **Objects** card at the top of the Scene panel is the scene graph.

- **Add** puts a new cube, sphere, cylinder, capsule, cone or wedge at the
  centre and selects it. It arrives a little smaller than the top object of
  the list (nine tenths), and is named by count: Cube, Cube (1), Cube (2).
  The cylinder and the capsule stand twice as tall as they are wide, as the
  Unity app's do.
- Click a row to select that object; brushes work on the selected one, and
  on any instance of it: sculpt whichever copy is under the pointer.
- Ctrl + click (or Shift + click) more rows to select several. The bright
  row is the one brushes work on; the grey ones are selected with it. Frame
  fits them all, and the hide keys act on them all.
- **Join** merges the selected objects into the bright one as a single mesh,
  masks included. **Remesh after Join** then welds the shells into one
  surface (and clears the mask, as any remesh does).
- **Subtract**, **Union** and **Intersect** work on the same selection. The
  bright object is the one kept: Subtract cuts the others out of it, Union
  welds them into it as one solid with nothing left inside the overlap, and
  Intersect keeps only what it shares with all of them. The others are
  hidden afterwards, not deleted (their eye shows them again), unless
  **Delete the others instead of hiding** is ticked. The result is
  rebuilt at **Voxel Resolution**, counted along the kept object's longest
  side, and starts with a clear mask.
- **Reset Shape** puts the selected object back to the shape it had before
  sculpting: as it was made, loaded or last remeshed.
- The eye shows or hides an object, the bin deletes it. Hiding the object
  you are working on moves the brushes to one you can still see.
- **H** hides the selection, **Shift + H** everything else, **Alt + H** shows
  everything. **/** is Local View: only the selection, framed; press it
  again and the rest comes back and the camera glides to where it was.
- The **Delete** key deletes the selected object after asking.
- Double-click a name to rename it (or use the field under the list).
- Press and hold a row and it lifts out of the list; drag it up or down and
  let go to move it. Near the top or bottom edge a long list scrolls.
- **Clone** makes an independent copy. **Instance** makes a linked copy that
  shares the mesh (shown in blue): sculpt one and all change. **Make Unique**
  unlinks one.
- Copies start on top of the original; **Position** and **Scale** place the
  selected one by numbers, and the Transform gizmo by hand.
- Every one of these is an undo step, a Join or a boolean included (one
  step brings the separate objects back, shown again if they were hidden). The hide keys and Local View are not, as in the
  Unity app: undo steps past them and leaves what they hid hidden.

## Hidden parts

Part of an object can be switched off to get at what is behind it. A hidden
part is not drawn, the pointer goes straight through it to whatever is
behind, and brushes leave it alone; the edge of what still shows sculpts like
any other surface. Nothing is cut away.

- **Show All** brings everything back and **Invert Visible** swaps the hidden
  and the showing parts. Both are in the **Hidden Parts** card of the Sculpt
  panel, under Masking; its header says how many faces are hidden.
- Hiding, showing and inverting are each an undo step.
- Remesh, Quad Remesh, Join and both exports always work on the whole model,
  hidden parts included, and what a remesh or a join makes starts with
  everything showing, as in the Unity app.
- The wireframe draws only the faces that show.

Choosing what to hide is the job of the box and lasso tools.

## Turntable

Press **O** (or **Spin** in the Turntable card of the Scene panel) and the
view goes round the model, easing in and out. The camera turns, not the
model, so nothing about the scene changes. **Speed** is 5 to 180 degrees a
second and **Reverse Direction** goes the other way. Alt + drag still orbits
while it spins, and the spin carries on from wherever you leave the view.

**Tab** is the clean view: the panels, the gizmo, the symmetry guides, the
brush ring and the SSphere spheres all go, and for a moment a line at the
bottom says how to get back (Tab or Esc). It is for looking: strokes and the
single-key shortcuts are switched off, a drag orbits, and the pointer hides
once it has rested for a second and a half.

**Record 360° Loop** saves exactly one turn, starting from the view as it is
and at the Speed above, as numbered PNG pictures (`frame_0000.png`...) in a
folder of their own under `Videos\Sculpt`. The last picture flows straight
back into the first, so it plays as a seamless loop. Pick the shape (16:9,
1:1, 9:16 or 4K) and 24, 30 or 60 frames a second first; the line under the
buttons says how many pictures that makes. The pictures are drawn at that size
whatever size the window is, with no panels or guides in them, and a square or
upright picture gets a wider lens so nothing at the sides is cropped. The
window shows progress while it records; **Cancel** or Esc stops it and removes
what was written. The pictures are large (about 2 MB each at 1080p).

## Timelapse

**Start Timelapse** (the Timelapse card of the Scene panel) records a sped-up
video of your sculpting, and **Stop Timelapse** saves it as an MP4 in
`Videos\Sculpt`. Only the time the model is actually changing is kept:
thinking, orbiting, and undo and redo are left out. **Speed** then squeezes
what is left (8x plays a minute of sculpting in 7.5 seconds).

The video has a camera of its own. **Follow My View** films from your own
angle, leaning toward the spot you are working on, and smooths your moves into
a steady shot; **Turntable Orbit** circles the model and ignores your camera.
**Camera Smoothing** and **Idle Grace** (how long it keeps recording after the
last change, so gaps between strokes do not become cuts) can be changed while
recording; the size and frames per second are fixed once it starts. Recording
does not slow the brush: frames are compressed on a thread of their own, and
if that ever falls behind, a frame is dropped. Closing the app while recording
still leaves a finished video. Needs Windows (it uses the video encoder
Windows comes with).

## What is here, and what is not yet

Ported from the Unity app: the nine brushes above with their settings, mask
painting and mask operations, mirror symmetry on X / Y / Z, voxel remesh, quad remesh,
undo / redo, matcaps, the lit materials and finishes, lighting presets, HDRI
lighting and post effects (see Look below), cavity, the view gizmo, the brush
wheel, the two-panel interface, and multiple objects with the scene graph
(add shapes, show / hide, rename, clone, instance, reorder), and SSpheres
(sphere blockout with a live skin, symmetry, posing, attaching to an object,
and Make Skin).

Quad remesh is ported too: **Quad Remesh** in the Remesh card rebuilds the
selected object as about the number of quads on the **Quads** slider, even in
size and following the form. It is one undo step, and OBJ export then writes
the quads themselves. The **Wireframe** tick box at the bottom of the viewport
draws the edges of every face (the quads after a quad remesh, the triangles
otherwise); a quad remesh turns it on. The result ignores mirror symmetry. The
quads are kept in `.sclp` files.

Not ported yet: the Pose brush, radial symmetry, dynamic topology, box / lasso hide (listed in the Mask tool, greyed out), the gizmo on an SSphere,
live mirror copies and tubes.
The turntable and the timelapse are ported (see Turntable and Timelapse above);
a 360 loop is saved as PNG pictures only, not yet as a video.

Left out on purpose: the Unity app's mold maker and hard-surface tool.

## For development

```
cargo test --release          # engine tests
sculpt.exe --bench            # engine timings at 200k / 1M / 4M triangles
sculpt.exe --screenshot out.png --size 1600x960 --demo   # scripted strokes (SCULPT_DEMO_BRUSH=focal=-0.8,curve=plateau,fixed sets the brushes up first)
SCULPT_DEMO_BRUSH=radial=6,axis=z,mirror=off sculpt.exe --screenshot out.png --demo   # the same strokes six times round Z, seen down the axis (axis=x | y | z; leave mirror=off out for Mirror X as well)
sculpt.exe --screenshot out.png --demo --demo-hide   # half of the model hidden, cut on a slant
sculpt.exe --screenshot out.png --demo-scene   # one of each shape + instances
sculpt.exe --screenshot out.png --demo-select  # the same with three objects selected (SCULPT_DEMO_SELECT=ask | local | subtract | union | intersect | play for more)
sculpt.exe --screenshot out.png --demo --look "panels=off,matcap=off,finish=metal:rust,light=moody"
sculpt.exe --screenshot out.png --demo-spheres rig   # SSphere figure: rig | preview | skin | back | again | attached | followed | followed-skin | wheel | new | play
sculpt.exe --screenshot out.png --demo-gizmo   # the gizmo on a turned object (SCULPT_DEMO_GIZMO=global | local | three | three-turn | masked | play for more)
sculpt.exe --screenshot out.png --demo-tool measure  # a hosted tool: id or id:what (measure:inches, array:curve, keys:applied, studs:job, any:play)
sculpt.exe --screenshot out.png --demo --clean-view  # the Tab view with its hint (SCULPT_DEMO_CAMERA=turntable | timelapse | recording shows a card or the recording bar instead)
sculpt.exe --record-loop folder --demo               # writes a short 360 loop and closes: 12 pictures at 640x360 (--loop-size 1080x1920 --loop-frames 24 for others)
sculpt.exe --demo-timelapse out.mp4                  # sculpts a few scripted strokes while recording, prints what the file holds, closes
SCULPT_DEMO_CAMERA=check sculpt.exe --record-loop folder --demo   # presses the turntable's own keys and buttons and prints a line per check
sculpt.exe --screenshot out.png --demo --demo-files recent  # the File card's recent tiles; or ask-exit | ask-open | ask-open-clean | play
```

A run started with any `--option` is a scripted one: it neither reads nor
writes the user's settings file, so a visual check never depends on how the
app was last left and never leaves its brush settings behind. Set
`SCULPT_SETTINGS_DIR` to a folder to give such a run settings of its own.

Tools are added as crates against `sculpt-toolkit`, not wired into the app:
see `crates/toolkit/README.md`.

### Claude bridge

Claude Code sessions opened in this folder get a `sculpt` MCP server (set up in
`.mcp.json`), which lets Claude see and drive the running app: start and close
it, read its state, take screenshots, set the brush, draw strokes, move the
camera, manage objects, and open, save and export files. Everything it changes
is an ordinary undo step.

There are two halves. The app listens on `127.0.0.1:47823` (`src/app/bridge.rs`;
this machine only, and only the first copy of the app if several are open).
`sculpt.exe --mcp` (`src/mcp.rs`) is the server Claude talks to, and passes
each tool call on to the app. `Sculpt-mcp.bat` starts that server from a copy
of the program in `target\mcp`, so a connected session never blocks a rebuild.
A new tool needs its description in `mcp.rs` and its code in `bridge.rs`.
Set `SCULPT_BRIDGE_PORT` to use another port.

Matcap images in `assets/matcaps` keep their original credits files. Icons in
`assets/icons` come from the Unity project's `Design/Icons`.
