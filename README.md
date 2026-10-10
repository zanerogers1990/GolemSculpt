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
| Ctrl + drag, or right drag | Sculpt inverted (Pose: twist the limb, or slide it) |
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
| 1 2 3 4 5 6 7 8 9 0 | Move, Clay, Smooth, Crease, Inflate, Flatten, Pose, Standard, Layer, Snake Hook |
| S / F / D + drag | Brush size / strength / focal shift |
| Space (hold) | Brush wheel |
| M | Mask painting |
| X | Mirror X |
| R | Voxel remesh |
| G | Transform gizmo on / off |
| L | Measure tool on / off |
| Y | Array tool on / off |
| (Lathe button) | Lathe tool on / off: the Unity app gives it no key; Enter creates, Esc puts it away |
| T | Tube tool on / off: Enter fuses the tubes into the model, Esc puts it away |
| N | Mask tool: on (box), again for lasso, again for off |
| (Hide button) | Hide tool: a box or lasso hides part of the selected object; it has no key, as H hides whole objects |
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

### Pose

**Pose** (**7**) bends a limb the way you would pose a figure, with nothing to
mask or set up first. It works as the Pose brush in Blender does.

- Rest the pointer on a limb and a white line shows what a press would do. It
  runs from the pointer to a dot with a ring round it: the **joint**. What is
  on the pointer's side of the joint is the limb that will move, hand and all;
  what is past it stays where it is.
- **The brush size places the joint.** It sits in the middle of the limb where
  the brush ring crosses it, on the side toward the body, so a bigger brush
  bends the limb further up. Scroll, or hold S and drag, and the line follows
  as the ring grows. The ring has to reach right round the limb: with the
  pointer on one side of an arm it must take in the far side too. A smaller
  one only gets hold of a patch of skin, and turns that patch and nothing
  else (the line shows it: the joint sits right under the pointer).
- Drag, and the limb turns about the joint to point at the pen. **Ctrl + drag**
  (or the right button) twists it about its own length, the skin under the pen
  going round with the pen.
- **Segments (Joints)**: with more than one, a chain of joints runs back along
  the limb, each a brush size from the last, and the limb reaches for the pen
  like an arm: the point you took hold of follows the pen, every part keeps
  its length, and the last joint (the ringed one) stays put.
- **Joint Offset** pushes the joint further up the limb, past the brush ring,
  by that many brush sizes: for a hand or a face, where a brush small enough
  to pick out one part would put the joint too close to it.
- **Rigidity** sets how sharply a joint bends, from a long soft curve to a
  hinge. Focal Shift and Custom Falloff shape the bend itself: the curve's
  left end is the limb's side of the joint, its right end the side that stays.
- **Resize** makes the limb bigger or smaller about the joint as you drag away
  from the joint or toward it (Ctrl + drag slides it without turning).
  **Stretch** makes it longer and thinner, or shorter and fatter, keeping its
  volume. Both work about one joint.
- Masked surface stays where it is, hidden parts are left alone, and one drag
  is one undo step. Strength and pen pressure do not apply: the limb follows
  the pen exactly.
- With a mirror on, each side gets a joint of its own, so both arms bend
  together. A limb that lies on the mirror plane itself (a neck, a tail, the
  trunk) can only bend within the plane, and will not twist: dragged to one
  side it stays put, which is the mirror keeping the model symmetric. Turn the
  mirror off to tilt a head.

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
| Drag an arrow (Move) | Move the selected sphere and everything below it along that axis |
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

In Move mode the selected sphere carries the Transform gizmo's arrows: drag one
to slide the sphere, and everything below it, exactly along that axis of the
world, for the times a limb has to go straight up or straight out. The mirrored
twin follows, a sphere near the red plane sticks to it as it does for any
other move, and each pull is one step in the spheres' own undo history (Esc
cancels it). Pressing on a sphere itself, wherever an arrow runs over it, still
moves that sphere freely. A sphere gets arrows only, no rings or cubes: it is
round, and its size and turn are what the Scale and Rotate modes are for. The
arrows go when the mode changes, when the sphere is deselected or deleted, in
the solid preview, and when the tool is put away.

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
3. **Mirror to the other side** (X, Y, Z) repeats the object and every copy
   on the other side of the scene's centre. It is the object's own live Mirror
   (the Mirror section of the Objects card, see Objects) within reach: the
   same switch, so it stays live, and an object that is already mirrored has
   its array mirrored from the start. With X and Z ticked there are three
   reflections of each copy, as there are of the object.
4. **One Object** turns the copies into real geometry inside the object (a
   remesh then fuses them); **Separate Objects** makes each copy an object of
   its own. Either is one undo step, and Z gives the live array back. Large
   meshes are built on a background job, with the array on screen until it lands.
   A mirrored array stays mirrored: the object keeps its mirror, and each
   separate object gets the same live mirror.

| Input (Array up) | Action |
| --- | --- |
| Drag a point | Move it, on the plane facing you |
| Drag the square start point | Move the object, and the whole array with it |
| Click the path | Add a point there, and keep dragging it |
| Shift + drag the last point | Extend the path from there |
| Right-click a point, or Delete | Delete it |
| Click a copy (on either side of a mirror, or the object) | The gizmo's arrows and rings sit on it |
| Drag the gizmo on a copy | Turn or move them all |
| Click another object | Switch to it (each object keeps its own array) |
| Esc | Cancel, then stop drawing, then let go of the point or the copy, then put the tool away |

**Posing the copies.** Click a copy (off the path: a click on the path adds a
point) and the gizmo's arrows and rings sit on it; turning or moving that one
turns or moves them all. Turning it turns the object, and every copy turns the
same way in place, each in its own frame on the path. Moving it moves the
object off the path's start and leaves the path where it is in the world, so
every copy moves the same way in its own frame (turned with the path, scaled
with its size): a row of links all lifted off a curve, or pushed out from it.
On a mirrored array a copy on the far side can be picked too: dragging it right
moves the original left, and a turn is turned the other way. The gizmo has no
cubes, since a copy's size is set with the sliders. **Copies Back onto the
Path** (it lights up once copies are off the path) undoes any move made this
way and leaves a turn.

Dragging on the background orbits (except while Draw Curve is on). Arrays are
the tool's, not the scene's: they are not saved in `.sclp` files yet, and while
the tool is up Z steps the array's own edits first, then the scene's. The
exceptions are a pose, Copies Back onto the Path and a mirror switch: each is a
step of the scene's history (it changes the object, or its mirror), but Z
takes it back in order with the array's own edits, and the array follows when
the scene takes one back, even with the tool away. Moving the object by its
square start point is a step of the scene's alone, which Z reaches after the
array's own edits.

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

## Tube

Press **T** (or the Tube button) to draw tubes along a stroke: horns, arms,
legs, tails, veins. A tube stays live after you draw it, as a curve through
round points that you can move, add to and thicken, with the solid following
as you work. When it is right, fuse it into the model or make it an object of
its own.

1. Drag from the model, out into the air, to draw a tube. The stroke is laid
   on a plane facing you through the spot where it began, so the tube grows
   out of that spot. **On Surface** lays it along the model instead, half sunk
   into it (a vein, a ridge, a tail draped along a body). The ring at the
   pointer shows how thick the next tube will be. With a mirror axis on, one
   stroke makes both: the twin's curve and points are drawn fainter, and
   grabbing one of its points edits the tube through the mirror.
2. Shape it by its points, the dots along its orange curve; press one to
   select it (it turns orange). Drag a point, click the curve to add one,
   double-click a point to make it a sharp corner (it turns square) or smooth
   again, Shift + drag an end point to carry the tube on from there. Each
   point can be made thicker or thinner. The panel sets the whole tube:
   **Radius**, how thick; **Taper**, which narrows it toward its end, for a
   horn or a claw (**Thick End at the Tip** turns that round, so the start of
   the stroke narrows instead); **Smoothness**, how loosely it flows between
   its points; and **Round** or **Flat** ends. These change the selected tube,
   and the next one you draw.
3. **Merge** (Enter) fuses every live tube into the object it was drawn on, as
   one surface with a soft join; **Blend** sets how soft (0 is a crisp seam).
   The object is rebuilt at its current density, as a remesh does, so it
   starts with a clear mask and everything showing. The work runs in the
   background, with its progress and a Cancel button (or Esc) while it does.
   **Keep Separate** makes each tube an object of its own to sculpt, placed
   with the object it was drawn on. Either is one undo step.

| Input (Tube up) | Action |
| --- | --- |
| Drag from the model | Draw a tube (Ctrl + drag starts one from empty space, at the depth of the selected object) |
| Drag a point | Move it (a point on the surface slides over the model) |
| Shift + drag an end point | Extend the tube from that end |
| Click the selected tube | Add a point there, and keep dragging it |
| Click another tube | Select it |
| Double-click a point | Sharp / smooth |
| Right-click a point | Delete it |
| Delete | Delete the selected point, else the selected tube |
| Drag the square beside the selected point | Thicker / thinner there |
| Scroll over a point | The same |
| S + move sideways | Thickness of the point under the pointer, else of the tube |
| Enter | Merge |
| Z, Shift + Z | Undo, redo the tubes' own edits |
| Esc | Cancel the drag, then deselect, then put the tool away |

Dragging on the background orbits, as everywhere else (the Unity app drew a
tube there; here that is Ctrl + drag). Live tubes are a scaffold: not saved in
`.sclp` files, and drawn only while the tool is up. Put it away and they wait,
out of sight, until it is brought back. While the tool is up Z steps the
tubes' own edits first (a point moved, a slider changed), then the scene's.
Merge and Keep Separate are steps of the scene's history: Z right after one
gives the tubes back live, with the tool brought up if it was put away, and
Shift + Z fuses them again. A tube thinner than the model's own detail comes
out thicker than drawn: undo, remesh the model finer, and merge again.

## Mask, Hide, Trim, Split and Extract

Four tools act on what a shape drawn over the viewport covers, front and
back, straight through the model, and a fifth turns a masked area into a
piece of its own:

- **Mask** (**N**): drag a box or draw a lasso, and what it covers is masked.
- **Hide**: drag a box or draw a lasso, and the part of the selected object it
  covers is hidden, to get at what is behind it (see Hidden parts). It has no
  key, since H hides whole objects: use the Hide button.
- **Trim**: a box, a lasso or a straight line. What it covers is cut away and
  the hole closed with a face you can sculpt on; a line leaves a flat one.
- **Split** (**K**): the same cut, keeping both sides. The covered side becomes
  a new object ("Sphere Part") in the same place, and one undo brings the whole
  object back.
- **Extract**: the masked area as a new solid with thickness (a plate, a
  strap). A see-through shell shows what you will get while you set its
  thickness, offset and smoothing; **Accept** (Enter) makes it an object, Esc
  cancels.

| Input (Mask, Hide, Trim or Split up) | Action |
| --- | --- |
| Drag, anywhere on the viewport | Draw the shape; it acts when you let go |
| Ctrl + drag, or right drag | The reverse: unmask; show what was hidden; trim keeps what the shape covers |
| Shift | Box, lasso: everything outside the shape. Line: turn it in steps of 15 degrees |
| Click | Mask: clear the mask. Hide: show everything |
| N / K | The next shape, then put the tool away |
| Space (hold) | Shape wheel (Mask adds Clear and Invert, Hide adds Show All and Invert) |
| Esc | Cancel the drag, else put the tool away |
| 1 to 9 | Back to sculpting |

While Mask, Hide, Trim or Split is up, a drag on the background draws a shape
instead of orbiting, because a box or a cut line has to be able to start
beside the model: orbit with Alt + drag, pan with the middle button. (With
Extract up a drag orbits as usual.) They work on the selected object. With a
mirror axis on, a mask or a hide covers the other side too, and a cut repeats
there. Each mask, hide, trim, split and extract is one undo step, and a mask
is carried across a cut. On a large model a cut runs in the background: its
progress shows at the bottom of the viewport, with a Cancel button.

Every place the selected object is drawn counts, as in the Unity app: a shape
drawn over one of its **live mirror copies** (see Objects), or over an
instance of it, or over a mirror copy of an instance, acts on the object
through that place. A copy is the object's own mesh drawn again, so a box
over the arm on the far side of a mirrored figure masks, hides, trims or
splits the arm of the one mesh, and it goes on both sides at once. A shape
that covers several places at once (a wide box, or a line, which has no end)
acts through all of them. A split leaves the object where it was, with its
mirror, cut; the new "Part" is a plain object in the object's own place with
no mirror of its own, since a piece cut off is not a mirrored object (as in
the Unity app; the mirror of the part is a Mirror tick away). The copies of an
Array (the Array tool's, not the scene's) are not under these tools yet.

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
  visibility; instances stay linked; live mirrors stay on) and how it was left: the material and
  finish, lighting, HDRI, background and effects, the camera view and the
  mirror axes. Reopening a file puts all of it back. (An HDRI is kept as the
  path of its picture, not the picture.)
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
  and axes, and a quad-remeshed object goes out as its quads. The live mirror
  copies of an object are part of the model on screen and go out with it: the
  STL holds them all in one file, the OBJ writes each as an object of its own
  ("Cube_MirrorX"). The save box
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
- **Mirror** (its own section, under Selected Object) gives the selected
  object live copies of itself across the world's planes, through the world's
  centre: tick X, Y and / or Z and press **Mirror**. There is one copy for each
  combination of the ticked axes (X and Z make three: X, Z and XZ). A copy is
  the object's own mesh drawn again, so every stroke, remesh or cut shows on
  all of them, and moving, turning or resizing the object carries them with
  it. Either side can be worked on: sculpt through whichever copy is under the
  pointer, and with the Transform tool a click on a copy (or one of the
  buttons under **The Transform gizmo is on**) puts the gizmo on that copy,
  where dragging it moves the original the other way. A tick on an object that
  is already mirrored changes its mirror at once, **Remove** takes the copies
  away, and **Validate** turns them into objects of their own ("Cube Mirror
  X"), exactly where they were shown. Each is one undo step. Mirrored objects
  are amber in the list. The copies are framed with the model, saved in `.sclp`
  files and written by the exports. A clone or an instance of a mirrored object
  is not mirrored. The Mask, Hide, Trim and Split tools see the copies too: a
  box, lasso or line drawn over a copy acts on the object through that copy
  (see the section on those tools below).
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

- The **Hide** button brings up the Hide tool (no key: H hides whole objects).
  Drag a box or draw a lasso over the part to hide: what it covers goes, front
  and back, straight through the selected object. A face is hidden when all
  three of its corners are under the shape, so a face on the edge stays.
  Ctrl + drag (or a right drag) shows what the shape covers instead, Shift +
  drag hides everything outside it (to isolate a limb), and a click shows
  everything again. With a mirror axis on, the other side goes too, and a box
  or lasso drawn over a live mirror copy of the object hides through that
  copy. A drag
  that would hide the whole object is refused, with a line saying so. A box or
  lasso mask leaves hidden parts alone, so hide a limb first and a mask drawn
  over the body in front of it will not reach it.
- **Show All** brings everything back and **Invert Visible** swaps the hidden
  and the showing parts. Both are in the **Hidden Parts** card of the Sculpt
  panel, under Masking, and in the Hide tool's own panel and wheel; the card's
  header says how many faces are hidden.
- Hiding, showing and inverting are each an undo step.
- Remesh, Quad Remesh, Join and both exports always work on the whole model,
  hidden parts included, and what a remesh or a join makes starts with
  everything showing, as in the Unity app.
- The wireframe draws only the faces that show.

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
and at the Speed above, as an **MP4** video in `Videos\Sculpt`
(`Turntable_<date>_<time>.mp4`), ready to post or to play round and round. Pick
the shape (16:9, 1:1, 9:16 or 4K) and 24, 30 or 60 frames a second first; the
line under the buttons says how long the loop is, how many frames it holds and
what it will be saved as. The frames are drawn at that size whatever size the
window is, with no panels or guides in them, and a square or upright picture
gets a wider lens so nothing at the sides is cropped. The window shows progress
while it records, and afterwards the card says what the file is called and
where it went. **Cancel** or Esc stops it and removes what was written;
closing the app part-way does the same. Until it is finished the video is
called `Turntable_<date>_<time>.recording.mp4`, so a half-written file is never
mistaken for a loop.

It is a true loop: the video holds one picture for each step of the turn and
no more, so its frames add up to exactly the turn's length (a 12 second turn at
30 frames a second is 360 frames, 12.000 seconds) and the last frame flows into
the first with the same step as any other two, with no repeated frame at the
join. No frame is ever dropped: if the encoder is behind, the recording waits
for it.
The video needs Windows (it uses the video encoder Windows comes with); the
card offers it only there.

**PNG frames**, the second choice beside **MP4**, saves the same turn as
numbered pictures (`frame_0000.png`...) in a folder of their own under
`Videos\Sculpt`, for compositing or a GIF tool. They are large (about 2 MB each
at 1080p). The Unity app offered the same two choices, with the video as the
default.

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

Also ported: tubes (see Tube above) and box / lasso hide (see Mask, Hide,
Trim, Split and Extract, and Hidden parts, above). A tube is drawn along a
stroke, shaped by its points, and fused into the model or kept as an object of
its own; the Hide tool hides part of an object with a box or lasso.

Quad remesh is ported too: **Quad Remesh** in the Remesh card rebuilds the
selected object as about the number of quads on the **Quads** slider, even in
size and following the form. It is one undo step, and OBJ export then writes
the quads themselves. The **Wireframe** tick box at the bottom of the viewport
draws the edges of every face (the quads after a quad remesh, the triangles
otherwise); a quad remesh turns it on. The result ignores mirror symmetry. The
quads are kept in `.sclp` files.

Not ported yet: the Pose brush, radial symmetry, dynamic topology, the gizmo
on an SSphere and live mirror copies.
The turntable and the timelapse are ported (see Turntable and Timelapse above);
a 360 loop is saved as PNG pictures only, not yet as a video.

Left out on purpose: the Unity app's mold maker and hard-surface tool.

## For development

```
cargo test --release          # engine tests
sculpt.exe --bench            # engine timings at 200k / 1M / 4M triangles
sculpt.exe --screenshot out.png --size 1600x960 --demo   # scripted strokes (SCULPT_DEMO_BRUSH=focal=-0.8,curve=plateau,fixed sets the brushes up first)
sculpt.exe --screenshot out.png --demo --demo-hide   # half of the model hidden, cut on a slant
sculpt.exe --screenshot out.png --demo-scene   # one of each shape + instances
sculpt.exe --screenshot out.png --demo-select  # the same with three objects selected (SCULPT_DEMO_SELECT=ask | local | subtract | union | intersect | play for more)
sculpt.exe --screenshot out.png --demo --look "panels=off,matcap=off,finish=metal:rust,light=moody"
sculpt.exe --screenshot out.png --demo-spheres rig   # SSphere figure: rig | preview | skin | back | again | attached | followed | followed-skin | wheel | new | play | gizmo | gizmo-held | gizmo-play
sculpt.exe --screenshot out.png --demo-mirror  # the sphere off centre, its live mirror on, strokes laid on the copy (SCULPT_DEMO_MIRROR=xz | gizmo | validate | front | back | play)
sculpt.exe --screenshot out.png --demo-pose bend     # a figure posed with the Pose brush: hover | bend | chain | twist | resize | slide | stretch | done | wheel | play (bend:close moves the camera in; :soft and :offset change those settings)
sculpt.exe --screenshot out.png --demo-gizmo   # the gizmo on a turned object (SCULPT_DEMO_GIZMO=global | local | three | three-turn | masked | play for more)
sculpt.exe --screenshot out.png --demo-tool measure  # a hosted tool: id or id:what (measure:inches, array:curve, keys:applied, studs:job, any:play)
sculpt.exe --screenshot out.png --demo-tool trim:mirror   # a shape drawn over the object's live mirror copy: also hide:mirror, mask:mirror, split:mirror
sculpt.exe --screenshot out.png --demo-tool array:mirror  # a mirrored column of copies; array:copy picks one (the gizmo on it), array:posed moves and turns it, array:onpath puts the move back
sculpt.exe --screenshot out.png --demo --clean-view  # the Tab view with its hint (SCULPT_DEMO_CAMERA=turntable | timelapse | recording shows a card or the recording bar instead)
sculpt.exe --record-loop folder --demo               # writes a short 360 loop and closes: 12 pictures at 640x360 (--loop-size 1080x1920 --loop-frames 24 for others)
sculpt.exe --record-loop out.mp4 --demo --loop-fps 60 # the same as a video (a name ending in .mp4): then reads the file back and checks its frames, size, frame rate and length
sculpt.exe --record-loop out.mp4 --demo --loop-cancel 15   # cancels after 15 frames (SCULPT_DEMO_CAMERA=close closes the app instead) and checks nothing is left
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
