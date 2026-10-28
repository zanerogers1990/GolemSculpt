# Sculpt

A native sculpting app written in Rust, modelled on the Unity sculpting app in
`D:\Unity\sculptingapplication`: same controls, same brush behaviour, same look.

## Run it

Double-click `Sculpt.bat`. The first run builds the app (a minute or two);
after that it starts instantly. The program itself is
`target\release\sculpt.exe`.

To open on a particular monitor, give its number as Windows numbers them in
Display settings: `sculpt.exe --monitor 2` (or `Sculpt.bat --monitor 2`, or
add it to a shortcut's Target). Without it the window opens where Windows
puts it, usually the main monitor. Test builds made while working on the app
open on whichever monitor the Claude window is on.

For pen pressure, tick **Enable Windows Ink** in the Huion driver. The
**Stroke** card (Brush tab) shows whether pressure is arriving: a live bar
while the pen is down, and a line saying so if none has been seen since the
app opened. Lifting the pen does not add a last, harder bit to the stroke: it
ends at the pressure the pen last had (a mouse has no pressure and always
sculpts at full strength and full size).

## Controls

| Input | Action |
| --- | --- |
| Left drag on the model | Sculpt |
| Ctrl + drag, or right drag | Sculpt inverted (Move: push the surface in or pull it out; Pose: twist the limb, or slide it) |
| Shift + drag | Smooth |
| Ctrl + Shift + click on the model | Isolate the face group under the pointer: the rest of the object is hidden (again, or on the background: show everything) |
| Left drag on the background | Orbit |
| Alt + drag | Orbit |
| Middle drag, or Alt + Shift + drag | Pan |
| Ctrl + Alt + drag | Zoom |
| Numpad 5 | Orthographic / perspective, as in Blender (also the Ortho / Persp button) |
| Numpad 1 / Numpad 3 / Numpad 7 | Front / right / top view, as in Blender |
| Ctrl + Numpad 1 / Ctrl + Numpad 3 / Ctrl + Numpad 7 | Back / left / bottom view |
| (Hand, magnifier and house beside the view gizmo's buttons) | Drag the hand to pan, the magnifier up or down to zoom; tap the house for Home, the view a scene starts with |
| Scroll over the model | Brush size |
| [ / ] | Brush size a step smaller / bigger (a tenth each step; hold the key to keep going) |
| Shift + [ / ] | Strength a step down / up, to the next 5% |
| Scroll off the model | Zoom |
| Double-click background, or F F | Frame the selection |
| Double-click another object | Select it |
| Ctrl + click a row of the object list | Add that object to the selection |
| H / Shift + H / Alt + H | Hide the selection / hide everything else / show everything (each an undo step, as the eye buttons are) |
| / or Numpad / | Local View: only the selection, framed; again to bring the rest and the view back |
| Delete | Delete the selected object (asks first) |
| 1 2 3 4 5 6 7 8 9 0 | The base brushes Move, Clay, Smooth, Crease, Inflate, Flatten, Pose, Standard, Layer, Snake Hook (Pinch, Nudge and Erase Layer have no key, nor has any other brush of the library until you give it one on the Keyboard Shortcuts card) |
| P | The Paint brush; again, the brush you had before it (see Polypaint) |
| I | Eyedropper: pick up the colour and material of the paint under the pointer |
| Ctrl + drag / Shift + drag with the Paint brush | Erase the paint / blur the paint (it never smooths the clay) |
| S / F / D + drag | Brush size / strength / focal shift; the number shows above the ring while the key is held |
| Space (hold) | Brush wheel: the brushes pinned to it, with the Strength of the brush in hand on a bar under it |
| M | Mask painting |
| X | Mirror X |
| R | Voxel remesh |
| Ctrl + D | Subdivide: add a finer level of the selected object and go to it |
| Page Up / Page Down | One subdivision level finer / coarser |
| G | Transform gizmo on / off |
| L | Measure tool on / off |
| Y | Array tool on / off |
| (Lathe button) | Lathe tool on / off: the Unity app gives it no key; Enter creates, Esc puts it away |
| T | Tube tool on / off: Enter fuses the tubes into the model, Esc puts it away |
| B | Muscle tool on / off: Enter fuses the muscles into the model, Esc puts it away |
| N | Mask tool: on (box), again for lasso, again for off |
| (Hide button) | Hide tool: a box or lasso hides part of the selected object; it has no key, as H hides whole objects |
| K | Split tool: on (line), again for lasso, box, then off |
| J | Keys tool on / off |
| Ctrl + Z | Undo |
| Ctrl + Y, Ctrl + Shift + Z | Redo |
| Ctrl + S, Ctrl + Shift + S, Ctrl + O | Save, save as, open |
| (Drag a file onto the window) | A sculpt (.sclp, .sculpt) opens as Open opens one; a model (.obj, .stl, .ply, .fbx, .glb, .gltf) is imported; a picture (.png, .jpg) becomes a reference picture |
| Ctrl + N | New scene (asks first if anything would be lost) |
| Ctrl + Q | Quit (asks first if anything would be lost) |
| Enter | Finish what the tool that is up is doing: fuse the tubes or muscles, accept an Extract, create a lathe's mesh, build the keys |
| Esc | Close the brush wheel, leave the clean view; during a stroke, cancel it (the surface goes back, no undo step); during a gizmo drag, put the objects back; otherwise put the tool that is up away |
| Tab | Clean view: nothing on screen but the model (Tab or Esc brings the panels back) |
| (Arrow tab on a panel's inner edge) | Fold that panel away to the edge of the screen, or bring it back; sculpting carries on |
| (Small window button in a card's header) | Float that card over the view; drag it by its title; the same button puts it back |
| O | Spin the view round the model (turntable) |
| F11 | Fullscreen |
| Ctrl + = / Ctrl + - | Interface size |
| (Tabs at the top of the Scene panel, and under the tools in the Sculpt panel) | Show that tab's cards: Brush or Mesh; Scene, Look, File or Settings |

Dragging on the background to orbit is the one addition to the Unity app's
controls; everything else matches it. These are the keys as the app ships
them: almost every key can be changed (see Keyboard shortcuts, below), and
the keys on the buttons follow.

### Keyboard shortcuts

The **Keyboard Shortcuts** card (Scene panel, Settings tab) lists every
command a key can be put on, grouped as the panels are, with the key each is
on now.

- **Click a command's key, then press the new one**, with Ctrl, Shift or
  Alt held if you like. Esc leaves it as it was; Backspace (or Delete) takes
  its key away. A key that another command had is taken from it, and the
  status line says which.
- **Any brush** can have a key, not only the ten base brushes on the number
  keys: type part of its name in the box at the top to find it (the card
  lists only the brushes that have a key until you do). The key shows in the
  brush's corner in the list and in its tooltip.
- Commands that ship with no key can have one too: Quad Remesh, Decimate,
  Reconstruct Subdiv, Project Detail, the mask's Invert, Clear, Grow, Shrink,
  Blur and Sharpen, Home view, Wireframe, Mirror Y and Z, Clone, Instance,
  Import. The hosted tools (Measure, Tube, Muscle...) are there under Tools.
- **S, F and D**, the keys held to drag the size, strength and focal shift,
  can move too; the strength key tapped twice frames the model, whichever
  key it is.
- **Keys like ZBrush**: the **ZBrush** button at the top puts ZBrush's keys
  on the commands the two share: W for the gizmo, D and Shift + D up and
  down the levels, F to frame, Shift + F for the wireframe, P for
  perspective, C to pick up a colour, U and O held for strength and focal
  shift, Ctrl + I to invert the mask, Ctrl + Shift + A to clear it. Where
  ZBrush uses a key this app had for something else, that command loses it
  (Remesh's R, the turntable's O and the Paint brush's P), and keeps its
  button. **GolemSculpt** puts every key back as the app ships it. Either
  asks to be pressed twice when keys were changed some other way. ZBrush's
  Alt to subtract and Ctrl to mask are things the pen does, not keys: here
  Ctrl inverts the brush and the Mask button paints the mask.
- A changed command gets a **↺** beside it, which puts it back on the key
  the app ships with; **Reset All Shortcuts** (press it twice) puts them all
  back. The header says how many are changed.
- Kept for the app, and refused with a word why: Space (the brush wheel),
  Esc, Enter, Tab, [ and ], Backspace, Ctrl + Q and Ctrl + = / Ctrl + -. What the pen and mouse do with a key held (Alt to
  orbit, Ctrl to invert the brush, Shift to smooth) is how sculpting works,
  not a command; the card lists those last, as they are.
- The keys are kept in the settings file with the rest, and only the ones
  you changed: a key the app adds later reaches you if you never moved it.
- While a hosted tool is up it has its own keys first; the view's keys and
  the Transform gizmo's still work.

A double-click is two clicks within half a second and 20 pixels of each
other, which leaves room for a pen tip, which never lands twice on the same
pixel. A press that drags (an orbit) is not the first half of one, so two
quick orbits one after the other do not frame the view; nor is an F held
and dragged to set the strength the first F of F F.

If the window loses focus (Alt-Tab, a click on another program) while a button
or key is held, the drag ends there as if you had let go (a stroke is kept as
its one undo step) and every held key is let go, so Ctrl + Z works at once on coming
back.

## Brushes

There are any number of brushes: the fourteen base ones, the ones that come
with the app, and your own (see Brush library, below). Each keeps its own
Strength, Focal Shift and falloff, its own spacing and pen settings (see
Stroke and pen, below) and the rest of what is in its settings card; Size is
shared by them all.

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
- **Strength** is how hard a brush works. On Standard, Inflate, Layer,
  Smooth, Flatten, Pinch and Nudge the slider gives the gentle end the most
  room: one pass of Standard, Inflate or Layer at full pressure barely shows
  at 10%, is a clear stroke at 50% (about a tenth of the brush's size high)
  and a strong one at 100% (about a third). Clay, Crease and the brushes made
  of them (Clay Strips, Dam Standard) work as they always have. Settings and
  brushes saved by an earlier version are brought over once: a Strength you
  had chosen becomes the one that does the same (as far as 100% reaches),
  and one left where the brush started takes the new starting value.
- **Accumulate** starts off on every brush that has it (Standard, Inflate,
  Clay and Crease). Off, a stroke builds up to a height and no further:
  going back and forth over the same place in one stroke smooths the stroke
  out rather than piling it up, and its cross-section stays the soft,
  rounded shape of the brush. Lift the pen and stroke again to build on top.
  Strength and the pen's pressure set how tall a stroke stands, and
  **Build-Up** how quickly it gets there. With Accumulate on, every pass adds
  more, with no limit. Standard and Inflate weigh the surface where the
  stroke found it, so a stroke laid over itself is a rounded bead, not a
  peaked ridge. (Settings from an earlier version: Accumulate on Standard,
  Inflate and Clay where they started, which was on, becomes off once.)
- **Relax Normals** (Inflate, starts at 30%) evens out the directions
  Inflate pushes the surface in, so neighbouring bits swell together and a
  strong stroke does not split into spikes. With Accumulate on, Inflate also
  works out which way the surface faces afresh before every dab.
- **Steady Plane** (Clay and Flatten, and Trim, Fill and Polish made of
  Flatten; starts at 50%) keeps the level the brush works to steady along
  the stroke, so it does not tip and bob over every bump it passes: a more
  even strip. 0% lets each dab find its own level; 100% holds the one the
  stroke started with. Clay Strips and Clay Tubes keep it at 0%, as they
  were. Brushes saved by an earlier version keep 0% too. Trim keeps it at
  30% and a touch less Strength, so it cuts as deep as it always did.
- **Auto Smooth** smooths the surface a little behind every dab, the way
  that keeps its volume, so a stroke leaves it clean: no lumps or spikes at
  the edge of a swelling, and nothing gets thinner. It leans on the pen as
  the brush does. It starts at 15% on Inflate and 5% on Standard, and at 0%
  on every other brush (Clay Strips and Dam Standard are as they were; on
  Snake Hook it would smooth the pull back as you pull, and on Layer it
  would ripple the step's edge).
- **Even Out** slides the mesh within the surface so the triangles a brush
  stretched come back even, without the shape changing much: cleaner for
  the next stroke. It starts at 0% on every brush, so nothing moves that you
  did not move. Turned up, Inflate evens out behind every dab; Move evens
  out once, as you let go of the drag (part of the same undo step, and what
  is right under the pen stays there, though a far drag's shape shifts a
  little); Snake Hook as you let go (the clay under the pen slips back down
  the pull a little for it).
- **Clay Depth** on the plain Clay starts at 0.50 (it was 0.30): with
  Accumulate off, one pass at 50% Strength builds about a third of the
  brush's size. Settings from an earlier version that had 0.30, where it
  started, take the new start once; a depth you had set stays.
- **Strength** on Move is how far the surface follows the pen: at 100% (where
  it starts) the grabbed point stays under it; lower is a gentler pull that
  goes only part of the way.
- **Front Facing Only** starts on for Standard, Clay, Inflate, Layer,
  Flatten, Smooth, Pinch and Nudge: a stroke on a thin form (an ear, a fin)
  leaves its far side alone. It starts off for Move, Snake Hook and Crease,
  and Clay Strips and Dam Standard are as they were.
- **Ctrl + drag** with Move (or a right drag) pushes the surface in or pulls
  it out along the way it faces, instead of sliding it across the screen:
  the one direction a plain Move cannot go without turning the camera. Where
  the surface faces you, drag up to pull it out and down to push it in. At
  the outline, where you see it from the side, drag the way it faces. The
  whole grab goes one way, by the brush's falloff and Strength.
- **Connected Only** (Move) grabs only the piece of surface under the pointer,
  so moving one finger leaves the finger next to it alone.
- **Snake Hook** follows the pen's pressure: press lightly and the pulled
  clay lags and thins, press firmly and it stays with the pen. **Thickness
  (Thin - Full)** says how a pull keeps its thickness: at 50, where it
  starts, a long pull stays about as thick out to its tip as where it left
  the surface; at 0 it thins to a point as before; below 0 it draws in to a
  spike, for horns and claws. The pulled tip also turns with the stroke, so
  a pull that curves keeps its shape round the bend.
- Mask painting has **Mask Hardness** for its edge; the selected brush's
  falloff and Front Facing Only do not apply to it.
- **Pinch** draws the surface in toward the line of the stroke, from both
  sides, without digging in: a soft ridge or fold gets sharper, an edge gets
  tighter. Ctrl + drag (or the right button) spreads the surface away from
  the line instead. A press that never sets off pinches toward its own
  point. It has no number key.
- **Nudge** pushes the surface sideways along the stroke, like sliding clay
  with a thumb: whatever is under the ring is carried along and left behind
  as the brush moves on, staying on the surface. Its Strength is how far the
  surface follows the pen (at 100% what is under the middle of the ring
  stays under it; it starts at 63%). Unlike Move it works on what is under
  the ring now, all along the stroke. It leaves stretched triangles behind,
  which Smooth's Relax evens out. It has no number key.
- **Erase Layer** rubs a sculpt layer out under the ring: the layer's
  wrinkles or pores go where you draw and stay everywhere else, and the
  model underneath and the other layers are not touched. It works on the
  layer that is recording (see Sculpt layers, below, which has the rest).
  It has no number key and starts off the Space wheel.
- **Paint** paints colour and materials on the model instead of sculpting
  it: see Polypaint, below. P takes it up (and puts it down again), and it
  starts off the Space wheel.

**Layer** raises an even step of the Layer Height and eases up to it: a
light pass raises a low step, a firm pass or another one in the same stroke
raises it to the full height and never past it. A new stroke builds a new
step on top.

**Smooth** (and Shift + drag with any brush, which is Smooth) has five modes
and one slider. Each dab smooths up to four times over (all four at 100%
Strength); it starts at 22%, which smooths as much as it always has. Every
mode smooths an open edge (the rim of a hole, the edge of a sheet) along
itself, so the opening keeps its place, and leaves a lone spike's tip alone.

- **All** is plain smoothing: bumps go down, pits come up.
- **Surface** takes fine roughness off and keeps every form its size: a
  finger, a horn or a thin edge does not get thinner however often you go
  over it. It leaves lumps as big as the brush mostly alone, which is why
  Smooth starts on All; the **Surface Smooth** brush is Smooth set to it.
- **Peaks** only lowers what stands up: it knocks bumps off and leaves the
  creases beside them as deep as they were.
- **Valleys** only fills what lies low: pits and creases come up and the
  peaks are left standing.
- **Relax** slides the mesh within the surface until its triangles are even,
  without changing the shape. Use it on a patch that Move, Snake Hook or
  Nudge has stretched.
- **Keep Volume** (All only) gives back what smoothing takes away. Plain
  smoothing makes a form a little thinner every stroke: a finger, the edge
  of a fin, a small ball. At 100% the form keeps its thickness and its bumps
  still go; at 0%, where it starts, it shrinks as it always has.

**Flatten** has three modes and a lock:

- **Both** levels to the plane under the ring: high parts come down and low
  parts come up. **Scrape** only brings down what stands above the plane,
  like scraping with a flat tool. **Fill** only raises what lies below it.
  Ctrl + drag with Scrape fills, and with Fill scrapes; with Both it pushes
  the high and low further apart, as before.
- **Lock Plane** fixes the plane where the stroke begins, and the whole
  stroke levels to that one plane: scrub over a patch to cut a true flat
  facet. It works on what lies within half the brush's size of the plane
  and leaves the rest alone, so a stroke that wanders off round the form
  does not drag the surface after it.
- **Plane Offset** moves the plane lower or higher, as before.

### Brush library

A brush here is what it is in ZBrush or Nomad: a name over a set of settings.
Fourteen of them are the **base brushes** (Move, Clay, Smooth, Crease,
Inflate, Flatten, Pose, Standard, Layer, Snake Hook, Pinch, Nudge, Erase
Layer), one for each kind of stroke the app can make. Every other brush is one of those kinds with its
own settings: Dam Standard is a Crease that tapers with the pen, Trim is a
Flatten that only scrapes, Rake is a Standard with the Rake alpha.

**Picking a brush.** Every brush is a row of the **Brushes** card on the
Brush tab, with its kind's icon, its name and, for the base brushes, its
number key in gold. The rows are sorted into kinds under small headings
(**Clay & Build**, **Smooth & Flatten**, **Cut & Pinch**, **Move & Pose**,
**Paint & Layers**, then **Yours**); press a heading to fold its kind away,
and the app remembers it. Click a row and that brush is in hand; its name is
in the card's header and on its settings card. Add, Sub, Mask, Size,
Strength and Focal Shift sit on a plate above the card, always in view,
whatever is folded.

**Every brush keeps its own settings**, and a change sticks as you make it:
there is no save button. What a brush keeps is everything in its settings
card (Strength, Add or Sub, Accumulate, Build-Up, Focal Shift, Front Facing,
the falloff curve, the alpha and how it is laid, Spacing, Scatter, Size
Jitter, the two pen pressure settings, Smooth's and Flatten's modes, and the
numbers of its kind: Clay Depth, Crease's Pinch and Depth, Layer Height and so
on). Picking one brush never changes another, even another of the same kind:
set Clay Strips up one way and Clay another and each is as you left it when
you come back. Lazy Mouse is a brush's own too: on for Dam Standard, say, and
off for Move. What is NOT a brush's own: the size of the ring (one size for
every brush, so changing brush never resizes it), Build Up on Hold, the pen's
Pressure Curve, symmetry and dynamic topology.

At the top of the card, **Find a brush** lists only the brushes with what
you type in their names (any part, any case; Esc empties it), and the
**…** button beside it opens the menu below for the brush in hand, for a pen
whose barrel button is easy to miss. Under them, **Recent** holds the four
brushes you took up last before the one in hand, however you took them up
(the list, the Space wheel, a number key); it is remembered between runs.

**Right-click a brush** (or press the pen's barrel button on it, or the
**…** button for the brush in hand) for everything else that can be done
with it:

- **Duplicate** makes a brush of your own that starts as this one is now, and
  puts it in hand. This is how to make a brush: duplicate the nearest one,
  name it, change it.
- **Name** (your own brushes only, under the list while one is in hand):
  type a new name and press Enter. The brushes that came with the app keep
  their names.
- **Reset** puts the brush back as it came with the app or, for one of your
  own, as it was when you made it.
- **Delete** takes it out of the library. The fourteen base brushes stay. A
  brush of your own that is deleted is not destroyed: its file is moved to a
  `deleted` folder inside the brushes folder, and Import Brush brings it
  back. A deleted brush that came with the app comes back with **Restore
  Shipped**, under the list, which appears once one is gone.
- **Put On / Take Off the Space Wheel** puts the brush on the wheel that
  opens while Space is held, or takes it off, and **Reset the Space Wheel**
  goes back to the brushes it started with. A dot on a row marks a brush
  that is on the wheel.
- **Shift Smooths With This** (smoothing brushes only): holding Shift with
  any brush smooths with the one brush that has this on, marked with an S in
  the list. It starts on Smooth. Turn it on for Relax, say, and Shift relaxes.
- **Import Brush...** (under the list) brings in a brush from a file, and
  **Export Brush...** writes this brush to one, to keep or to give to someone. A brush
  that uses an alpha the other computer does not have comes in with no
  alpha, and the status line says which one it wanted.

**The brushes that come with the app**, and what each is for:

| Brush | Kind | What it does |
| --- | --- | --- |
| Clay Strips | Clay | Flat-topped, square-ended strips that lay over each other: blocking in |
| Clay Tubes | Clay | A rounded bead, like a rolled coil pressed on |
| Dam Standard | Crease | A clean sharp cut whose width follows the pen, thinning to nothing as it lifts |
| Trim | Flatten | Shaves off what stands above the surface, hard-edged, and leaves hollows alone |
| Fill | Flatten | Fills hollows up to the surface around them and leaves what stands proud |
| Planar | Flatten | Levels the whole stroke to the plane it started on: one flat facet |
| Polish | Flatten | A gentle flatten for evening a surface out |
| Smooth Peaks | Smooth | Smooths only what sticks out |
| Smooth Valleys | Smooth | Smooths only the hollows |
| Relax | Smooth | Evens out the triangles without changing the shape |
| Keep Volume | Smooth | Smooths without thinning |
| Surface Smooth | Smooth | Polishes fine roughness away and keeps every form its size |
| Rake | Standard | Light parallel scratches that follow the stroke |
| Surface Noise | Standard | Roughens the surface with scattered, turned prints of a noise picture |
| Scales | Layer | Press and drag out one print of scales |
| Spread | Pinch | Pinch turned round: opens a crease up |
| Airbrush | Paint | Soft, light mists of paint that build up as you go over a place: shading and blushes |

Each was set up by drawing with it and looking, on a finely remeshed ball.
They are starting points: change one and it stays changed, and Reset gives
you the original back.

**Where brushes are kept.** The fourteen base brushes are in the settings file,
as they always were, so settings from an older version come straight in as
the base brushes' settings. Your own brushes are one small file each in
`%APPDATA%\Sculpt\brushes` (plain JSON; the file Export Brush writes is the
same kind), and so is a shipped brush you have changed. A file in that folder
that is not a brush, or one damaged by a hand edit, is skipped with a line on
the status bar and left exactly as it is.

### Stroke and pen

A stroke is a run of dabs laid along the pen's path. How a brush lays them is
that brush's own, under **Dabs** and **Pen Pressure** in its settings card
(Clay Settings, Crease Settings and so on); how the hand is steadied is in
the **Stroke** card, just under Size and Strength. Move, Pose and Snake Hook lay no dabs and have
none of the per-brush ones, but for Snake Hook's Pressure -> Strength.

- **Spacing** is how far apart the dabs are, as a share of the brush's size:
  2% to 250%. It starts at 20% (10% for Crease). Lower is a smoother stroke
  and more work for the machine; higher shows the dabs. It changes how smooth
  the stroke is, not how much it builds: at any spacing up to 100% a stroke
  builds at the same rate. From about 100% the dabs stand apart as separate
  prints, each as tall as the stroke would have been, which is how to lay an
  alpha down as a row of prints. Clay stops sweeping its tip from dab to dab
  above 50%, so its dabs show too.
- **Scatter** throws each dab off the line of the stroke, any way along the
  surface, by a random distance up to that share of the brush's size.
- **Size Jitter** makes each dab smaller than the brush by a random amount,
  up to that much.
- **Pressure → Size** makes the brush smaller the more lightly you press, so
  a stroke can swell and taper: at 100% the lightest touch is a fine point (a
  tenth of the ring) and full pressure is the ring's size. It starts at 0,
  which is off. The ring shrinks and grows with the pen while you draw. The
  dabs stay as close together for their size all the way along.
- **Pressure → Strength** makes the brush weaker the more lightly you press:
  at 100% a feather touch does next to nothing, at 0% pressure does not
  change the strength. It starts at 100% (Clay Strips keeps the 88% it has
  always had, and Dam Standard its 60%). A setting of the one Light-Touch
  Floor there used to be for all brushes is carried over into every brush.
  Mask painting uses the selected brush's Strength and this with it, and
  none of that brush's other stroke settings.
- The pen's pressure is smoothed a little, to take the grain out of the
  sensor, and follows the pen within a few hundredths of a second. Every
  stroke starts from the pen's own first pressure, so a light touch after a
  firm stroke starts light, with no blob, and a firm one starts firm. With
  Lazy Mouse on, the pressure trails the pen as the brush trails the pointer,
  and still comes down as the pen lifts, so a stroke tapers at its end.
- Random does not mean different every time: the same stroke on the same
  surface always comes out the same, and mirrored and radial copies of a
  scattered stroke are still exact copies of each other.

The **Stroke** card sits right under Size, Strength and Focal Shift, and its
header shows what is set while it is closed ("Lazy 25 px · Curve 1.60"). In
it: **Lazy Mouse** for the brush in hand (the brush trails the pointer on a
short rope; **Lazy Radius** is how long the rope is, **Lazy Smoothing** how
readily the brush follows once it is taut). Each brush keeps its own Lazy
Mouse, as in ZBrush; it is off on every brush to start with, and a settings
file from before, which had one Lazy Mouse for all brushes, gives that one to
every brush. Move and Pose never trail. Then, for every brush: **Build Up on
Hold**, the pen's **Pressure Curve** (how the pen answers your hand: higher
needs a firmer press before pressure counts) and the pen readout.

**Checking the pen** (once, with the tablet; it takes a minute):

1. Open the **Stroke** card and press the pen on the tablet without drawing.
   The line that says no pressure has arrived should turn into a bar that
   moves as you press harder and softer. If it stays a line, the pen is
   working as a mouse: tick **Enable Windows Ink** in the Huion driver.
2. Lift the pen: the bar goes, and the readout says the pen is up. Hover it
   over the model without touching: the ring follows and nothing is drawn.
3. Pick **Dam Standard** (its **Pressure → Size** is on). Draw one stroke
   that starts light, presses hard in the middle and ends light: it should be
   a hair at both ends and a wide cut in the middle, and the ring should
   shrink and swell as you go.
4. Draw two strokes side by side with **Standard**, one as lightly as you can
   and one pressing hard: the light one should be much the fainter, with no
   blob where it began. (Until a pen has been seen on this computer, a touch
   that reports no pressure is taken for a mouse, at full strength. Once one
   has, the settings remember it, so even the first touch after the app opens
   is the pen's.)
5. With the pen down and drawing, rest your other hand on the display: the
   stroke should carry on under the pen and not jump to the hand or stop.

**Esc in the middle of a stroke** cancels it: the surface goes back exactly as
it was when the stroke began (sculpt layers, paint and what dynamic topology
added included), nothing goes on the undo history, and the rest of that drag
does nothing until the pen is lifted.

A fast stroke with a big brush on a dense model can be more than the machine
can lay as fast as the pen moves. The stroke then falls behind the pen and
catches up, and lifting the pen finishes it: nothing along the path is left
out, where before such a stroke came out as a row of separate dabs. Only if
it gets more than about a second behind are its dabs laid further apart (each
doing as much more), so it gets coarser rather than later.

On this machine that takes some doing. With a ring of up to 250 pixels on two
million triangles, and with rings of ordinary size on eight million, a
stroke drawn across the model in half a second kept up at sixty frames a
second. It fell behind only with a ring bigger than the whole of an
eight-million-triangle ball, every dab reworking the model's whole front:
there the frames took 70 ms, the brush trailed the pen by up to four fifths
of the ring, and after a second of scribbling it caught up about a second
after the pen stopped (a lift at that moment held the app for a second).

### Alphas

An alpha is a greyscale picture the brush prints with instead of its plain
round tip: white prints the most and black not at all. Standard, Clay,
Inflate and Layer take one, each its own, under **Alpha** at the bottom of
the brush's settings. This app's own; the Unity app had five for Clay only.

- Seven come with the app: **Square**, **Rake**, **Noise**, **Bumps**,
  **Scales**, **Cracks** and **Weave**. **Import Alpha...** adds a picture of
  your own (.png or .jpg), copied into `assets/alphas/Imported`; pictures put
  in `assets/alphas` by hand show up after **Rescan Folder**. A picture that
  is not square is centred with black round it, and a big one is kept at 512
  samples a side.
- **Alpha Size** is the picture's size against the brush ring (at 100% it
  fills the square the ring sits in), and **Alpha Rotation** turns it.
- **Random Rotation** turns each print by a random angle, up to that many
  degrees either way (at 180° a print may face any way at all), so a row of
  prints does not look stamped by a machine. Set the brush's **Spacing** to
  100% or more to lay the prints apart. A Drag to Place print gets one random
  turn, and keeps it while you drag.
- **Turn With Stroke** points the top of the picture the way the stroke is
  going, so a rake's lines follow the hand; off, the top points up the
  screen. The first dab waits for the stroke to set off, so it faces the
  right way too.
- **The picture shows under the cursor.** While a brush with an alpha is in
  hand and the pointer is over the model, the picture is drawn faintly inside
  the ring, the size it will be and turned as it will be (and, once a stroke
  with Turn With Stroke is moving, the way the stroke is going), so a print
  can be aimed before it is made. It lies flat on the screen: on a surface
  seen at a slant the print itself is foreshortened and the preview is not.
  It goes wherever the ring goes, so the clean view has neither.
- **Repeat** tiles the picture past its edges: with a small Alpha Size it
  covers the brush in a fine pattern. All but Square repeat without a seam.
- **Invert** swaps black and white: bumps become pits.
- **Drag to Place** makes one stroke one print (ZBrush's DragRect): press
  where its middle goes, drag outward to size it, and lift to leave it. Until
  the lift the surface is put back and the print made again each frame, so it
  can be dragged smaller as well as bigger. Strength sets how tall it stands
  for its size (at 100%, half its radius). A tap that does not drag prints
  nothing. Without Drag to Place a stroke lays the picture down dab after dab
  and smears it along itself, which suits a rake or a rough texture and
  nothing with a shape of its own.

The brush's own falloff still fades the picture toward the ring, as in
ZBrush: slide Focal Shift toward Hard to print more of it. Mirror and radial
copies print mirrored and turned pictures. Clay's Surface Relax is left out
of a stroke with an alpha, since it would smooth the print away. A print
cannot be finer than the mesh under it: remesh finer, or turn Dynamic
Topology's Detail down, before printing small. Drag to Place does not add
triangles itself.

A Clay stroke with an alpha lays its strip from where the pen lands, rising
over one print's length at the start as it falls away over one at the end.
(It used to stamp one whole print where it landed as well, which left a line
across every strip, one print in.) A press that does not go anywhere still
prints once: when it lifts, or after a quarter of a second held still.

Not here yet: alphas for mask painting.

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
- With radial symmetry on (see Symmetry below), limbs round the axis, like the
  arms of a starfish, each get a joint of their own and all bend alike. A
  limb that stands on the axis itself (the stem of a vase, the spike on a
  helmet) will not bend, since no bend is the same from every side. It can
  still be twisted about the axis (Ctrl + drag, unless a mirror runs through
  it as well), resized, slid along the axis and stretched. A limb on the line
  where two mirror planes cross is held the same way.

## Symmetry

The **Symmetry** card on the Sculpt panel's Mesh tab makes every stroke in several
places at once: on both sides of a plane, all the way round an axis, or both.

**Every object has its own symmetry**, as in the Unity app. The card shows the
selected object's (it names it under its header) and changes that one alone.
Select another object and the card, the guides and every stroke go by that
object's settings: a figure whose face is mirrored across X and whose body
across Z keeps both, with nothing to re-tick on the way from one to the other.

- **Symmetry space** is where the planes and the axis sit. **Local** (to start
  with) is through the object's own centre along its own axes: the symmetry
  moves and turns with the object. **World** is through the world's centre
  along the world's axes: it stays put while the object is moved or turned
  through it. World pairs with **Mirror** in the Objects card: parts placed
  either side of the world's centre sculpt symmetrically.
- **Mirror strokes across** X, Y and Z, in any combination. **X** on the
  keyboard is Mirror X of the selected object.
- **Radial** repeats every stroke evenly round an axis through the object's
  centre (the world's, in World). **Radial Repeats** is how many times the
  stroke is made, itself included, from 2 to 32. **Radial Axis** is the
  object's own X, Y or Z (the world's, in World; Y to start with: the way up
  of a vase or a bust, and the axis a lathed object is turned round), or
  **Custom**: a direction typed in as `x, y, z`, or taken with **From View**,
  which points the axis straight down the view, so that seen from where you
  are the repeats land evenly round the centre.
- Mirrors and Radial combine. Mirror X with six repeats about Y makes each
  repeat and its mirror image, twelve strokes in all; a mirror that the
  repeats already make (Z on top of X with an even count about Y) is not
  made twice.
- **A brush is as strong beside a mirror plane, on it, and at the radial
  axis, as it is anywhere else.** Where copies of a stroke land on top of
  each other they are shared out rather than stacked. A stroke drawn down
  the centre line of a mirrored model builds up as much as the same stroke
  drawn beside it (it used to build up twice as fast, with every brush), and
  one drawn a little to one side runs together with its image into one
  smooth ridge of the same height, with no line down the middle. Where two
  planes cross it is the same (a dab half a brush radius from both used to
  be half as strong as it should be). Measured with every brush, a dab or a
  stroke by a plane now moves the surface between 0.9 and 1.1 times as far
  as the same one with no symmetry. A Move at the axis slides along it
  instead of being pulled every way at once, and a Move on a mirror plane
  holds the surface with the same soft edge as anywhere else (it used to
  hold everything inside half the ring as firmly as its centre).
- Strokes that cross still build up where they cross, as they do with no
  symmetry at all: a stroke drawn across a mirror plane meets its image
  coming the other way, and the arms of a radial stroke all meet at the
  axis, so the middle comes up about half as tall again as the arms.
- Layer does not go past its Layer Height on a plane, where planes cross or
  at the axis: the step is no taller than anywhere else. The same goes for
  Standard, Crease and Inflate with Accumulate off, and for Flatten and
  Smooth, which each work toward a place and stop there: where copies meet,
  the surface goes as far as the one that asks most, and no further.
- Every brush follows the selected object's symmetry, mask painting included,
  and so do the Mask, Hide, Trim and Split tools. The Tube tool follows the
  symmetry of the object a tube is drawn on (it draws a tube for each copy).
  A stroke that lands on an instance of the selected object, on one of its
  live mirror copies or on a copy of its array is made under the same
  symmetry: a copy is the object's own mesh drawn again.
- **Show Symmetry Guides** draws a thin line on the selected object where
  each mirror plane cuts it and along the edge of each radial sector, the
  spokes meeting where the axis comes through the surface. They are in the
  axis's colour (red, green, blue; amber for a custom axis), and Tab hides
  them. This one tick is the whole viewport's, not an object's: in the Unity
  app a tick that hid one object's planes and left another's standing read
  as a tick that did not work. (The Unity app drew every object's planes at
  once; here only the selected object's guides are drawn.)

**Symmetry Repair**, folded under its own heading at the foot of the
Symmetry card (press the heading to open it), is for a model whose
two sides no longer match: one sculpted with the mirror off, joined from odd
parts, or imported lopsided. It is the Unity app's panel of that name. Pick
the **Symmetry Plane** (**X**, **Y** or **Z**) and the buttons under it work
across that plane of the selected object:

- **Check Symmetry** changes nothing and says on the status line how the
  two sides compare: how many vertices pair up across the plane, how many
  sit on it, and how many have no partner. Pairing up is not the same as
  matching: a model with a lump on one side pairs up perfectly, so the line
  goes on to say how many pairs are not mirror images of each other and how
  far out the worst is, or that every pair is. It ends with the button that
  helps. It is worked out when pressed, not kept up to date: every stroke
  would make it stale.
- **Make Symmetric** (**+X to -X**, **-X to +X**) moves the vertices of the
  side named second onto the mirror image of the side named first, each onto
  its own partner. No triangle changes, so the mask, the face groups and the
  subdivision levels are all as they were, and it is recorded as an ordinary
  stroke: one undo step, taken by a sculpt layer that is recording as a
  brush stroke would be. With a mask painted only the
  unmasked part moves. Hidden vertices are left where they are, as by a
  brush, and the line says how many. Where more than a twentieth of the
  model has no partner it changes nothing and says why ("Too asymmetric to
  match up ... use Cut & Mirror +X to -X instead"): moving the half that
  pairs and leaving the half that does not would tear the surface.
- **Cut & Mirror** (below) is the one that always works.
- **Symmetry Cleanup** puts the vertices that sit close to the plane exactly
  on it, and merges the doubled ones that leaves: two halves joined along
  the plane and never welded become one closed surface. Where nothing needed
  merging it is a stroke like Make Symmetric; where vertices were merged it
  is a new mesh, as after Cut & Mirror. One undo step either way, and none
  when the model was clean already.
- **Match Tolerance** is how far apart two vertices may be and still count
  as a pair, from a quarter of what the mesh itself suggests to eight times
  it (the slider reads `1x` where it starts). Widen it when halves that were
  built separately will not pair. For Cut & Mirror it is how near the plane a
  vertex must be to be kept as part of the seam. It belongs to the card, not
  to an object, and starts at `1x` on every run.

**Cut & Mirror** keeps one side and throws the other away. Press **+X to -X**
or **-X to +X**: the side named first is kept, and the model is rebuilt as
the kept side and its exact mirror image, the two joined along one row of
points on the plane.

- The plane is the object's own, through its own centre along its own axes:
  the one a Local mirror sculpts across. That is so whatever Symmetry space
  is chosen, as in the Unity app, and with World chosen a line under the
  buttons says so.
- The kept side is left as it was, triangle for triangle, up to the plane;
  only the triangles the plane runs through are cut.
- The mask comes across, mirrored with the surface. Hidden parts are cut
  with the rest and everything shows again afterwards, and a quad-remeshed
  object comes out as triangles, as after anything that rebuilds a mesh.
  Instances share the mesh, so they all change together.
- It is one undo step: Ctrl + Z brings the whole model back, with its mask, its
  hidden parts and its quads.
- The status line says what was kept and what was replaced ("Cut & mirrored
  +X onto -X: kept 3461 triangles, replaced 2347, now 3478 vertices."),
  because the replaced side is gone for good once the step leaves undo. It
  goes on to say if the result is not a closed surface: a model that had
  holes still has them. With nothing of the model on the side to keep,
  nothing is changed, and the line says "Nothing on the +X side to mirror".
- It does not tick a mirror axis: do that to go on sculpting both sides.

What an object starts with, as in the Unity app:

- A new shape, an imported model, an SSphere skin, a mask extract and a
  lathed object start with no symmetry (a lathed object with **Radial
  Symmetry on Create** ticked starts with that).
- A copy starts with what it was copied from: **Clone**, a live mirror made
  into objects with **Validate** (a custom radial axis is mirrored with it),
  the part **Split** cuts off, a tube kept separate (it takes the symmetry of
  the object it was drawn on), and the objects an array is turned into.
- **Instances share one symmetry.** An instance is the same mesh in another
  place, so the objects that share a mesh are sculpted the same way: change
  the symmetry with any one of them selected and it changes for all of them.
  **Make Unique** gives one its own copy of the settings along with its own
  mesh.
- The object a **Join** or a boolean keeps, and an object that is remeshed,
  trimmed or merged into, keeps the symmetry it had.

Changing a symmetry is not an undo step, and no undo puts an older setting
back, as in the Unity app: Z after ticking an axis steps back the last change
to the model, and an object that Ctrl + Z brings back has the symmetry it was left
with. Each object's symmetry is saved with it in the `.sclp` file, so a scene
whose symmetry has been changed has something to save: closing asks first
(see Files), until it is saved or the setting is put back as it was.

**World, exactly.** A World symmetry is worked out afresh from where the
selected object stands each time a stroke begins, so moving or turning the
object between strokes moves the planes through the model and never in the
world. A mirror image across a plane the object is square to is exact to the
last bit, as a Local one is; across a plane it is turned against, the two
sides agree to rounding (a few ten-millionths of the model's size in the
tests). On an object scaled unevenly and turned as well, the plane still
passes through the right place but is as tilted as a rigid mirror image of a
stretched object must be (the Unity app's limit too).

**Rebuilds never mirror.** A **Remesh**, a **Join** with remesh and the
booleans rebuild the whole model as it is, whatever the Symmetry card says.
The brush mirrors are for sculpting; they never decide that one side of a
model is thrown away and replaced by a copy of the other. (They used to,
and a door sculpted on one side came out as two.) To make a model
symmetric, use **Cut & Mirror** or **Make Symmetric**, which say so.

## Dynamic topology

**Dynamic Topology**, in the Remesh card on the Sculpt panel's Mesh tab, adds triangles
under the brush as you sculpt, so detail appears where you work and nowhere
else, and merges them where a bigger brush asks for less. It starts off, and
the app remembers how you left it.

- **Detail (Fine - Coarse)** is the longest edge left under the brush, as a
  percentage of the brush's radius: 3% to 50%, starting at 15%. Detail goes
  with the brush, so a small brush adds fine triangles and a big one makes
  the surface as coarse as itself. Zoom in and shrink the brush to ask for
  more.
- **Both, Add, Remove** say what it does to the triangles under the brush.
  **Both** (the default) splits the ones too big for the detail and merges
  the edges shorter than 40% of it, so the surface comes out about the
  detail size whatever was there: run a big brush over a finely worked area
  and it is coarse again. This is what Blender (Subdivide Collapse) and
  Nomad (Uniformisation) do by default. **Add** only splits, as dynamic
  topology did here until October 2026: an area stays as fine as the finest
  brush that has worked on it. **Remove** only merges: a brush that takes
  detail away and leaves the shape.
- Standard, Clay, Crease, Inflate, Flatten and Layer refine. Move, Pose,
  Snake Hook, Smooth and mask painting do not. Pull a shape out with Snake Hook,
  then run one of the refining brushes along it, and the triangles the pull
  stretched are split into even ones.
- **How a merge works.** One end of a short edge is folded onto the other,
  which stays where it is, and the two triangles on the edge go. It is not
  made where it would harm the surface: where it would turn or flatten a
  triangle, pinch two parts of the surface together, or leave an edge longer
  than the detail (which the next split would undo). Points frozen by the
  mask stay, a point much more masked than the other end stays, and so do
  the borders of face groups, hidden faces and open edges. A mirrored stroke
  merges on both sides; the two sides' triangles need not match exactly, the
  surface does: in the tests, to a millionth of the model's size on average
  and under a thousandth at the worst point.
- **Remesh** evens the whole model out at once. Dynamic topology stops adding
  at 4 million triangles, and a line under the slider says when it has;
  merging goes on there, so Both or Remove brings the count back down.
- What a merge took away stays behind in the mesh's lists, as triangles with
  no area and points in no triangle, and the next split near there takes
  the places back, so the lists stop growing: six rounds of a fine pass, a
  coarse one and a fine one again over the same place left a ball's lists
  at the size its surface was at most (8,954 triangles), where before they
  grew every round (to 28,430, for a surface of about 8,900). An object with
  sculpt layers, paint or hidden faces is the exception: its places are not
  taken back (each vertex carries more there, in records of their own), and
  its lists grow as before. Whatever is left is cleared when the mesh is
  next built afresh: saved and opened again, remeshed, decimated. It is
  never drawn, never hit and never counted (the counts shown are of the
  surface). Export, Send to Blender, Decimate, Subdivide and Check Symmetry
  work on a copy without it; Make Symmetric, Symmetry Cleanup and Groups by
  Angle first build the mesh again without it, as an undo step of its own.
- A stroke that added or merged is one undo step like any other: undo puts
  the triangles back as they were and redo does the stroke again.
- Masked surface is left alone, and a new vertex takes its mask from the two
  it was made between. Hidden parts are not refined. Instances and live
  mirror copies show the refined mesh, and a stroke drawn on one of them
  refines under the pointer. A mirrored stroke refines both sides, and a
  stroke under radial symmetry refines under every repeat.
- A quad-remeshed object stops being quads the first time a brush refines
  it (undoing that stroke brings them back): from then on its wireframe
  shows triangles, and OBJ export writes triangles.
- **Reset Shape** still goes back to the shape before sculpting, with the
  added triangles lying on it.
- It is off for an object that has subdivision levels (below), and the card
  says so: the levels need the object's triangles to stay as they are.
  Subdivide is how such an object gets more detail.

## Subdivision levels

The **Subdivision** card, on the Sculpt panel's Mesh tab, keeps one model at
several levels of fineness. Shape the big forms on a low level, where there
are few triangles and a stroke moves a lot of clay; add the detail on a high
one; and go back down whenever a big form needs changing. What you sculpt on
one level is kept on the others.

- **Subdivide** (Ctrl + D) adds a level above the finest one, with four times
  the triangles, and moves the object to it. The line under the button says
  what it will make and about how much more memory that takes ("77k to 307k
  triangles: about 34 MB more memory": about 35 MB a million triangles for
  the levels and 80 for the model, the coarser model being kept for Undo).
  The model is rounded a little
  as it is divided, as clay is when it is smoothed. Tick **Flat** to cut each
  face into four and move nothing, for hard-edged shapes that must keep flat
  faces and sharp corners (the app remembers the tick). The most a level can
  have is 25 million triangles; past that the button is greyed and the line
  says why.
- The card lists the levels with the triangles each has, the one you are on
  lit. Press a row to go to that level, or use **Lower** (Page Down) and
  **Higher** (Page Up) to go one at a time. Subdivide works from the finest
  level only, and says so from any other.
- **Going down**, the coarse level is the sculpt itself with fewer points, not
  a cage round it: its points sit on the surface you sculpted. Going straight
  back up changes nothing at all.
- **Going up after changing a low level**, the finer detail goes with the
  surface under it: bend an arm on level 1 and the wrinkles sculpted on level
  4 come round the bend with it, the same depth, still standing straight out
  of the skin. Only what the change reaches is touched.
- One thing to know. A big, broad change made on a low level comes back up
  very nearly as you made it. **One point pulled out by itself** on a low
  level comes back up as a softer, lower swelling, about half as high, and
  stepping down again shows the point on that swelling. The finer levels
  round a lone spike off; pull a few neighbouring points together if you
  want the full height.
- **Delete Lower** throws away the levels below the one you are on, which
  becomes the coarsest. **Delete Higher** throws away the levels above it,
  and the detail sculpted on them. Neither changes the model on screen.
- A model from **Quad Remesh** subdivides into quads and stays quads on every
  level, so Quad Remesh then Subdivide is the clean way to a model whose
  faces follow its form. A quad model imported from ZBrush or Blender does
  the same. Any other model subdivides as triangles.
- **Reconstruct Subdiv** (under Subdivide, for a quad model with no levels)
  is for a model subdivided in ZBrush or Blender and brought in at its finest
  level: it finds the levels it was made from and puts them back, as many as
  there are, and leaves you on the finest. The model does not move; step down
  with Page Down, change the big forms and come back up with the detail kept.
  It works out whether the quads really came from subdividing only when it
  is pressed, and says so in words if they did not (a quad remesh, or a model
  edited after it was subdivided, has no levels to find). A model of separate
  pieces goes down only as far as every piece can. One undo step.
- **Everything works on the level you are on**: brushes, the mask, hiding
  parts, Check for Printing, a measure, Send to Blender, and export (OBJ, FBX
  and STL write that level, quads included). Go to the finest level before
  exporting the finished piece.
- The **mask** and **hidden parts** follow you between levels. Going down, a
  face is hidden only if everything made from it was: nothing that showed
  goes out of sight. Coming back up, a hidden patch is as you left it. Face
  groups follow too (see Face groups).
- **Reset Shape** on an object with levels goes back to the shape the object
  had when it arrived on the level it is on: a step to another level is where
  the shape is counted from afresh, as a remesh is.
- Every step between levels, every Subdivide and every delete is **one undo
  step**, and undo puts back the levels, the model, the mask and the hidden
  parts exactly. Instances share their levels; Clone and Make Unique copy
  them.
- **Anything that rebuilds the object's mesh drops its levels**: Remesh, Quad
  Remesh, Decimate, Cut & Mirror, Join, the booleans, Trim, Split, a tube or
  muscle fused in, Keys, a mesh that comes back changed from Blender. They
  work on the level you are on, nothing is asked, the status line says the
  levels were dropped, and Undo (Ctrl + Z) brings them back. Strokes, the mask,
  Reset Shape, Set Origin to Mesh and moving the object do not touch them.
- Levels are saved in the `.sclp` file, and in the copies kept in case of a
  crash, and a file opens on the level it was saved on. The card says how
  much memory the levels take (about 35 MB for each million triangles of the
  finest level, beside the model itself, and about 60 MB more while you are
  on a lower level: see below). If that part of a file cannot be
  read, the model still opens, as the level it was saved on, and the status
  line says its levels could not be read.
- A step to a very fine level used to take a moment, because the model was
  rebuilt for the brushes each time. Now stepping down from the finest level
  keeps what it was built into, so stepping back up to it is quick (about a
  tenth of a second for five million triangles, a quarter for ten), with or
  without sculpting done lower down. Stepping down from the finest level
  takes a little longer for it (a few hundredths of a second), and a step to
  a fine level that is not the finest, or the first step up after a file is
  opened, still rebuilds.

## Project Detail

The **Project Detail** card, under Subdivision on the Mesh tab, puts the
detail of one object onto another of the same form, as ZBrush's Project All
does. Its main use is giving a sculpt clean topology:

1. **Clone** the sculpt (the copy sits exactly on the original).
2. **Quad Remesh** the copy: even quads that follow the form, but the fine
   detail is gone.
3. **Subdivide** (Ctrl + D), then **Project**. Repeat until the copy holds
   as much detail as the original. Projecting after each Subdivide, rather
   than once at the end, lets every level take the forms it can hold.
4. Hide or delete the original. The copy has the sculpt's detail on even
   quads, with levels to step down to.

- **From** names the object the detail comes from. It starts as the one that
  overlaps the selected object most, which after Clone is the original; pick
  another from the list and it holds until the app closes. An instance of the
  selected object cannot be picked: it is the same surface.
- **Reach** is how far a point may move, shown in millimetres (it is a share
  of the object's size, 2% to start). Too little and the deepest detail is
  missed; too much and a point may jump to a part it does not belong to, a
  finger to the next finger.
- Each point looks along its own direction, out and in, for the other
  surface, and goes to the nearer hit. A surface facing the other way is
  never taken, so the far side of a thin ear or the inside of a lip is left
  alone. A point that finds nothing along its direction goes to the nearest
  point facing its way, if that is within reach, and otherwise stays; the
  status line says how many stayed.
- The **mask** protects as it does from a brush: a fully masked point stays,
  a half-masked one goes half way. Hidden parts stay where they are.
- Both objects are taken where they stand, each with its own position,
  rotation and scale.
- It is **one undo step** (Ctrl + Z). On a level below the finest it works as
  a stroke on that level would: the finer levels follow when you step up.
- Five million points take about half a second.

## UVs and maps

The **UVs & Maps** card, under Project Detail on the Mesh tab, gets a model
ready for a game engine or a renderer: UVs (the surface laid out flat in a
square) and maps (pictures of the sculpt's detail laid out by them). The
usual way through, after Project Detail's:

1. **Clone** the sculpt and **Quad Remesh** the copy (or Import a low model
   with UVs from Blender or another program, where the sculpt is).
2. **Unwrap UVs** on the copy. **Seams on Face Groups** cuts along the face
   groups' borders, so the seams go where you drew them; without groups the
   unwrap places them. Each quad stays whole in one piece. The model does
   not move, and it is one undo step.
3. **Bake Maps** from the sculpt: pick the maps, their size (1K to 8K) and
   Reach (as Project Detail's), press the button and choose a folder. The
   bake runs in the background, with its progress and a Cancel at the
   bottom of the view, and the status line says where the files went.
4. **Export** the copy as OBJ or FBX: its UVs go with it.

- **From** names the sculpt the detail comes from; it starts as the object
  overlapping the selected one most, as Project Detail's does.
- The maps, each named after the selected object:
  - **Normal** (`_normal.png`): the sculpt's detail as a tangent-space
    normal map, in the MikkTSpace frame Blender, Unity, Unreal and Godot
    read normal maps in, green up (OpenGL; Unreal and DirectX tools want
    the green channel flipped).
  - **Displacement** (`_displacement.exr`, 32-bit, in scene units; and
    `_displacement.png`, 16-bit, with `_displacement.txt` saying what black
    and white stand for, and the Midlevel and Scale for Blender's
    Displacement node).
  - **Ambient Occlusion** (`_ao.png`): how much light reaches each place.
    The slow one: seconds for a 2K map.
  - **Colour** (`_colour.png`): the sculpt's paint, as an export colours
    it.
- Each island is padded out by a few pixels so that a seam never shows the
  empty background; outside the islands a normal map is flat.
- Where the sculpt is further from the low model than Reach, nothing is
  found and the map is left flat there; the status line says how much of
  the surface that was when it is more than a little.
- UVs are kept in the `.sclp` with the model. Sculpting keeps them; anything
  that builds the model again (Remesh, Decimate, dynamic topology's first
  split, Subdivide, a boolean) leaves them behind, as it does face groups.
  Unwrap again after.

## Sculpt layers

The **Layers** card, under Subdivision on the Sculpt panel's Mesh tab, keeps
passes of sculpting apart from the model: a layer of pores, a layer of
wrinkles, a scar, another expression. Each can be turned down, hidden or
deleted without touching the rest, and nothing is lost by trying.

- **New Layer** adds an empty layer and starts recording on it. From then on
  what you sculpt goes into that layer, with any brush and with symmetry, and
  the model underneath stays as it was.
- Each layer is a row: the **red dot** (lit on the layer that records), its
  name (double-click to rename), the **eye**, the bin, its **Strength**
  slider and **Bake**.
- **One layer records at a time, or none.** Click a dot to record on that
  layer; click the lit one to stop. With no dot lit you are sculpting the
  model underneath, and the layers ride on it (see below).
- **Strength** runs from 0% (none of the layer) to 100% (as it was sculpted),
  and the **eye** takes the layer out of the shape and puts it back. Neither
  loses anything: back at 100% the model is the one you sculpted, to within
  the last digit the computer keeps (about a ten-millionth of its size).
- **Delete** (the bin) takes the layer and what it added away. **Bake** makes
  what the layer adds, as it shows now, part of the model underneath, and
  takes the layer off the list: the shape does not change. A layer at 40% is
  baked at 40%, and a hidden one adds nothing. **Bake All** does that for
  every layer.
- **A layer that records but does not show takes no stroke.** If the layer
  with the lit dot is hidden or at 0%, sculpting is refused and the status
  line says why: there would be nothing on screen to see the stroke by. Show
  it, or click its dot to sculpt the model underneath. Painting a mask is
  never refused.
- **Recording below 100%.** What you sculpt looks exactly as you sculpt it,
  whatever the layer's strength, and is kept that much stronger: sculpt at
  50% and the layer holds the stroke twice as deep, so raising the slider to
  100% afterwards doubles it. The card says so while a layer records below
  100%. The most a stroke can be magnified is a hundred times, at 1%.
- **Sculpting the model underneath** (no dot lit) leaves the layers on it.
  A brush that pushes the surface about (every brush but Pose) takes what
  stands on it along as it stands, so a layer hidden afterwards leaves the
  model as the stroke would have left it bare. After a hard pull with the
  Move brush under a layer of deep creases, a faint trace of the creases can
  remain where the pull was hardest (about a quarter of their depth at the
  worst point in the test that measures it; in a picture it is barely
  visible). The **Pose** brush and the **gizmo** turn what they move, and the
  layers turn with the limb, or grow with it when the gizmo scales it.
- **Smooth, Flatten and Clay on the model underneath work on what they see**,
  which is the model with its layers. Smoothing away a wrinkle that belongs
  to a layer, with no layer recording, puts the opposite of the wrinkle into
  the model underneath: hide the layer and a dent is left. To smooth a
  layer's own work, record on that layer.
- **The Erase Layer brush rubs a layer out locally.** Pick it on the Brush
  tab (the last tile), light the dot of the layer to erase, and draw: under
  the ring that layer's work is taken away, back to the model underneath
  with the other layers still on it. Wrinkles come off an eyelid and stay on
  the rest of the face.
  - **Strength** is how much one pass takes, by the brush's falloff: at
    100% (where it starts) the middle of the ring is rubbed right out and
    the edge fades; lower, or a lighter touch of the pen, takes part of it,
    and going over the same place again takes more. It stops at the model
    underneath and never goes past it, however often you go over.
  - What is rubbed out is gone from the layer, not just hidden: turn the
    layer down, hide it, show it again, raise it back to 100%, and the patch
    is still clean. If the layer was at 40% when you erased, raising it to
    100% afterwards does not bring the patch back. (What is left is the
    rounding of the last digit the computer keeps: a ten-millionth of the
    model's size or so, and a few millionths if the layer was down at 1%
    when it was erased.) **Undo (Ctrl + Z)** is how you get it back: one stroke is
    one step.
  - **Ctrl and the right button erase too.** There is no opposite stroke:
    once rubbed out, the detail is in no layer for a brush to bring back.
  - **It needs a layer that is recording and shows.** With no dot lit, or on
    an object with no layers, the stroke is refused and the status line says
    to pick a layer; nothing is added to undo. With the recording layer
    hidden or at 0% it is refused as every stroke is.
  - Masked and hidden parts are left alone, Front Facing Only and symmetry
    work as for any brush, and it adds no triangles with dynamic topology
    on. With an **alpha** it erases through the picture (rub pores out in a
    broken pattern rather than a clean patch); Drag to Place erases one
    print of it.
  - **On a lower subdivision level** it rubs out that level's share of the
    layer: the layer's broad form goes, and the fine detail that only the
    finer levels hold stays. To take the fine detail off, erase on the
    finest level.
- **Every change is one undo step**, and undo puts back every point exactly:
  a stroke on a layer, a drag of a Strength slider (one step however far it
  went), the eye, a rename, a new layer, Delete, Bake, and which layer
  records.
- **Dynamic topology** works with layers. A stroke that adds triangles is
  recorded on the layer, new points and all, and hiding the layer leaves the
  added points lying on the smooth model underneath.
- **Subdivision levels** carry the layers with the model. Subdivide takes
  every layer up to the new level. Stepping down and back up with nothing
  changed gives back every layer exactly. A layer can record on any level,
  and a change of form made on a low level comes back up with the layers
  riding on it: hide a detail layer after reshaping the model on level 1 and
  the detail is gone cleanly. **On a lower level, Strength, the eye and
  Delete wait for the finest level** (Page Up), where a layer's detail is;
  the card says so. A new layer, recording, renaming and Bake work on any
  level.
- **Reset Shape** puts the model underneath back to the shape it had before
  sculpting and leaves the layers on it, standing as they stood.
- **Anything that rebuilds the object's mesh merges its layers into the
  model**: Remesh, Quad Remesh, Decimate, Cut & Mirror, Join, the booleans,
  Trim, Split, a tube or muscle fused in, Keys, a mesh that comes back
  changed from Blender. The new mesh is built from the shape as it shows,
  nothing is asked, the status line says the layers were merged, and Undo (Ctrl + Z)
  brings them back. Set Origin to Mesh, Clone and Make Unique keep them, and
  instances share them.
- Export, Send to Blender, Check for Printing and a measure all see the shape
  as it shows, layers in.
- Layers are saved in the `.sclp` file and in the copies kept in case of a
  crash. A file saved before there were layers opens as it always did. If the
  layers in a file cannot be read, the model opens with the shape it was
  saved with, what the layers added now part of it, and the status line says
  so.
- **Memory**: a layer takes 12 bytes for each vertex of the object, so about
  6 MB for each million triangles; the card gives the figure for the layers
  it shows. An object can have 24 layers.

Not here yet: reordering the list (the order does not change the shape), and
merging two layers into one.

## Face groups

The **Face Groups** card, under Layers on the Sculpt panel's Mesh tab, splits
an object's surface into regions, each with a colour of its own, so that one
can be hidden, masked or sculpted while the rest is left alone (ZBrush calls
them polygroups). A group is simply a set of the object's faces; a face is in
one group or in none.

**Making them**

- **Group Masked**: the masked surface becomes a new group and the mask is
  cleared (tick **Keep the mask** to leave it on). Mask a part with the mask
  brush or the Mask tool, then press it. The group's edge runs where the mask
  is at half strength.
- **Group Visible**: everything that shows becomes a new group. Hide what
  should stay out first; with nothing hidden the whole object becomes one
  group.
- **Auto Groups**: each separate part of the object becomes a group of its
  own: the parts a Join brought together, or shells that pass through each
  other without being one surface. One connected piece is one group.
- **Groups by Angle**: the object is split along its sharp edges, wherever
  the surface turns by more than the **Angle** below the button. Made for
  hard shapes (each flat side of a block becomes a group); on a soft sculpt
  it follows the creases, and a low angle makes a great many small groups.
- **Merge Into...**: with a group picked, press it and click another group on
  the model: the first becomes part of the second.
- **Clear Groups** takes them all off.

Every one of these changes only what shows, as a brush does: a hidden part
keeps the groups it has. Each is one undo step, and undo puts back exactly
the groups there were (and the mask, for Group Masked).

**Seeing them**

- **Show Group Colours** (on the card, and beside Wireframe at the bottom of
  the view) tints every face with its group's colour, over whatever material,
  matcap or shading is showing; surface in no group keeps the material's own
  colour. It is off until the first group is made on an object, which
  switches it on. Off, it costs nothing at all.
- A group keeps its colour for good: through undo, a save and a reopen, a
  remesh. A new group is given a colour that is not close to those of the
  groups it touches.

**Using them**

- **Ctrl + Shift + click a group on the model** isolates it: the rest of the
  object is hidden, so the group can be worked on alone. The same click
  again, or on the background, shows everything. The group clicked is also
  the group **picked**, which the card shows as a swatch of its colour with
  its size, and which the card's buttons work on.
- While a tool is up (Mask, Hide, Trim, Split, Measure, Tube and the rest),
  Ctrl and Shift are that tool's own keys, so there the click is made with
  the card instead: press **Pick**, then click the group. That is also the
  way with a pen and no keyboard in reach.
- **Isolate** does what the click does, for the group picked; **Hide Group**
  hides it; **Show All** brings back every hidden face. These are the same
  hiding as the Hide tool's, and each is an undo step.
- **Mask Group** masks the group picked and unmasks everything else, so
  brushes leave it alone; **Mask All But** does the opposite, so brushes work
  on it alone. Either mask can then be blurred for a soft edge.
- **Stay in Group** is a tick on the Brush tab, under Front Facing Only, kept
  with each brush like its other settings (off unless you tick it). With it
  on, a stroke changes only the group it begins on and stops dead at the
  group's border, whatever the brush: Move, Snake Hook, Pose and Erase Layer
  are held to it like the rest, and so is mask painting. The Pose brush's
  white line, before the press, shows the bend the kept stroke will make. The
  points along a border belong to both groups, so the border itself can be
  sculpted from either side. With mirror or radial symmetry on, the stroke is
  kept to the groups its sides begin on, the left side's and the right
  side's, or each repeat's; where two of those groups lie within a brush's
  width of each other, a side can reach a little way into its neighbour's.
  Shift smooths with the Smooth brush's own tick, not the tick of the brush
  in hand. On an object without groups it does nothing.

**What happens to them**

- **Dynamic topology** keeps them: a face split from another is in its
  group, and the border between two groups stays where it is.
- **Subdivision levels** carry them. Subdivide gives each new face its
  parent's group; a step down gives each coarse face the group most of it is
  in; and stepping down and back up with nothing changed gives back exactly
  the groups that were there, to the face. Groups can be made and changed on
  any level; the other levels then take theirs from that one.
- **Sculpt layers** and groups do not touch each other.
- **Everything that rebuilds the object's mesh keeps the groups**, by giving
  each new face the group of the old surface nearest to it: Remesh, Quad
  Remesh, Decimate, the booleans, a Join with a remesh, a mesh that comes
  back changed from Blender. The borders come back where they were to within
  a face of the new mesh. Cut & Mirror gives the rebuilt side the mirror
  image of the kept side's groups. A plain Join keeps every object's groups,
  and where two objects used the same group the joined object has two
  groups, not one (the object joined into keeps its colours). Trim, Split
  and Keys keep the groups on the surface that is left, and the new surface
  over a cut is in no group. A mesh that comes back from Blender as another
  shape altogether starts without groups, and the status line says so.
- Reset Shape, Set Origin to Mesh, Clone, Make Unique and a live mirror made
  real keep them exactly, and instances share them. An array made real, as
  one object or as separate ones, gives every copy the object's groups.
- Groups are saved in the `.sclp` file and in the copies kept in case of a
  crash. A file saved before there were groups opens as it always did. If
  the groups in a file cannot be read, the model opens whole without them and
  the status line says so.
- **OBJ export** writes each group as a `g` group (`Group_3`), quads as quads,
  which is how ZBrush writes its polygroups and most programs write theirs;
  **OBJ import** reads `g` groups back as face groups. A file whose faces are
  all in one group is read as having none. STL and FBX carry no groups.

An object can have 65,535 groups, far more than the colours can tell apart;
a few dozen is the practical limit for working by eye. The card never lists
them: a group is found by clicking it on the model. Groups take 2 bytes for
each triangle.

Not here: groups are not sent to Blender (they are put back on the model
when it returns).

## Remesh

**Remesh** (R), in the Remesh card on the Sculpt panel's Mesh tab, rebuilds
the selected object as one closed surface of even triangles at the card's
**Resolution**: how many cells fit along the model's longest side. It fuses
parts that pass through each other and closes holes, and says when it did.

- **Resolution goes from 4 to 4096.** It used to stop at 1024, which is
  about 10 million triangles on a compact model. The line under the slider
  says about how many triangles a number makes; past the limit of 80
  million it says so, and what number would do. Measured on this machine
  (a lumpy ball, 32 threads, the new sculpt mesh and the check for a closed
  surface included): 1024 makes 9.6 million triangles in 0.7 s, 2048 makes
  38 million in 3.2 s, and 3072 makes 80 million in 8 s. Building the
  sculpt mesh was most of that time, and is now about a third quicker,
  which also speeds up opening a big file.
- **How it sculpts up there.** At 40 million triangles a small brush (40
  pixels across) takes 3 to 6 ms a frame and a large one (150 pixels) 10 to
  40 ms; a large Snake Hook slows to about 150 ms. Turning the view stays
  under 10 ms a frame. A mesh takes about 78 bytes of memory a triangle (6
  GB at 80 million), and again for each step undo can go back to.
- **It works in the background**, as Decimate does: its progress and a
  **Cancel** button show at the bottom of the view, and the view can be
  turned meanwhile. Cancel leaves the model as it was. If the model is
  changed before it finishes, the result is of a model that is no longer
  there and is not used; the line says so. One at a time, and not while
  another job is working.
- **Smooth**, under Resolution, goes from Off to 10 and starts at 2. A
  remesh copies the old surface exactly, its flat faces and every crease
  included, so after a step up in resolution the old triangles come back as
  ripples, along a groove the rebuild leaves small stair-steps of its own,
  and detail finer than the new triangles (the fibres of a muscled body
  remeshed low) comes back as scales and pits. Above Off, the old mesh is
  first rounded into a finer smooth one where its triangles are bigger than
  the new ones (sharp edges, past 60 degrees, kept); the rebuild places
  everything that turns by less than 60 degrees within one cell as the
  smooth surface it nearly is, and only sharper edges as creases; and the
  result is then smoothed lightly that many times, without shrinking it.
  Off is the remesh exactly as before. On a grooved test ball stepped up
  from 128 to 300, the shading noise fell by more than half, with the shape
  no further from the true one. It costs a little time, not sculpting
  speed (the triangle count is the same): measured here, a step up from
  2.4 to 9.6 million triangles went from 0.7 s to 1.0 s, to 38 million from
  4.0 s to 5.6 s, and a remesh at the same resolution took 0.03 s more. The
  line under the slider says what the number does. Join's remesh and the
  booleans are not smoothed.
- **One undo step**, the old mesh kept whole. It never mirrors (see
  Symmetry). Join's Remesh Resolution is the same setting. The booleans'
  Voxel Resolution is a setting of its own and still stops at 1024, since a
  boolean still runs in the window's own thread.

## Decimate

**Decimate**, at the foot of the Remesh card, takes triangles off the selected
object and keeps its shape. It is for a model that has to fit a triangle
budget, a game asset above all: a finished sculpt runs to millions of
triangles, and an engine usually allows one model somewhere between 200k and
500k.

- **Target Triangles** is the count to come down to, from 1k to 4M. It starts
  at 200k, and the app remembers it. The figure in brackets is what share
  that is of the triangles the object has now, and **75%**, **50%**, **25%**
  and **10%** set the target to that share. **Decimate** does it in one go;
  **Preview** shows it first (below). The buttons are grey while the object
  is at or under the target.
- **Preview** shows the model decimated before anything is changed, and
  follows the slider: drag Target Triangles and the model is shown at that
  count at once, so the count can be chosen by eye, with Wireframe on to see
  where the triangles went. **Apply** (Enter, or the Decimate button) keeps
  what is shown as one undo step, the very mesh Decimate would have made;
  **Cancel** (Esc) leaves the model as it was. The first press works the
  decimation out in the background, all the way down to 1k with every fold
  written down (a few seconds on a large model, with its progress and a
  Cancel at the bottom of the view); after that any count is put together
  from what was written down in a few hundredths of a second. The model
  itself is not touched while the preview is open: saving, the recovery
  copies and every command see it as it is. A press on the model turns the
  view instead of sculpting; Undo closes the preview; any other command
  that is not only about the view (a Remesh, a save, a mask) closes it
  first and then runs. This is ZBrush's Decimation Master pre-process and
  Cinema 4D's live Polygon Reduction, the slider drawn on the model.
- **Where the triangles come from.** Flat and gently curved surface gives its
  triangles up first; ridges, creases and fine detail keep theirs longest.
  (It is quadric edge collapse, Garland and Heckbert's method: an edge is
  folded to the one point that stays closest to the surface it replaces, and
  the cheapest edge of the whole model goes first.) What is left is uneven on
  purpose: big triangles where little happens, small ones where the form
  turns. A Remesh does the opposite, even triangles everywhere, so decimate
  last, when the sculpting is done: brushes work poorly on big triangles.
- **A closed model stays closed.** No fold is made that would tear the
  surface, pinch it or turn a triangle over: a hole stays one hole with its
  rim where it was, and a gap that goes right through (under an arm) stays
  open. The status line says how it was and how it is, by the same count of
  open edges Check for Printing makes: "Decimated "Sphere" from 202,800 to
  50,700 triangles in 253 ms. It was a closed surface and still is. Undo (Ctrl + Z)
  brings the triangles back." For a model that had holes it says that it was
  not closed before and is not now, in red, with what closes it.
- **The count is a target.** When no fold is left that keeps the surface
  whole, it stops above the count and says so ("4 were asked for: any fewer
  would tear or fold the surface"). That only happens far below any budget.
- **The mask protects**, as it does from brushes. Fully masked surface keeps
  every point and every triangle exactly as it was: mask a face, decimate,
  and the face keeps its detail while the rest of the figure pays for it.
  If the masked part alone has more triangles than the target, it stops
  there and the line says why.
- **A softer mask keeps more detail, the stronger it is.** Surface masked
  short of fully is decimated, but folding it costs more, so it keeps more
  of its triangles: measured on a lumpy ball, half of it masked at 50% kept
  about four times the triangles to the area the unmasked half did, and at
  95% about thirteen times. The count is the same; the rest pays. (Each
  step of mask strength multiplies what a fold there costs, up to a
  thousand times; triangles left go with about the cube root of that.) It
  is how ZBrush's Decimation Master reads a mask, and Blender's Decimate
  modifier a vertex group. The mask is carried onto the new points.
- **Hidden parts are decimated with the rest, and stay hidden.** The budget
  is the whole model's, as is the file an export writes.
- **Face groups stay as they were.** Every face left is one the model had,
  and it stays in its group, so a group's border stays on the faces it ran
  along. (With a mirror on, see below, that is so on the side decimated;
  a face of the other side takes the group of the model's face nearest it.)
- **A mirror is kept exactly.** With a Local mirror on (X, Y or Z, or any
  two or all three), the model is cut at the mirror planes, the part on the
  plus side of each is decimated, and that part is mirrored back, so the
  sides of the result are mirror images of each other point for point and
  triangle for triangle; the line says so ("Its sides mirror each other
  across X, point for point."). The cut is Cut & Mirror's own, with the
  Symmetry card's Match Tolerance for which points count as on the plane,
  and the edge of the cut is held to the plane while the part is decimated,
  so the halves join up again into one closed model. Nothing is asked. With
  a mirror it is also about twice as quick: half the model is decimated.
  This is what ZBrush's Decimation Master and Blender's Decimate (Symmetry)
  do.
- **Where the sides are a little off, each keeps its own shape.** A model
  sculpted with a mirror is seldom exactly symmetric, so the other side is
  not thrown away. Where it is further from the mirror image of the
  decimated side than the Match Tolerance (half a triangle's width, about),
  its surface is paid for in triangles as well, on both sides, and
  afterwards its points are moved onto its own shape; the line says how far
  apart the sides were at most ("where they were not alike (up to 0.82 mm
  apart), each kept its own shape"). Nearer than half the tolerance the
  sides count as the same and come out exact. Where a point cannot be moved
  onto its own shape without folding a triangle over, it is not, and the
  line says how many and how far off they are. The mask protects on both
  sides the same way: a part masked on either side keeps its detail on
  both, and each side keeps its own mask. Hidden parts too: each side keeps
  its own.
- **Where the sides are two different shapes, nothing is mirrored.** When
  anywhere the other side is more than eight Match Tolerances from the
  mirror image (a door on one side only, a cut a few millimetres deep), mirrored
  triangles could not follow the difference, so the model is decimated
  whole, as it is, exactly as with no mirror on, and the line says so:
  "With Mirror X on, its sides are not mirror images of each other (up to
  12.30 mm apart), so it was decimated as it is: each side keeps its own
  shape, and their points do not pair up." The preview's line says it when
  the preview opens. A mirror switched on for the brushes never makes one
  side over into the other: that is how Remesh once lost a door
  (docs/remesh-symmetry-fix.md), and why Remesh ignores symmetry.
  (Eight, because Zane's 13-million-triangle figure, sculpted with Mirror X,
  has all of one side within eight tolerances of the other's mirror image
  and 99.8% of it within one: at four it was taken for two shapes over a
  few specks 0.65 mm off.)
- **Radial symmetry is not kept exactly, so it asks first.** The repeats of
  a radial symmetry are thinned each in its own order, so afterwards they
  match to the eye and their points no longer pair up. Brushes still work on
  all of them. When the object has a Local radial symmetry on, a question
  says this and offers **Decimate Anyway** (Enter) or **Cancel** (Esc); a
  mirror on as well is still kept. The preview does not ask; its line says
  the repeats will not match. A World symmetry is not asked about, and its
  mirror is not kept: the world's planes cut the mesh anywhere, and it was
  never exact in it. The other way would have been to refuse until the
  radial symmetry is turned off. That was not done because the way past a
  refusal (untick it, decimate, tick it again) gives exactly the same model
  in more steps; the only thing to decide is whether exact repeats matter
  for this model, so that is the question.
- **It works in the background.** Its progress and a **Cancel** button show
  at the bottom of the view, and the view can be turned meanwhile. Cancel
  leaves the model as it was. If the model is changed before it finishes (a
  stroke, an undo, a remesh), the result is of a model that is no longer
  there and is not used; the line says so, and Decimate is one press away.
  One at a time, and not while a merge or a cut is still working.
- **One undo step**: Ctrl + Z brings every triangle back (the mesh from before is
  kept whole, and counts towards undo's memory). Instances share the mesh,
  so they all change. A quad-remeshed object comes out as triangles, and
  Reset Shape goes back no further than the decimated shape.
- **How long.** Measured on this machine, the new sculpt mesh included:
  200k to 100k triangles in 0.17 s, 1M to 500k in 1.3 s, 1M to 200k in 2.3 s.
  In one run on 22 October 2026, with other work on the machine: 1M to 200k
  in 3.7 s whole and 1.8 s with Mirror X; working the preview out for 1M
  triangles 5.3 s (64 MB held while it is open), and then each count shown
  in 44 ms (12 ms for a 200k model).

## Transform gizmo

Press **G** (or the Transform button) to move, turn and resize the selected
object as a whole. Drag an arrow to move along it, a ring to turn about it, the
small cube on an arm to stretch that way, or the white cube in the middle to
resize evenly (up grows, down shrinks). Click another object to select it
(Shift or Ctrl + click adds it to the selection, or takes it out again);
dragging anywhere else orbits. Each drag is one undo step. **Esc** during a
drag puts the objects (or, with a mask, the reshaped surface) back where they
were when the drag began, and leaves no undo step; Esc with nothing being
dragged puts the tool away, back to sculpting, as picking a brush does (from
the panel, the Space wheel or a number key). **Reset Rotation and Scale** in
the Scene panel puts an object back square.

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
for the opposite side). Top and bottom always come up straight. An axis view
is orthographic, and the first turn you make by hand after it (a drag on the
view or on the gizmo) goes back to perspective, as Blender's Auto Perspective
does; orthographic picked with the **Ortho / Persp** button stays on through
turns. Either switch keeps the model the size it is on screen: the button
used to make it jump when a pan or zoom had left the camera's pivot in front
of the model or behind it.

**Lens**, under the Ortho / Persp button, is the lens the view is seen
through, named in millimetres as a camera's lenses are (18, 24, 35, 50, 85
and 135 mm, and a slider for anything between 12 and 135). Wide lenses bend
the model toward you, as a camera held close does; 50 mm is about what the
eye sees; 85 mm and up flatten it, the way portraits are taken, which shows
the proportions of a face or figure truest. The model stays the size it is
on screen: the camera steps back as the lens gets longer. Picking a lens
goes into perspective, the only view a lens shows in. The view starts at
about 21 mm. New scenes and the next start keep the lens last picked; a
saved sculpt opens with the lens it was saved with.

Down the left of the gizmo's three buttons are three round handles, so the
view can be moved every way with the pen alone, no keyboard or middle
button in reach: drag the **hand** (beside Persp) to pan, the model following
the pen as with a middle drag; drag the **magnifier** (beside Stroke Pivot)
up to come nearer and down to go further, as with Ctrl + Alt + drag; tap the
**house** (beside Frame) for **Home**, which glides back to the view a scene
starts with: the whole scene from a little to one side and above, in
perspective. **Numpad 5** switches between orthographic and perspective, as
in Blender, like the Ortho / Persp button. The handles sit beside the
buttons rather than in a row of their own so that the gizmo does not grow:
the panels' fold tabs hang under it, and a small window has no room to
spare.

### Deform

The **Deform** card under the Transform card reshapes the whole selected
object along one of the gizmo's axes, the way ZBrush's gizmo deformers do.
Pick one of its eight tiles and the gizmo's arrows give way to the
deformer's own handles: a line along the axis with a round handle at each end
of the range it works over.

| Deformer | What it does to the clay |
|---|---|
| **Bend** | Bends the shape round by **Angle**, toward **Direction** (0° bends Y toward Z, Z toward X, X toward Y). |
| **Bend Arc** | Bends it into the smooth arc that ends where you drag the yellow tip handle: horns, tails, tubes. |
| **Twist** | Turns it about the axis, more and more along it, by **Angle** at the far end. |
| **Taper** | Makes it thinner or fatter across: **Start Size** at the start of the range, **End Size** at the end. |
| **Flatten** | Squashes it along the axis by **Amount**; 100% presses the range flat. |
| **Inflate** | Puffs the surface out along the way it faces by **Amount** (a share of the model's size); below 0 sucks it in. |
| **Stretch** | Makes the range **Length** long; with **Keep Volume** it thins as it lengthens, as clay does. |
| **Lattice** | A cage of 2 to 6 points each way round the shape: drag a white point and the clay inside follows smoothly. |

- **Along** X, Y or Z picks the gizmo axis (its red, green or blue arrow);
  **Gizmo axes** above sets which way those point, as for the arrows.
- **Range** Start and End (or the two round handles, dragged along the axis)
  limit the deformation to part of the model; **Softness** eases it in and
  out at the ends of the range so no crease is left there. Bend, Twist, Taper,
  Flatten and Stretch carry the rest of the model beyond the range along with
  them, as one piece; Inflate and Lattice change only what is inside it.
- **Stays Put** picks the part that does not move: the start of the range,
  its middle (both ends move alike) or its end.
- Every value has a number box beside its slider: double-click it to type.
- What you see is not kept until you press **Apply** (or Enter). **Cancel**
  (or Esc) puts the model back exactly as it was; Esc again puts the
  deformers away. Anything else you do (another tool, a click on another
  object, Z, a save) applies it first, so Ctrl + Z right after a bend takes the bend
  back and Ctrl+Shift+Z puts it on again. **Done** keeps it and brings the arrows
  back. Each Apply is one undo step.
- The masked part stays where it is and a half-masked part goes half way, so
  mask the body to bend only an arm. Hidden parts stay where they are. Stay in
  Group belongs to brushes; to deform one face group, use **Mask All But
  Group** first.
- With several objects selected they are deformed together, as one row or
  group, about their shared centre; one Ctrl + Z takes them all back.
- With sculpt layers, a deformation goes where a stroke would: into the layer
  that records, or into the model underneath with the layers' detail carried
  round with it. It is refused, as a stroke is, while the recording layer is
  switched off or at 0%. On a coarser subdivision level it is carried up to
  the finer levels like any sculpting there.
- With a mirror ticked on the Symmetry card, the status line says in red if
  the deformation would make the two halves differ: a bend that leans to one
  side of the mirror plane, a twist (which always turns one half one way and its mirror image the
  other), or a lattice point moved on one side only.

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
| Esc | Cancel a drag, else deselect, else put the tool away (the spheres stay) |
| A | Solid preview of the skin |
| X | Symmetry across the red plane |
| Space (hold) | SSphere wheel, with the size of the next sphere under it |
| Ctrl + Z, Ctrl + Shift + Z | Undo, redo the spheres (they keep their own history) |
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
not written to `.sclp` files: make the skin before closing the app. The app
asks before it closes over spheres that would be lost (two or more, not yet
made into a skin, or changed since the skin was made), saved scene or not; it
does not ask once Make Skin has used them as they stand. Opening another
sculpt leaves the spheres standing where they are, let go of the object they
were attached to.

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
on the undo history. Closing the app does not ask about them: a ruler is a
reading, and one drag brings it back. Opening another sculpt leaves them where
they are. The **Measure & Size** card in the Scene panel is there in
every tool: it shows the selected object's real width, height and depth, and
typing a size into one ("50", "2in", "1 1/4in") resizes the object as one undo
step, evenly or along that axis alone.

## Array

Press **Y** (or the Array button) to repeat the selected object along a line,
round a circle or along a curve you draw. The copies are live: each is the
object's own mesh drawn again, so every brush stroke shows on all of them, and
moving, turning or resizing the object carries the whole array. They stay on
screen when you go back to sculpting, and they count as the object, as in the
Unity app: a stroke begun on any copy sculpts the object through that copy, a
click on one picks the object, and Mask, Hide, Trim and Split act through them
(see below).

1. **Line** or **Circle** makes an array in one click, spaced so the copies sit
   side by side. **Draw Curve**, then a drag in the viewport, makes the copies
   follow the curve you draw (a spine, a chain, a bracelet); end the stroke
   where it began to close it into a loop.
2. Reshape it with the handles. **Copies** sets how many, **End Size** tapers
   or grows the run, **Middle Size** swells or pinches it, and the ticks
   choose whether copies turn to follow the path, are spaced by size, or close
   into a loop. A circle also has the world axis it goes round, a Sweep and a
   box to go the other way round (the same arc, run from the other side).
3. **Mirror to the other side** (X, Y, Z) repeats the object and every copy
   on the other side of the scene's centre. It is the object's own live Mirror
   (the Mirror section of the Objects card, see Objects) within reach: the
   same switch, so it stays live, and an object that is already mirrored has
   its array mirrored from the start. With X and Z ticked there are three
   reflections of each copy, as there are of the object.
4. **One Object** turns the copies into real geometry inside the object (a
   remesh then fuses them); **Separate Objects** makes each copy an object of
   its own. Either is one undo step, and Ctrl + Z gives the live array back. Large
   meshes are built on a background job, with the array on screen until it lands.
   A mirrored array stays mirrored: the object keeps its mirror, and each
   separate object gets the same live mirror.

| Input (Array up) | Action |
| --- | --- |
| Drag a point | Move it, on the plane facing you |
| Click a point | Pick it: the gizmo's arrows sit on it |
| Drag an arrow on a picked point | Slide it along that axis |
| Drag the square start point | Move the object, and the whole array with it |
| Click the path | Add a point there, and keep dragging it |
| Shift + drag the last point | Extend the path from there |
| Right-click a point, or Delete | Delete it |
| Click a copy (on either side of a mirror, or the object) | The gizmo's arrows and rings sit on it |
| Drag the gizmo on a copy | Turn or move them all |
| Click another object | Switch to it (each object keeps its own array) |
| Esc | Cancel, then stop drawing, then let go of the point or the copy, then put the tool away |

**Sliding a point.** A picked point (the end of a new line or circle is picked
for you) has the gizmo's arrows on it, and only the arrows: a point can be
moved but not turned or resized. The dot itself is still the free move, on the
plane facing you; an arrow is a straight pull along one axis, for placing a
point exactly. The copies follow as the layout says. One drag is one undo
step: for any point but the first it is one of the array's own, and Ctrl + Z puts the
point back exactly where it was. The first point is the square start point,
the object's own place, so there the arrows carry the object and the whole
array together, as dragging the square does, and the step is the scene's.
Clicking a copy moves the gizmo to the copy, and clicking a point moves it
back; the gizmo is on one of them at a time. It is not drawn in the clean
view, and goes with the tool.

**Posing the copies.** Click a copy (off the path: a click on the path adds a
point) and the gizmo's arrows and rings sit on it; turning or moving that one
turns or moves them all. Turning it turns the object, and every copy turns the
same way in place, each in its own frame on the path. Moving it moves the
object off the path's start and leaves the path where it is in the world, so
every copy moves the same way in its own frame (turned with the path, scaled
with its size): a row of links all lifted off a curve, or pushed out from it.
(Set Origin to Mesh, in the Objects card, leaves the object off the path's
start in the same way, without moving a copy: see Objects.)
On a mirrored array a copy on the far side can be picked too: dragging it right
moves the original left, and a turn is turned the other way. The gizmo has no
cubes, since a copy's size is set with the sliders. **Copies Back onto the
Path** (it lights up once copies are off the path) undoes any move made this
way and leaves a turn.

**The copies are the object's.** The Array tool says every frame where its
copies are, and the rest of the app treats each as one more place the object is
shown, as it does a live mirror copy or an instance: a brush stroke begun on a
copy is mapped through that copy, so it sculpts exactly the vertices the copy
shows under the pointer (the ring follows the pointer on the copy, and the
other copies and the object show the stroke as they do every stroke); a click
or double click on a copy picks the object; and the camera frames them. A
tapered array's copies are smaller, and the far side of a mirrored array is
reflected, and a stroke or a shape works through both as it does on the object.
Mask, Hide, Trim and Split are described with them below.

Dragging on the background orbits (except while Draw Curve is on). Arrays are
the tool's, not the scene's, and while the tool is up Ctrl + Z steps the array's own
edits first, then the scene's. The exceptions are a pose, Copies Back onto the
Path and a mirror switch: each is a step of the scene's history (it changes the
object, or its mirror), but Ctrl + Z takes it back in order with the array's own
edits, and the array follows when the scene takes one back, even with the tool
away. Moving the object by its square start point is a step of the scene's
alone, which Ctrl + Z reaches after the array's own edits.

**Arrays in files.** An array is saved in the `.sclp` file with its object, so
closing the app does not lose it: save, close, open, and it is there, live, on
the same object, with its settings, its path, its pose and its mirror. Making
an array, or changing its count, its sizes, its ticks or its path, counts as a
change to save although it is the Array tool's own step and not the scene's:
closing asks first, and a recovery copy is kept of it (see Files). The file
keeps what makes the copies (the kind, the count, the sizes, the ticks, the
axis and sweep, and the path's points as offsets from the object), as the
Unity app's did. The pose needs nothing of its own: it is the object's saved
place and the path's first point; and the mirror is the object's own live
mirror, which the file already keeps. Every object keeps its own array, whichever
row it is on and however many there are. A file saved before this opens as it
did, with no arrays, and a file with arrays opens in an older version of the
app with its objects and without them.

- **Open** (in place of the scene) gives each array to the object of the same
  row in the file. The arrays of the scene it replaced are not thrown away:
  Undo brings that scene back with its arrays as they were, and Redo brings
  the file's. The Array tool's own undo steps end at an Open, so while the tool
  is up the first Ctrl + Z undoes the Open itself, not an array edit to an object that
  is no longer there.
- **Import** adds the file's arrays to its objects as they join the scene. The
  scene's own arrays, and the tool's steps, are untouched, and one Undo takes
  the added objects and their arrays away.
- A file that is damaged or from a newer version costs the array that cannot be
  read and nothing else: a kind this version does not know, fewer than two
  points, or a point that is not a place means no array; a count outside 2 to
  100 or a size out of range is brought into range, and a path of more than 256
  points is cut to 256; a setting that is missing or is not a number is what a
  new array starts with.

## Lathe

Press the **Lathe** button to turn a solid like clay on a wheel. A starting
vase stands beside the model with its outline drawn over it: a curve through
round handles on both sides of a dashed axis. Drag a handle and the solid
follows live. **Create Mesh** (Enter) makes it a new object to sculpt, selected
and ready, in one undo step; Ctrl + Z right after takes it away again and brings the
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
closed loop, cap open ends, even triangles, **Radial Symmetry on Create**, and
the Density, Radius, Height and Scale sliders; the preview is drawn at up to
256 around and Create always uses the full Density. Dragging on the
background orbits. The profile is a scaffold: not saved, with its own Ctrl + Z while
the tool is up. The app asks before it closes over an outline you have shaped
and not yet created (a preset as it was loaded is nothing to lose); opening
another sculpt leaves the profile as it is.

**Radial Symmetry on Create** (off to start with) gives the new object radial
symmetry of its own, round the axis it was turned about (its own Y, which runs
through its middle), so every stroke you make on it is repeated all the way
round: eight times, which is what every object starts with, and the Symmetry
card changes it. It is the new object's symmetry, as in the Unity app. The
object that was selected before keeps its own, Ctrl + Z right after Create takes the
symmetry away with the object, and Ctrl + Shift + Z brings both back. Unticked, the
new object starts with no symmetry, like any other.

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
   pointer shows how thick the next tube will be. A tube follows the symmetry
   of the object it grows from, as brushes on that object do (each object has
   its own: see Symmetry): with a mirror axis on, one stroke makes both arms;
   with **Radial** on, it makes one tube for each repeat evenly round the
   radial axis (a ring of horns), and twice as many with a mirror as well. The
   copies' curves and points are drawn fainter, and grabbing one of their
   points edits the tube through the mirror or the turn. Change that object's
   symmetry and the copies change with it; the Tube panel says what the
   selected object's is. (A tube drawn on an object other than the selected
   one is under that object's symmetry, not the selected one's, as in the
   Unity app.)
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
   The object is rebuilt at its current density, as a remesh does (and as it
   was in the Unity app), so it starts with a clear mask and everything
   showing; undo brings the old mask and hidden parts back with the old model.
   Every copy under the symmetry is fused in the one merge: across a mirror
   plane the result is exactly symmetric, and round the radial axis it is the
   same all the way round to within a fraction of one cell of the rebuilt
   model. The work runs in the background, with its progress and a Cancel
   button (or Esc) while it does. **Keep Separate** makes each tube (all its
   copies together) an object of its own to sculpt, placed with the object it
   was drawn on and starting with that object's symmetry settings, and leaves
   that object, its mask and its hidden parts as they were. Either is one undo
   step.

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
| Drag the square beside the selected point | Thicker / thinner there (a muscle's point has a square on each edge, which moves that edge alone: see Muscle) |
| Scroll over a point | The same |
| S + move sideways | Thickness of the point under the pointer, else of the tube |
| Enter | Merge |
| Ctrl + Z, Ctrl + Shift + Z | Undo, redo the tubes' own edits |
| Esc | Cancel the drag, then deselect, then put the tool away |

Dragging on the background orbits, as everywhere else (the Unity app drew a
tube there; here that is Ctrl + drag). Live tubes are a scaffold: not saved in
`.sclp` files, and drawn only while the tool is up. Put it away and they wait,
out of sight, until it is brought back (the Unity app hid them too: to see a
tube among the rest of the model while you sculpt, merge it or keep it
separate). The app asks before it closes over live tubes, saved scene or not,
and before a sculpt is opened in place of the scene: a tube stands on the
object it was drawn on, and goes when that object does (adding a sculpt's
objects to the scene keeps them). While the tool is up Ctrl + Z steps the
tubes' own edits first (a point moved, a slider changed), then the scene's.
Merge and Keep Separate are steps of the scene's history: Ctrl + Z right after one
gives the tubes back live, with the tool brought up if it was put away, and
Ctrl + Shift + Z fuses them again. A tube thinner than the model's own detail comes
out thicker than drawn: undo, remesh the model finer, and merge again.

## Muscle

Press **B** (or the Muscle button) to block out a body: draw a line on the
model and a muscle lies along it, thin at both ends, full at its belly, wider
than it is deep, and sunk into the form. It is the Tube tool with another shape
in hand: a muscle stays live the same way, is shaped by the same points with
the same gestures, has the same undo, and is finished by the same Merge and
Keep Separate. The two tools keep their own shapes: a tube is the Tube tool's,
a muscle the Muscle tool's. This tool is this app's own; the Unity app has no
muscles.

What is different from a tube:

- **It lies on the model all the way along**, not only at its points. A
  straight line drawn over a shoulder is two points, and the muscle between
  them follows the shoulder. It reads the surface at its own size, so on a
  finely detailed model it rests over the ridges and takes the lie of the form,
  not of every groove under it.
- **A rough line is enough.** The stroke is judged as you drew it on screen:
  wobbles smaller than a quarter of the muscle's width are dropped, so a shaky line
  gives the same clean muscle as a careful one, with few points to edit. While
  the pen is down the muscle follows the line more closely, so it does not jump
  each time the line bends enough for another point; it settles onto its few
  points as the pen comes up.
- **It lies steady under the pen.** Each point takes the way up of the form
  under the muscle, felt at the muscle's own width, not the tilt of the one
  triangle it landed on, so dragging a point over a finely detailed model, or
  drawing over one, slides the muscle along rather than rocking it about.
- **A stroke that starts just off the model is pulled onto it**, to the
  nearest place on its outline, as is one that strays over the edge on the way.
  Further out than that a drag orbits, as everywhere.
- **The same stroke on the same model gives the same muscle**, every time.
- **It follows the model.** Move, turn or scale the object and the muscle goes
  with it. Sculpt the model under a live muscle and it is laid on the new
  shape the next time the tool is brought up (and straight away after an undo
  or a redo of the scene, a remesh or a merge); its points are shown where the
  surface is now.

The panel shapes the selected muscle, and the next one you draw:

| Setting | What it does |
| --- | --- |
| **On Surface** / **Free** | Where new points go, and where a dragged point goes: on the model, or floating. See below. |
| **Width** | How wide the muscle is at its belly (S + move sideways does the same). |
| **Swell** | How full it is: low is a lean spindle, high stays wide almost to its ends. |
| **Belly** | Where along it the widest part sits: 50% is the middle. |
| **Taper** | How thin the two ends are: 0 is as wide as the belly, 100% comes to a point. |
| **Thickness** | How deep it is for its width: low is a flat strap, 100% is round. |
| **Depth** | How far it is sunk into the model, from resting on top to grown out of it. |
| **Smoothness** | How loosely it flows between its points. |
| **Blend** | How soft the join is when merged, against how far the muscle stands off the model. |

Each point can still be made thicker or thinner than the muscle's own width
there, by the wheel or **Point Size**: a point in the middle of a muscle,
made thicker, is a second belly.

**One side at a time.** The selected point has a square on each edge of the
muscle. Drag one and only that edge moves, in or out; the edge on the other
side stays where it is (the point slides across to keep it there, over the
model). That widens a muscle toward the open side without spreading it into
the muscle beside it. Shift + drag a square sizes both sides evenly. A muscle
drawn as one straight line has only its two thin end points: click its
middle to add a point there first.

**Show Points: Selected Only** hides the points of every muscle but the
selected one, so on a body covered in muscles the others' points are out of
the way and cannot be grabbed by mistake. Their lines still show: click one
to select it and bring up its points. **All** shows them all again.

**On the surface and floating are not two kinds of muscle.** Each point is one
or the other, and a muscle can have both: its two ends on the model and its
middle lifted clear, bridging a hollow. With **On Surface** a dragged point
slides over the model, and a floating point dragged onto the model sticks to
it. With **Free** a stroke is drawn on a plane facing you, just in front of
everything it passes over (so none of it is hidden inside the model), or
anywhere at all with Ctrl + drag; and a dragged point lifts off the surface
and floats. A floating muscle is as flat as **Thickness** says, turned to face
the view it was drawn from; a point lifted off the surface keeps the way it
was turned. **Depth** only acts where a muscle lies on the surface.

**Merge** is sized for muscles. A tube's soft join is measured against its
radius; a muscle is wide and low, and a join that size swallowed it, so a
muscle's **Blend** is measured against how far it stands off the model, and
starts low (0.2). On a finely detailed model keep it low or at 0: a soft join
there draws the detail up the sides of the muscle, and is most of the time a
merge takes. For blocking a figure, **Keep Separate** costs nothing and leaves
the model untouched: each muscle becomes an object named Muscle.

**Nothing is left open under a muscle.** Where it bridges a groove or a dip,
where a rounded form falls away beside it, or where a point has been lifted off
the model, the space under it is filled down into the model, so Merge leaves
no pocket or overhang underneath. The fill is part of the live muscle too
(hidden inside the model), and of a muscle kept separate, which therefore sits
on the body with no gap under it. A part floating more than a few widths off
the model is left floating.

The keys and gestures are the Tube tool's (see the table above), with B in
place of T. Live muscles, like live tubes, are a scaffold: not saved in
`.sclp` files, drawn only while the tool is up, and the app asks before it
closes over them.

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

**A mask from the surface's own shape.** In the **Masking** card (Mesh tab),
under **From the Surface**:

- **Mask Recesses** masks what lies in creases and pits: wrinkles, cuts, the
  foot of every bump. **Mask Peaks** masks what stands proud: ridges, the
  tops of bumps, edges. Either takes the place of the mask there was, and is
  one undo step.
- **Amount** is how much of the surface the next press takes: low for only
  the sharpest creases or ridges, high for anything that lies low or stands
  up at all. It starts at 70%. Move it and press the button again.
- To **sculpt only in the wrinkles**, press Mask Recesses and then
  **Invert**: everything but the recesses is now protected. To **polish only
  the high spots**, Mask Peaks and Invert, then Smooth or Flatten.
- A flat face, a plain ball and any evenly curved surface are neither and
  are left alone. The same model twice as big masks the same places, and so,
  near enough, does the same shape with more or fewer triangles (a wrinkle
  only two triangles wide is shallower in the mesh itself, and reads so).
  Hidden parts keep the mask they had.

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
Extract up a drag orbits as usual.) They work on the selected object, under
its own symmetry. With a mirror axis on, a mask or a hide covers the other
side too, and a cut repeats there; with radial symmetry on (see Symmetry) they
repeat all the way round the axis; with the symmetry in World space the other
side is across the world's plane. Each mask, hide, trim, split and extract is
one undo step, and a mask
is carried across a cut. On a large model a cut runs in the background: its
progress shows at the bottom of the viewport, with a Cancel button.

A cut through a hollow part leaves a face with a hole in it, and the hole's
rim has to be joined to the outline before the face can be filled. Now and
then no join is found the quick way (the two rims touch or cross, where the
cut ran along the wall, or the hole lies beside a slit and the outline
nearest it is all across the slit). The app then tries every pairing of a
point on one rim with a point on the other. While there are few enough to
list (4 million pairs: two rims of 2,000 points each) they are tried shortest
first; for the rims of a dense model that list ran to tens of gigabytes, so
past that each point of the hole is tried against the whole outline in turn,
which needs no list and still finds a join wherever there is one. Only a
search that has gone on for a second or two with nothing found is given up.
Whenever no join is found, each rim is closed on its own and the line ends "1 cut edge could
not be closed cleanly": the model is closed there, but one cap lies over
another rather than round it, and a Remesh makes one clean solid of it. Keys
fill the ring of cut face round a key the same way; where that cannot be
done the line says "1 could not be built: could not fill the cut face round
this key", and the other keys are built.

Every place the selected object is drawn counts, as in the Unity app: a shape
drawn over one of its **live mirror copies** (see Objects), over a copy of its
**Array**, or over an instance of it, or over a mirror copy of an instance,
acts on the object through that place. A copy is the object's own mesh drawn
again, so a box over the arm on the far side of a mirrored figure masks, hides,
trims or splits the arm of the one mesh, and it goes on both sides at once; a
lasso over the third link of an array's chain bites all of the links the same
way. A shape that covers several places at once (a wide box, or a line, which
has no end) acts through all of them. A tapered array's copies are smaller and
a mirrored array's far side is reflected, and the shape still lands on the
vertices each copy shows. A split leaves the object where it was, with its
mirror and its array, cut; the new "Part" is a plain object in the object's own
place with no mirror or array of its own, since a piece cut off is not a
mirrored object (as in the Unity app; the mirror of the part is a Mirror tick
away). It does start with the object's symmetry settings (the Symmetry card's,
see Symmetry), as in the Unity app: a piece of an object is sculpted the way
the object is. These tools know nothing of arrays: they learn of an array's copies
from the scene, which holds what the Array tool last said of them, so it works
for any copies a tool keeps (see `Draw::live_copies` in
`crates/toolkit/README.md`). The array has to be on screen for it, which it is
as long as it exists, with the Array tool up or put away.

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
| Ctrl + Z, Ctrl + Shift + Z | Undo, redo a joint edit |
| Esc | Let go of a joint, then stop a build, then put the tool away |

Dragging off the cut face orbits. The panel picks the key's shape (circle,
square, rectangle, hexagon, triangle, D, cross, star, dovetail, puzzle tab),
its size and height (or Auto, from the face), how many per cut, which piece
gets the pegs, and under **Fit** the clearance, extra socket depth, chamfers,
draft and minimum wall, all in millimetres. **Build Keys** changes both pieces
in one undo step, in the background on a large model. The planned joints are a
scaffold: not saved, kept when the tool is put away, and handed back when the
build is undone, so Ctrl + Z then J is how built keys are changed. Once a joint has
been placed, moved, turned or removed by hand the app asks before it closes,
or opens a sculpt in place of the scene, over joints that are not built yet;
joints the tool planned by itself and nobody touched are nothing to lose. The pieces must
still be touching when the tool first comes up, since that is how it finds the
pair; after that they can be moved apart and the joints stay on them.

## Files

- **Save / Open** use `.sclp`, which keeps the whole sculpt: every object
  (its exact mesh and mask, hidden faces, quads, name, position and
  visibility, and its own symmetry: mirror axes, radial symmetry, Local or
  World; instances stay linked; live mirrors and live arrays stay on) and how
  it was left: the material and finish, lighting, HDRI, background and
  effects, the camera view, and whether the symmetry guides show. Reopening a
  file puts all of it back. (An HDRI is kept as the path of its picture, not
  the picture. The picture is only looked for on this computer's own drives:
  a file that names one on a network share, or on a drive letter that stands
  for a share, opens with the note "Saved HDRI not found", and Load HDRI
  brings it back. A file from somebody else must not be able to make the
  app sign in to a server of their choosing.) A `.sclp` saved before symmetry belonged to objects holds one
  setting for the whole scene: it opens with that setting given to every
  object in it, in each object's own space, which is how it was applied then.
- **Saving and exporting never harm the file already there.** A save and an
  export (OBJ or STL) are each written in full beside the old file, under the
  same name with `.tmp` added, and only then take its place, so a full disk or
  a crash part-way leaves the old file as it was.
- **Damaged points are repaired, never refused.** Now and then a point of a
  model ends up with no position at all (it is "not a number"). A save or an
  export writes such a point at the average of the points around it, so the
  file always opens and a slicer never meets a NaN, and says so in the status
  line; the model on screen is not changed. A file that already holds one
  (from an older version, or edited by hand) opens with the point moved
  there, the same way, and the status line says how many were repaired. The
  file is not turned away, and the rest of the sculpt is untouched. A Unity
  `.sculpt` with such a point is still refused, and the message says so in
  plain words: the file has a point with no position, and the way on is to
  open it in the Unity app and save it again there if that app still can, or
  else to go back to an earlier save. An OBJ or STL with one just leaves out
  the triangles that touch it.
  A `.sclp` damaged the other way, with a triangle that names a point the
  file does not hold, is not turned away either: that triangle is left out,
  everything else opens, and the status line says how many were left out and
  that the model may have a hole there (Check for Printing finds it, a Remesh
  closes it). Restoring a recovery copy says the same of points and
  triangles as opening the file would.
- A file the app is started with (named on its command line) that is missing
  or cannot be read says so in the status line, with the file's name, and the
  app starts with a new sphere.
- **Open** (and a click on a Recent tile) puts the file in place of the
  scene at once when nothing would be lost: the scene is saved, and no work
  that is in no file stands on it. That is one undo step (undoing an Open
  brings back the scene as it was, live arrays included, lit, coloured and
  framed as it was; see Array), as New Scene is. If the scene has changes
  that are not saved, it asks first, says so, and offers to save first or
  to add the file's objects to the scene instead. (It used to ask "Open or
  add?" every time; to add a sculpt's objects to a saved scene, use Import.)
  So does closing the app (the File card's Exit, the window's close button,
  Alt + F4 or Ctrl + Q); with nothing to lose it just closes.
- **The window's title says whether there is anything to save**: the
  sculpt's name, with a star while it has unsaved changes ("dragon head * -
  GolemSculpt"); "Untitled" for a scene with no file yet; just "GolemSculpt"
  for the untouched scene the app starts with. The File card's header shows
  the same name and star. "Unsaved" means the scene is not what its file
  holds:
  something was sculpted, added, moved, hidden or undone since the last save
  or open, or something was changed that is saved without being an undo
  step: an object's symmetry (the Symmetry card, the X key), what Local View
  shows (/), or a live array (its count, sizes, ticks, path). Those last are
  unsaved only while they differ from the file: leave Local View, or turn
  Mirror X off again, and there is nothing to save. Changing only the look,
  the view or which object is selected does not count.
- **New Scene** (the File card, or Ctrl + N) starts again without restarting
  the app: one fresh sphere at the Starting Detail, selected, with no
  symmetry set, seen from the view the app starts with. It is one undo step,
  like an Open: Undo brings the scene you had back, with its file name and
  whether it was saved, and Redo gives the new scene back. The new scene has
  no file (the File card says "Untitled"), so Save asks where it should go:
  it cannot be saved over the sculpt it replaced. Until something is done to
  it there is nothing in it to save, so closing straight after asks nothing.
  If the scene you have has changes that are not saved, New Scene asks first
  ("Save, Then New Scene", "New Scene Without Saving", Cancel); with nothing
  to lose it does not ask, since the scene is one Undo away. Like Open, it
  also asks when work that is in no file stands on the scene's objects
  (unmerged tubes, planned keys), names it, and makes Enter Cancel: that work
  goes with its objects and Undo does not bring it back. SSpheres, a lathe
  outline and rulers stand by themselves and are still there in the new
  scene, so they are not named (closing still asks about them). A live array
  belongs to its object and comes back with it on Undo.
  **What stays as it is:** the look (material, finish, lighting, backdrop),
  the brush and its settings, the tool that is up, and the settings. The app
  itself starts with the default look, but the one on screen was chosen for
  working in, so New Scene does not take it away.
  **When it waits:** in the middle of a stroke or a gizmo drag it does
  nothing, like every other command. While a background job is working (a
  tube merge, a trim) it is refused with a line saying which job; let it
  finish or press its Cancel. A 360 loop being recorded has the panels away
  and the keys to itself. A timelapse just keeps recording.
  **Recovery copies:** a new scene nothing has been done to is owed no copy.
  Work put aside with "New Scene Without Saving" is kept in memory for Undo,
  and a recovery copy of it is written first if it has none yet, however
  young the work is (the same for "Open Without Saving"). That copy stays in
  the recovery folder until the new scene is saved, three newer copies have
  been written, or the app is closed in the ordinary way; if the app crashes
  before any of those, the next start offers the work that was put aside.
  After a crash the app offers the newest copy, which
  is of the new scene once that has work in it. A new scene started over a
  scene that was restored after a crash and not yet saved leaves that
  crash's copies where they are: the restored scene is one Undo away with
  its note of where it came from, and only saving that scene, or Discard It,
  retires them (see the rule under "A crash does not take the session with
  it").
- **Exit** (the File card) closes the app exactly as the window's close
  button does, which is the way out in fullscreen without the keyboard: it
  asks the same question when something would be lost, and a timelapse still
  recording is finished into a video that plays.
- **Work that is in no file is named before it is lost.** SSpheres, unmerged
  tubes, planned keys and a lathe outline are scaffolds: a `.sclp` does not
  hold them, so saving does not keep them. Closing the app with any of them
  in hand asks first, even when the scene itself is saved, and says what
  would go and what keeps it: "These are not kept in a file and will be lost:
  an SSphere rig of 12 spheres (Make Skin first) and 3 unmerged tubes (Merge
  or Keep Separate first)." If the scene has unsaved changes as well, the one
  question says both, and that saving will not keep the scaffolds. In these
  questions Enter is Cancel, as Esc is, so nothing is given up by a key
  pressed in passing. Open and New Scene ask the same way about what
  replacing the scene would cost: tubes and planned keys stand on the scene's
  objects and go with them, whether or not their tool is up at the time, and
  "Add Its Objects to This Scene" keeps them. (SSpheres, a lathe
  outline and rulers stand by themselves and are still there after an Open,
  so Open does not name them.) Only work is named: not a single sphere, not a
  preset or joints the tool laid out by itself, not spheres already made into
  a skin, and never rulers, which a drag brings back. Import adds to the
  scene, loses nothing and asks nothing.
- **Undo of an Open brings the look and the view back.** A file is opened
  with the material, lighting, backdrop and view it was saved with. Undo of
  the Open gives the scene from before its own back, and Redo the file's, as
  often as the step is crossed. (It used to bring back the objects only, so
  the old scene stood under the opened file's lighting and was framed as that
  file was.) A new scene and a restored recovery copy are the same kind of
  step: Undo of either puts the view back where it was.
- **Undo of an Open brings the file name back too.** The scene from before
  returns with the file it had (the File card shows its name again, or
  "Untitled" if it never had one), so Save writes it where it always went, or
  asks where; it is never written over the file that was just opened. Whether
  it had unsaved changes comes back with it: a scene that was saved before
  the Open has nothing to save after the Undo, and one opened over without
  saving still has its changes to save. Redo goes the other way: the opened
  file's name, and nothing unsaved. The same holds after Save As on the
  opened scene: the new name belongs to that scene, and Undo past the Open
  still gives the earlier scene its own file. One case counts as unsaved on
  purpose: if the opened scene was saved over the earlier scene's own file,
  that file no longer holds the earlier scene, so when Undo brings it back
  the app treats it as unsaved work and asks before closing. Adding a file's
  objects (Import, or "Add Its Objects to This Scene") never changes the file
  name. Undo itself errs on the side of asking: undoing an edit made after a
  save still counts as unsaved, though the scene is then what the file holds.
- **Recent**: the File card shows the last six sculpts saved or opened, each
  with a picture of the model taken when it was saved. Click one to open it.
  A file that has been moved or deleted drops out of the tiles.
- **Drag a file onto the window** from Explorer: a sculpt (`.sclp`, or a
  `.sculpt` from the Unity app) opens as Open opens one, asking first only
  if unsaved work would be put aside; a model (`.obj`, `.stl`, `.ply`,
  `.fbx`, `.glb`, `.gltf`) is added to
  the scene as Import adds it; a picture (`.png`, `.jpg`) becomes a
  reference picture, as Add Picture... makes one (see Reference images).
  While the file is over the window a toast says which ("Drop to Open",
  "Drop to Import", "Drop to Add as a Reference Picture"). Anything else is
  turned away with a line saying what can be dropped. Opening and importing
  are each one undo step.
- **Import** adds a model (`.obj`, `.stl`, `.ply`, `.fbx`, `.glb` or `.gltf`) to the scene, centred, or the
  objects of another `.sclp` where they were saved, with their live arrays. An STL comes
  in at its real size (millimetres, Z up; one unit is 10 mm, so a 10 mm ball is
  one unit across), welded into one connected surface, with inside-out
  triangles turned the right way. An OBJ is fitted to the size of the first
  object, as in the Unity app.
  - **A file with several objects** (an OBJ's `o` lines) comes in as that
    many objects, named as the file names them, standing where they stood
    in the program that wrote them: they share one scale and one position,
    so eyes stay in their sockets and each object keeps the file's own
    symmetry plane, as ZBrush subtools do. The first object in the file is
    the one selected. All of it is one undo step.
  - **Quads stay quads.** A model made of quads (from ZBrush, Blender or a
    retopology) keeps them, so **Subdivide** uses Catmull-Clark as those
    programs do, and an export writes the quads back out. A triangle in
    among the quads is kept as a face of three.
  - **FBX** (binary, as ZBrush, Blender, Maya and 3D-Coat write it) brings
    each model in as its own object, at its real size, upright whichever way
    the file says is up, with quads kept and vertex colours as a paint layer.
    A text FBX is turned away with a line saying to export it as binary.
  - **glTF** (`.glb`, or `.gltf` with its `.bin` beside it) brings each
    object in at its real size (glTF counts in metres) with its vertex
    colours. A compressed glTF (Draco or Meshopt) is turned away with a line
    saying to export it without compression.
  - **PLY** (text or binary, from scanners, MeshLab and our own export) is
    fitted to the scene as an OBJ is, with its vertex colours. A PLY of
    points with no faces says it has no surface to sculpt.
  - A file that states its size but states it wrongly (smaller than half a
    millimetre or bigger than fifty metres across) is fitted to the scene
    instead. **UVs** come in with the model (an OBJ's `vt`, an FBX's first
    UV set, a glTF's `TEXCOORD_0`), and OBJ and FBX exports write them back
    out (see UVs and maps). Materials, normals and animation are not read.
- **Sculpts from the Unity app** (`.sculpt`) open with **Open** and
  **Import** just as a `.sclp` does, and show in Recent. They arrive the same
  way round and the same size: every object with its mesh, mask, hidden
  faces, name, place and visibility, and with its own symmetry (mirror axes,
  radial symmetry, Local or World), as the Unity app kept it; instances and
  live mirrors; the material and finish, lighting, HDRI, background and
  effects; the camera view; and the object that was selected. Whether the
  symmetry guides show is taken from that object. Dynamic topology is set as
  the file left it. A live array comes in live: the object has its array,
  handles and all, and the copies are where the Unity app drew them (a circle
  round Y or Z is set to go the other way round, which is how it is the same
  circle in this app's flipped axes). The status line says what a file could
  not bring: the Unity app's brush settings, an array of a kind this app does
  not know.
  **The Unity file is never written to.** Save on such a scene asks where to
  put a `.sclp`, offering the same name, and that `.sclp` is the document
  from then on. A file that is damaged, or from a newer Unity format than
  this app knows, is refused with a message and the scene stays as it was.
  How the Unity app's axes map onto this app's is set out in
  `src/unity_import.rs`.
- **Export...** in the File card opens a menu of the formats: OBJ, FBX,
  PLY and STL for Printing, and the FBX material weights setting.
- **Export OBJ / Export STL for Printing** write the selected object where it
  stands in the scene, with its position, rotation and scale baked in, so an
  instance exports as its own placed copy. The STL is binary, in millimetres,
  turned upright for a slicer (the model's up is the bed's Z, its front faces
  the front of the bed, nothing mirrored). The OBJ keeps the app's own units
  and axes, and a quad-remeshed object goes out as its quads. The live mirror
  copies of an object, and the copies of its live array, are part of the model
  on screen and go out with it, each the right way out: the STL holds them
  all in one file, the OBJ writes each as an object of its own ("Cube_MirrorX",
  "Cube_Array3", and "Cube_Array3MirrorX" for that copy's reflection, as the
  Unity app named them). An array of a very heavy object can ask for more
  triangles than an STL can hold (4 billion), and is refused in words. The save box
  offers `<object name>_<date>_<time>` as the file name. The size conversion
  lives in one place, `crates/geom/src/units.rs`. **Export STL for Printing**
  looks the object over first, and asks before writing one with holes in it
  (see Checking a model for printing).
- **Export PLY** writes the selected object (and its live copies) as a binary
  PLY, in the app's own units and axes like the OBJ: the format most programs
  read painted colour from (MeshLab, CloudCompare, colour 3D printing).
- **What each format carries of a painted model.** The colour that goes out
  is the colour you see: the paint layers over the Base Colour of the Look
  panel, so where nothing is painted the model goes out in the Base Colour,
  and where paint is thin it is mixed with it. (A finish preset's own colours,
  Clay or Metal, are not in it: no file format can carry a finish.)
  - **OBJ**: each point's colour on its `v` line (`v x y z r g b`, 0 to 1),
    which Blender, MeshLab, ZBrush and most others read. No materials: OBJ
    keeps those in a separate `.mtl` file per face, which a painted blend of
    materials does not fit. **Import** reads those colours back as a paint
    layer, "Imported Colours".
  - **PLY**: each point's colour (red, green, blue, alpha always full), and
    for each material the object is painted with, how much of it is at each
    point, 0 to 1, under the material's name (`mat_Metal___Rust`; the file's
    header gives each one's full name).
  - **FBX**: the colour as one colour set ("GolemSculpt Paint"), and each face
    with the material that has most of it (the surface nothing was painted on
    is "GolemSculpt Base"), each material named as in the palette and coloured
    with its swatch. **Unity** reads the one colour set into the mesh's
    vertex colours (a shader has to use them: Unity's standard ones do not)
    and makes a sub-mesh, with a material slot, for each material, so a
    material of Unity's own can be put on each painted region. **Blender**
    reads the colour set as a colour attribute and the materials as material
    slots. With **FBX: material weights in UVs** ticked on the File tab
    (off unless ticked, and not remembered between runs), the FBX also holds
    how much of each material is at each point, for a game shader of one's
    own that blends textures: UV set 0 is the model's own UVs, or all zero
    for a model that has none;
    UV sets 1 to 4 hold the weights two materials a set (U the first, V the
    second, in the order the set's name says: "Weights Metal - Rust + Clay").
    In Unity those are `uv2` to `uv5`.
  - **STL** is shape alone: it has no room for colour, and the line after an
    export says so when the object is painted.
  - **Send to Blender** carries the most: see Blender.
- **The app remembers how you left it**: interface size, the theme, tooltips
  on or off, which cards are open, which float over the view and where, which
  panels are folded away and which tab each is on, fullscreen, millimetres or inches, the remesh settings,
  Decimate's target, dynamic topology, every brush's tuning, the brush in hand,
  the brushes on the wheel and the one Shift uses. They are kept in
  `%APPDATA%\Sculpt\settings.json`, with the recent files; deleting that file
  puts everything back to how the app first starts. Brushes of your own are
  files in `%APPDATA%\Sculpt\brushes` (see Brush library) and are not touched
  by that.
- **A crash does not take the session with it.** While the scene has changes
  that are not saved, the app writes a recovery copy of it every three
  minutes, in a pause between strokes (about a tenth of a second for four to
  eight million triangles on a fast disk; a toast says "Recovery copy kept"
  when it was long enough to feel). It does not wait for the three minutes
  when unsaved work is about to be put aside by "Open Without Saving" or
  "New Scene Without Saving": the copy that is owed is written first. The copies are ordinary `.sclp` files in
  `%APPDATA%\Sculpt\recovery`, in a folder per run of the app; each is a new
  file, never written over another or over a file of yours, and the newest
  three are kept (each is the size of a save: about 200 MB for a scene of
  twelve million triangles). If the app stops on a fault of its own it says so in a
  message, and first writes one more copy of the scene as it stood, beside
  the others. If the graphics card reports a fault, the same.
  **The next time the app starts it offers the copy:** Restore It puts it in
  place of the scene (one undo step), not yet saved and with no file, so Save
  asks where it should go, starting from the file it was a copy of. (Undo of
  the restore gives the scene from before its own file back, as Undo of an
  Open does.) A copy
  made as the app stopped may have caught the last change half done, so the
  question says so and offers the last regular copy as well. Discard It
  leaves the scene alone; Not Now (Esc) keeps the copy and asks again next
  time. If the app failed to close properly more than once (two windows open
  when the power went, or a second crash before the first was dealt with),
  each time's copies are kept apart and asked about by themselves, the latest
  first: the question says so, and an answer touches that one time's copies
  and no others. The rest are asked about at a later start, once this one has
  been saved or discarded. Which copy is the newest goes by the names of the
  folder and the file, not by the dates of the files, so a backup or a copy
  of the folder does not change what Restore It restores.
  **A clean exit leaves nothing to ask about**, and nothing else removes a
  recovery copy. A copy is removed only when one of these holds:
  (a) the scene it holds has been saved to a file of yours by this run: the
  run's own copies when its scene is saved, and a crashed run's copies when
  the scene that was restored from them is saved;
  (b) you answered Discard It about that crashed run's copies, and this run
  has since saved or been closed in the ordinary way;
  (c) the run that made it is closed in the ordinary way, with nothing
  unsaved or with the question about closing answered (Save and Exit, or
  Exit Without Saving).
  (A run also drops its own regular copies beyond the newest three.) So
  Restore It removes nothing by itself: the copies stay until the restored
  scene is saved. Restore It followed by Exit Without Saving, or by Undo (Ctrl + Z)
  and closing, leaves them where they are, and the next start asks again;
  the earlier copy beside one made on a fault is still there to be had if
  the newest turns out bad. Undo of the restore puts the scene from before
  back, and saving that scene, or a file opened or a new scene started over
  the restored one, settles nothing about the copies either. Not Now removes
  nothing. Whenever
  it cannot be told, the copy stays and the app asks again; the cost is being
  asked once more, and Discard It ends it. Until they are removed the copies
  can be opened with Open like any sculpt. A copy is only ever offered if
  the run that wrote it is no longer running, so a second copy of the app
  open at the same time does not offer the first one's work; a folder the
  app cannot tell about is left alone, neither offered nor removed. If
  restoring a copy itself runs into a fault part of the way, the app says so,
  the scene on screen has no file (so Save asks where, and writes over
  nothing) and the copy is kept for the next start. What is lost in
  the worst case (the power going, the program being killed) is the work
  since the last copy, three minutes at most. A copy holds what a save
  holds: a change that is saved without being an undo step (a symmetry, the
  hide keys, an array) is owed a copy like any other and comes back with it.
  SSpheres, unmerged tubes, rulers, planned keys and a lathe outline are not
  in a copy, as they are not in a save: a crash loses them, and the restored
  scene comes back without them.
- **What went wrong is written down.** `%APPDATA%\Sculpt\log.txt` gathers the
  app's warnings and errors and, for a fault, what it was and where in the
  program; it starts again when it reaches a megabyte.

A `.sclp` is the meshes as raw arrays (so a heavy scene saves and loads as
fast as the disk allows) followed by one JSON section for everything else.
Files from earlier versions of the app still open. The layout, and how to
keep something new in it without changing the format, are set out in
`src/io.rs`. What a tool keeps for an object (an array's settings and points)
is one more field of that object's entry in the JSON, under the tool's name
(`"array"`), which a version that does not know it steps over.

## Blender

**Send to Blender** (the File tab) sends every selected object to Blender as
it stands, at its real size, and opens Blender if it is not open. There is no
file to save, find or import. In Blender, **Send to GolemSculpt** brings
objects back: it is in the GolemSculpt tab of the sidebar (N), and at the
bottom of the Object menu. Either button is the whole trip; the other program
takes what was sent within a second, and is opened first if it is closed.

- **Size is kept exactly, both ways.** 1 unit here is 1 cm. In Blender a model
  is that real size in whatever units the Blender file has. A new Blender file
  that has not been worked in is set to millimetres as the first object
  arrives (1 Blender unit = 1 mm, so a 5 cm sculpt is 50 units, sizes read in
  mm, and an STL exported from Blender is the size a slicer expects). A file
  that is saved, has been worked in, or has had its units chosen is left
  exactly as it is. Up is up in both: Y here is Z there.
- **An object that came from here goes back as itself.** Its shape is
  replaced, in one undo step, where it stands in Blender, whatever was done
  to it there: moved, turned, resized, modifiers and all. The object here
  keeps its centre, turn and size, its symmetry, its live mirror and its
  instances, so a symmetric sculpt is still sculpted about the same planes.
  If the model was moved a long way in Blender, Set Origin to Mesh brings its
  centre after it.
- **Anything made in Blender arrives as a new object**, selected and framed,
  under the name it has there, and goes back as itself from then on.
- **Only what was worked on is replaced.** Select everything and send it back
  and forth: an object neither program has changed since the last trip is
  left alone on both sides, and the status line says how many were. So a
  mask, hidden faces or the quads of a quad remesh are lost only on an object
  Blender really changed (what comes from Blender is a new mesh). Quads go to
  Blender as quads.
- **Live mirror and array copies go too**, as objects of their own
  (`Ball_MirrorX`), since they are part of the model on screen. Coming back
  unchanged they are passed over; one that was worked on in Blender arrives
  as a new object.
- **Modifiers in Blender.** What is sent here is the object as it looks, with
  its modifiers worked in. If it is then sculpted here and sent to Blender
  again, the modifiers that were worked in are switched off there (not
  deleted), because the sculpt already has them in it, and Blender says so.
  An object that was not touched here keeps its modifiers live in Blender.
- The first time, Blender is looked for where it is installed (Blender's own
  installer, Steam); if it is not found, a box asks where `blender.exe` is,
  once. Blender's side of the link is one script that GolemSculpt starts
  Blender with; it then keeps itself in Blender's own startup folder
  (`scripts\startup\golemsculpt_link.py` in Blender's settings), so the link
  is there however Blender is opened. Deleting that file removes it. A
  Blender that was already open the very first time does not have the link
  yet: a second one is started, and the status line says to close the other.
- **Paint goes too.** A painted object arrives in Blender with colour
  attributes on its points: "GolemSculpt Paint", the colour you see here (the
  layers over the Base Colour), which is the one Blender shows (Solid shading,
  Color: Attribute) and renders; then one for each paint layer that has
  colour, under the layer's name, holding that layer alone (its coverage as
  alpha); and for each material it is painted with, a number on each point
  under the material's name, how much of it is there (a shader's Attribute
  node of that name reads it). Paint in Blender on "GolemSculpt Paint"
  (Vertex Paint mode) and send the object back: its paint layers here are
  kept as they were, carried onto the shape Blender sent, and what was
  painted in Blender comes in as a new top layer, "From Blender", covering
  exactly the points that were painted there; nothing painted, no new layer.
  Changes to the per-layer attributes in Blender do not come back. An object
  made in Blender with colour on it arrives with that colour as its paint
  layer "From Blender". Repainting here, or there, counts as a change, so
  the object goes across again. In Blender, an object that is updated keeps
  the materials it had there.
- A Blender that was opened with the link from before paint (one already
  open when GolemSculpt was updated) takes the shapes and passes the paint
  over; the status line says so. Close Blender and press Send to Blender:
  GolemSculpt opens it with the new link, which it keeps from then on.

The two programs meet in a folder, `blender_link` in the settings folder
(`%APPDATA%\Sculpt`): each leaves a file in the other's tray there and looks
in its own twice a second. Nothing listens on a port and nothing leaves the
computer. What was sent and not taken within ten minutes (the other program
was never opened) is cleared away. The format is set out in `src/io/link.rs`.

## Checking a model for printing

A printer needs a closed surface: every edge shared by exactly two faces. The
app counts the edges that are not, and says so in the status line at the top
of the Scene panel, in red:

> "Sphere" has 12 open edges (2 holes) and 3 edges shared by more than two
> faces: it may not print. A Remesh (R) usually closes it.

An open edge has a face on one side only: it is on the rim of a hole. An edge
shared by more than two faces is where sheets meet along a line, so there is
no telling which side is inside. Hidden parts are counted like the rest: the
file a slicer gets is the whole model.

- **Check for Printing**, under Export STL for Printing in the File card,
  looks over the selected object and answers in that line: what is wrong and
  what closes it, or that it is a closed surface.
- **Export STL for Printing** makes the same check first. A closed object goes
  straight to the save box. For one that is not, a question says what is
  wrong and offers **Export Anyway** (Enter) or **Cancel** (Esc); exported
  anyway, the status line says once more that it may not print.
- **After anything that rebuilds a mesh** the result is checked, and the
  warning follows the operation's own line: Remesh, Quad Remesh, Cut &
  Mirror, Decimate (which also says how it was before), Join (with
  or without the remesh), Subtract, Union and Intersect, Trim, Split,
  Extract, a tube merge or tubes kept as objects, built keys, an array made
  into one object or into separate ones, a lathe, an SSphere skin, and an
  imported `.obj` or `.stl`. With dynamic topology on, a stroke that added
  triangles is checked too, and is only spoken of if it left more holes than
  it found.
- **Remesh and the booleans close holes on their own**, and now say when they
  did: "The model had holes, which were closed over flat" (for a boolean,
  "One of the objects had holes..."). Where the surface was torn, doubled or
  inside out, so that no flat cap fits, they say that what is solid there is
  a best guess and to look that area over.
- **Quad Remesh gives a closed model back closed, and keeps the openings an
  open one has.** It does not close a model (Remesh does), so a sleeve or a
  mask comes back with each opening still an opening, and its line counts
  them ("68 open edges (1 hole)"). A closed model used to come back, about
  one time in four, with a five-sided hole or with two quads cut along the
  same line; over the six closed test models at seven sizes each, all 42
  results are closed now, and so were 99 more at other sizes and on three
  further models. If one ever is not, its line still says so and what to do:
  another number of quads, or undo. A model with openings used to lose some
  of them at low quad counts (a ball with its top cut off came back a whole
  ball at 100 and 300 quads); those are kept now.

Only holes and crowded edges are looked for. Surfaces that pass through each
other (a plain Join of overlapping parts), walls too thin for the printer and
faces turned inside out are not, so a model that passes is closed, which is
not the same as certain to print. Nothing here repairs a model: a Remesh
does, and the warning says so. The count is quick, 5 to 8 ms for each million
triangles (`crates/geom/src/edge_census.rs`); the wording is in
`src/printable.rs`.

## Polypaint

Colour, and the materials of the Materials card, painted straight onto the
model's surface, in layers, as in ZBrush's Polypaint or a painting program.
Paint lives on the vertices like the mask, so a dense model holds as much
paint detail as it holds form, and the paint goes wherever the clay goes:
undo, a save, a remesh, a change of subdivision level, a join, a copy, Send
to Blender.

**The Paint brush** (P, or its tile in the Brushes card) paints on the paint
layer that is lit on the Paint Layers card. On an object with no paint yet,
the first stroke makes a layer, "Paint 1", and the line says so. It is a
brush like the others: the ring, Focal Shift, the falloff curve, alphas (a
stencil to paint through), Spacing, Scatter, Size Jitter, the pen's pressure,
Lazy Mouse, Front Facing, Stay in Group, the mirrors and radial symmetry all
work as they do for sculpting; masked surface takes no paint and hidden
faces none. A stroke is one undo step. Painting never moves the surface or
adds triangles, even with dynamic topology on.

Its settings (Brush tab, the Brush Settings card):

- **Flow** (the Strength slider, F + drag) is how much each dab lays, and
  **Opacity** the most a stroke lays: going over a place again in the same
  stroke builds up to the Opacity and never past it, so a 40% stroke
  scrubbed back and forth is 40% everywhere. The pen's pressure lightens the
  Flow as it lightens any brush's Strength.
- **Paint**, **Erase**, **Blur**, **Smear**, **Sample**: Paint lays the colour;
  Erase takes this layer's paint away (colour, material, roughness and
  metallic alike); Blur softens it as Smooth softens clay; Smear picks up the
  paint it passes over and drags it along, like a finger in wet paint; Sample
  is the eyedropper: a click picks up the colour and material under it.
  **Ctrl** swaps Paint and Erase for one stroke, and **Shift** blurs the paint
  (with the Paint brush in hand Shift never smooths the clay). **I** picks up
  the paint under the pointer at any time, whatever brush is in hand.
- **Colour**: the colour well opens a picker; the row of swatches under it is
  the colours painted with lately (click one to take it up again).
- **Paints**: which of the layer's channels the brush writes: Colour,
  Material, Roughness and Metallic (each of those two with the value it lays).
  The material is the one picked on the Materials card (see Materials), shown
  with its swatch; **None** paints colour only and leaves the materials under
  it alone, and **Materials...** opens the card.
- **Zadd (Colour + Sculpt)**: each dab also sculpts, with the brush chosen
  under it (Standard, Clay, Layer, Inflate or Crease), at that brush's own
  Strength and with the Paint brush's ring, falloff and alpha, so the paint
  stands up off the surface. Ctrl paints out and digs in. The stroke is one
  undo step for the paint and the shape together. The sculpting brush is a
  choice of its own rather than the brush you had before: that would change
  under your hand each time another brush was picked.
- **Paint Layer Mask** (for a layer whose mask is Paintable): the brush paints
  the layer's own mask instead of its paint: Paint hides the layer under the
  brush, Ctrl brings it back.

A shipped brush, **Airbrush**, paints soft, light mists that build up as you
go over a place, for shading and blushes.

**The Paint Layers card** (Mesh tab, under Layers) lists the selected
object's paint layers, the top one first; a higher layer paints over the
ones under it. A row has a grip to drag it up or down the stack, the dot that
marks the layer the brush paints on (click another row to paint on it), the
name (double-click to rename), the eye, the padlock (a locked layer takes no
stroke, fill or clear), the bin, and its **Opacity** (one drag is one undo
step). **New Layer**, **Duplicate**, **Merge Down** (the layer folded into the
one under it, looking the same) and **Flatten All** (every layer that shows
folded into one; hidden ones are dropped) are above the list. Under it are the
settings of the layer being painted on:

- **Blend**: how its colour lands on what is under it, by the names painting
  programs use: Normal, Multiply, Screen, Overlay, Darken, Lighten, Hard
  Light, Soft Light, Add, Subtract, Hue, Saturation and Colour.
- **Material**: **Normal** lays its material over what is under it, by how
  strongly it is painted; **Add** adds it, and the others give way only where
  there is no room.
- **Colour, Material, Rough, Metal**: which of its channels it shows.
- **Shows**: where on the surface it shows: **Everywhere**; **Paintable** (its
  own mask, painted with Paint Layer Mask); **Sculpt Mask** or **Not Masked**
  (the sculpt mask, as it is now, so remasking moves the layer); **Cavity**
  (the recesses of the surface, as they were when it was picked; **Recompute
  Cavity** takes them again); or a **Face Group** (the one picked on the model
  with Ctrl + Shift + click).
- **Fill All**, **Fill Masked**, **Fill Unmasked**, **Fill Picked Group**: the
  layer painted at once with the brush's colour and the material picked
  (**Fill with** says which), over the whole layer, the masked surface, the
  rest, or a face group. **Clear Layer** empties it. Each is one undo step.

**Masking by paint** (the Masking card, Mesh tab, on an object with paint):
**By Colour** masks where the paint is the Paint brush's colour, within
**Colour Tolerance**; **By Material** where the material picked is painted;
**By Layer** where the layer being painted on has paint. Each is an ordinary
mask step, so Invert, Blur and the rest work on it.

What leaves the app with the model: the paint as it shows goes into OBJ, PLY
and FBX files, its materials into PLY and FBX, and every layer into Blender
(see What each format carries of a painted model, under Files, and Blender).

## Look

How the model is shaded is in three cards on the Scene panel's Look tab. The shaders and
numbers are the Unity app's, so the same settings give the same picture.

- **Material**: a matcap, or (Matcap off) a lit surface. Lit surfaces are a
  plain base colour with Metallic and Smoothness, or a **Surface Finish**:
  Lure Plastic (translucent, with glitter), Metal (rust, patina, antique
  silver and gold, worn steel, pewter), Clay or Wood. Pick a preset, then
  adjust it with the sliders. **Glow** adds light the surface gives off (lava
  cracks, neon, veins and so on) over any of them, matcaps included.
  **Import Matcap...** adds a matcap picture of your own (.png or .jpg; it is
  copied into `assets/matcaps/Imported`), and **Rescan Folder** finds pictures
  you have put in `assets/matcaps` by hand. A picture longer on a side than
  the graphics card takes as a texture (8192 pixels, as the app sets the card
  up) is halved until it fits, the way an HDRI is, and the line says so:
  "It is 9000 x 9000, larger than this graphics card takes, so it is shown
  at 4500 x 4500." The file in the folder is left as it was. (Such a picture
  used to stop the app.)
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

**Show Backfaces** (at the bottom of the viewport, beside Flat Shading and
Wireframe) draws the inside of the surface as well as the outside. It is off
to start with: the side of the clay facing away from you is not drawn, which
on a closed model changes nothing you can see and leaves the graphics card
half the work. Where the model is open (a hole, a cut edge, faces you have
hidden) you then look straight through it, past the inside, to whatever is
behind. Switch it on to see the inside, tinted dark blue-grey so holes and
faces turned the wrong way stand out, as it always was before. It is saved
with the other shading settings.

### Materials

The **Materials** card (Look tab, under Material) is the library of materials
paint can put on the model, beside its colour. Each row has a little picture
of the material on a grooved ball.

- **Built in**: Plain, and one material for every preset of the Look's
  finishes: the 6 lure plastics, 7 metals, 5 clays, 6 woods and 8 glows. A
  built-in material is exactly that preset as the Look shows it before any
  slider is moved. To change one, **Duplicate** it: the copy is yours to
  rename, change and delete.
- **Yours**: duplicates, **Save the Look** (the Look's surface finish as it is
  now, sliders and all), and **Import...** (a material file a friend sent, or
  one you exported or deleted). Your materials are files in a `materials`
  folder beside the settings file; Delete moves the file into
  `materials\deleted`, where Import finds it again. A material of your own
  shows its finish's sliders in the card, and **Painted Colour** says what
  colour painted over it does: **Off** (the material keeps its own colours),
  **Multiply** (it is stained by the paint, like a tinted clay) or
  **Replace** (the paint takes the place of its colour; its gloss, bumps and
  sheen stay), with an Amount. Built in, plain and glow materials take the
  paint outright (Replace), clay as a stain (Multiply), and plastic, metal and
  wood keep their own colours (Off).
- **Custom Shaders**: materials you write yourself (see below).

Picking a material makes it the one painting lays. It joins **This Scene's
Palette**: the materials this scene's paint uses, kept whole inside the
`.sclp` file, so a scene looks the same on a computer without your library.
A material's pattern (grain, flakes, patches) is sized to the model when it
joins the palette, as picking a Look preset does; **Re-measure** sizes it
to the model as it is now. **Replace With...** puts another material in an
entry's place, so everything painted with it changes at once. **Keep** saves
an entry into your library, **Colour Only** paints no material, and **Remove
Unused** takes out the materials nothing is painted with. Changing one of
your materials changes what was painted with it.

One object's paint can use up to eight materials. Where materials meet, they
blend smoothly: each point of the surface is a mixture of the materials at
the corners of its triangle, never a material that is at none of them.

**Show Polypaint** and **Show Materials** (top of the card) switch the painted
colour and the painted materials off and on, to see the clay underneath.
Under a matcap, painted colour tints the matcap, and each painted material
tints it with its swatch colour.

#### Custom materials

A custom material is a finish written in WGSL, the graphics card's shading
language: two files with one name in `materials\shaders` beside the
settings file (the card says where): `copper.wgsl` and `copper.json`. The
first time the app runs it puts an example there, **Hammered Copper**
(`assets/materials/example`), with every line explained. The app looks at
the files about once a second: save a change and the model shows it.

The `.wgsl` file defines one function named for the file:

```wgsl
fn finish_copper(s: Surface, params: FinishParams) -> MaterialSample {
    var m = material(s, params.v[0].rgb);   // a sample of just this colour
    m.metallic = 1.0;
    m.smoothness = params.v[1].x;
    return m;
}
```

- **`Surface`** is the pixel: `world` (its place), `v` (toward the eye), `p`
  (its place in the object's own space at world size: put patterns here so
  they stay on the model), `footprint` (one pixel's size on the model),
  `p_dx` / `p_dy` (how much `p` changes to the next pixel right and down),
  `n` (the normal to shade with), `smooth_n`, `curv` (above 0.5 a recess,
  below a ridge), `shadow`, `ao`, `base_color`, `back_depth`, and the
  object's axes `mx`, `my`, `mz` and `scale`.
- **`FinishParams`** is sixteen `vec4`s, `params.v[0]` to `params.v[15]`,
  filled from the sliders as the sidecar says.
- **`MaterialSample`** is what the surface is made of there: `albedo`,
  `metallic`, `smoothness`, `occlusion`, `normal`, `emission` (light it
  gives off), `sheen_amount` / `sheen_power` (a second, sharper highlight),
  `scatter_color` / `scatter_wrap` / `scatter_shadow` (light bleeding past
  the shadow line) and `dim`. The app lights it.
- Everything in the app's own shader is there to use: `material(s, albedo)`,
  `value_noise`, `fbm`, `hash33`, `glow_cells`, `perturb_normal`,
  `surface_convexity`, `to_world_dir`, `surface_width` and the rest. Name
  your own helpers starting with your material's name, so two materials'
  helpers cannot clash.
- **No screen-space derivatives**: no `dpdx`, `dpdy`, `fwidth`, nor
  `textureSample` without a level. A painted material is shaded inside a
  branch chosen pixel by pixel, where those give garbage. Use `s.footprint`
  to fade detail smaller than a pixel, or `surface_width(s, gradient)` for
  what `fwidth` would have said. The app warns if a file uses them.

The `.json` sidecar names the material and lists its sliders:

```json
{ "name": "Copper", "swatch": [0.8, 0.5, 0.3], "needs": ["curvature"],
  "params": [
    { "name": "Colour", "kind": "colour", "default": [0.86, 0.52, 0.34], "at": [0, 0] },
    { "name": "Gloss", "kind": "number", "min": 0, "max": 1, "default": 0.7, "at": [1, 0] },
    { "name": "Dent Size", "kind": "number", "min": 0.005, "max": 0.2, "default": 0.03,
      "at": [1, 1], "scale": "size" },
    { "name": "Polished", "kind": "toggle", "default": true, "at": [1, 2] } ] }
```

`at` is which `vec4` and which lane (0 to 3) a slider's value goes in. A
colour fills three lanes, in linear colour; a toggle is 1 or 0; a number with
`"scale": "size"` is a fraction of the model's size, as the built-in
finishes' pattern sizes are. `needs` asks for what costs an extra pass:
`"curvature"` (for `s.curv`) and `"back_depth"` (for `s.back_depth`). The
swatch is the colour the material's row and the matcap show.

Before the graphics card sees a custom material it is checked the way the
card would check it. One with a mistake shows as its plain swatch colour, a
red dot marks its row, and the status line says which line of the file is
wrong ("Custom material "Copper" did not compile (line 12: expected `;`)").
The app never closes over one. **Reload Custom Materials** reads the folder
again.

## Reference images

Photos or drawings of your subject can stand in the scene as flat pictures to
sculpt against, as in ZBrush, Blender and Nomad: match the model's outline to
the front photo in the front view and to the side photo in the side view. They
are in the **Reference Images** card at the bottom of the Scene panel's
Objects tab (closed until you open it).

- **Add Picture...** takes a .png or a .jpg (or drop one onto the window
  from Explorer). The new picture faces the view
  you are in: turn to Front, Right, Top and so on with the view gizmo first
  (else it faces the front). It comes in as tall as the model, its middle on
  the model's, and just behind it, so you see the model against it. A
  phone's photo is turned the way the phone was held, and one larger than
  4096 pixels on its longest side is shown at that size (the file you chose
  is kept as it was).
- The list shows each picture small, with its eye to show or hide it. Tap a
  picture to see its settings:
  - **Facing**: Front, Back, Left, Right, Top or Bottom, the view that sees it
    straight on and the right way round. **Fit to Model** sizes and places it
    again for the model as it is now, from the view it faces.
  - **Size** (how tall it stands; the width follows from the image), **Left
    / Right** and **Up / Down** (slide it in its own plane), and **Depth**
    (how far behind the middle of the scene it stands, seen from its view;
    below nothing brings it in front). Lengths are in millimetres.
  - **Opacity**, **Flip** (mirror it left to right, so one photo of a side
    serves for both sides), **In Front** (drawn over the model, see-through,
    for tracing an outline; ticking it on a solid picture brings it to 50%)
    and **Show**.
  - **Delete** asks first: the picture's image is kept in the scene and
    nowhere else, and Undo does not bring it back.
- A picture shows only from the side it faces, so the front photo never
  stands between you and the model when you orbit round to the back. Behind
  the model it is hidden wherever the model is in front of it.
- Pictures are a view aid, not objects. Brushes go through them, and Frame,
  Remesh, Join, the booleans, exports, Send to Blender, Render Picture, the
  turntable loop, timelapses and the recent files' tiles leave them out.
  Their colours are the photo's own, whatever the lighting and the Look.
- They are saved in the `.sclp`, image and all (the photo file itself, a
  second picture of the same photo costing nothing more), so the scene opens
  with them on any computer; recovery copies and earlier versions carry them
  too. Opening a sculpt replaces the scene's pictures with its own, and Undo
  of the Open brings the old ones back; Import adds a sculpt's pictures to
  the scene's. Adding, changing and deleting pictures are not undo steps,
  like a symmetry setting, but Save and Exit know they are unsaved changes.
  An older version of the app opens such a file without its pictures.

## Objects

The Scene tab has four cards: **Add Object**, **Objects** (the scene graph's
list), **Selected Object** (its name, Clone and Instance, position and scale,
and its Live Mirror) and **Combine** (the booleans, Join and exporting the
selection, shown once two or more objects are selected). A button that would
do nothing is left out rather than greyed: Reset Shape appears once the
object has been sculpted, Make Unique on an instance, Reset Rotation and
Scale once it is turned or resized, Set Origin to Mesh once its centre is
off its mesh. Live Mirror is three buttons, **Mirror X**, **Mirror Y** and
**Mirror Z**, until the object is mirrored; then ticks for the planes it is
mirrored across, with Validate and Remove.

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
- **Each rebuild says beforehand what it will throw away.** Under the
  Remesh, Quad Remesh, Decimate, Join and boolean buttons a line names what
  the object (for Join and the booleans, any selected object) has that the
  rebuild would lose, when it has any: "Drops the subdivision levels, merges
  the sculpt layers into the model, clears the mask and shows the hidden
  parts again. Undo (Ctrl + Z) brings it all back." Each names only what it loses:
  Decimate keeps the mask and what is hidden, a plain Join keeps each mask.
  Under Quad Remesh another line says it ignores symmetry, while the object
  has a mirror axis or radial symmetry on.
- **How many triangles a resolution makes.** Under the Remesh card's
  Resolution, Join's Remesh Resolution and the booleans' Voxel Resolution, a
  line says about how many triangles a rebuild at that number would make
  ("About 1.20M triangles"), by the same reckoning the kernel uses to keep
  within its limit, from the surface's area; for Join and the booleans it is
  the most the result could have ("Up to about"). Past the limit it says so,
  and the highest resolution that would do. The area is worked out again
  after each edit, not every frame.
- **How big a rebuild may be.** Remesh, Join with remesh, the booleans and a
  tube merge answer to one set of limits (`crates/volume/src/budget.rs`): 80
  million triangles (the booleans used to stop at 40 million; they now go
  as far as Remesh always has), and 4 GB of working memory for the grid and
  the surface read onto it, reckoned as the most the job holds at any one
  moment (the first count was of what it kept, and a job could hold two or
  three times that while it ran). What would go over is refused before it
  starts: nothing is changed, nothing goes on the undo history, and the
  status line says what to do, with a number where a number helps. A number
  offered is one that is then accepted: it is found by asking the same
  question again. (A Join whose remesh is refused is still joined, plainly,
  and says why the remesh did not run.) "That resolution would make
  more than 80 million triangles. Try 900 or lower." "The objects are too
  far apart to unite at this resolution: the grid has to reach across all of
  them. Move them closer together, or lower the resolution to 362 or less."
  (A union's grid covers every object and the space between, at a voxel size
  set by the kept one; Subtract and Intersect only ever need the kept
  object's own space.) "That resolution needs too much memory for these
  objects. Try 1217 or lower." A union with something hundreds of times the
  size of the kept object has no resolution that works, and says so:
  "Uniting these objects would make too many triangles at any resolution."
  For an object far from its own mirror plane, or on the far side of it,
  see Symmetry. At 1024 (where the booleans' slider stops, and the Remesh
card's used to) a model that sits on
  its mirror planes is never refused for memory: everything the triangle
  limit allows there comes to under the 4 GB, and an ordinary model to a
  small part of it.
- **Reset Shape** puts the selected object back to the shape it had before
  sculpting: as it was made, loaded or last remeshed.
- **Set Origin to Mesh**, under Reset Rotation and Scale, moves the selected
  object's centre to the middle of its mesh (the middle of the box round it)
  and leaves the object exactly where it is: nothing changes on screen. The
  centre is the point the object's Position names, where the gizmo sits,
  what the object turns and resizes about, and where its own (Local) mirror
  planes and radial axis pass. A piece split off a model keeps the model's
  centre, which can be a long way from the piece; this gives it one of its
  own. The button is grey while the centre is in the middle already.
  - **How.** Every point of the mesh is shifted by the same amount inside
    the object and the object's Position is moved the other way, through its
    own turn and size, so the two cancel (to the last digit or two a number
    holds). Position shows new numbers; Scale and the turn are untouched.
  - **Instances** share the mesh, so each of them is corrected the same way,
    with its own turn and size, and stays where it is.
  - **Live mirror copies** are reflections across the world's planes of
    where the object is drawn, so they stay where they are too, and the
    mirror's own centre is not touched.
  - **A live array** stays where it is, copy for copy, tapered and turned
    ones included. An array is laid out from its object's centre, so the
    Array tool keeps the path where it was and notes that the object now
    sits off the path's start: **Copies Back onto the Path** lights up, and
    would slide the object's new centre onto the start. Leave it be to keep
    the copies where they are.
  - **Symmetry.** A World symmetry's planes are the world's and stay put. A
    Local symmetry goes through the object's centre, so its planes and its
    radial axis move with the centre, and the status line says so. That is
    the point for a piece far from its mirror plane (see Symmetry). On a
    model that is symmetric about the centre it already has, and whose box
    is lopsided (one arm raised), it would move the plane off the model's
    middle: do not press it there, or Undo.
  - **What comes across:** the mask, hidden parts, the quads of a quad
    remesh, and the shape Reset Shape goes back to.
  - **One undo step.** Ctrl + Z puts back every point of the mesh and every
    Position exactly (the mesh from before is kept whole, as for a remesh,
    and counts towards undo's memory), and an array follows. It is saved
    like any other change: a `.sclp` saved afterwards holds the mesh about
    its new centre and the Positions that go with it, and opens with
    everything where it was.
  - **When it is refused.** Unmerged tubes and planned keys are kept from
    the centre of the object they stand on, and an attached SSphere rig
    follows its object's centre; none of them is in a file, and each would
    be left behind. So the centre is not moved while there are any (tubes
    and keys on any object, a rig attached to this one or an instance of
    it), and the status line says which and what finishes it: Merge or Keep
    Separate, Build Keys, Detach. It also waits for a background job (a
    merge, a cut) to finish. One thing is not guarded: tubes drawn, keys
    planned or a rig attached after the centre was moved are left behind in
    the same way if Undo is then taken back past the move while they still
    stand. Finish them first.
- The eye shows or hides an object, the bin deletes it. Hiding the object
  you are working on moves the brushes to one you can still see.
- **H** hides the selection, **Shift + H** everything else, **Alt + H** shows
  everything. **/** is Local View: only the selection, framed; press it
  again and the rest comes back and the camera glides to where it was.
- The **Delete** key deletes the selected object after asking.
- Double-click a name to rename it (or use the field under the list).
- Press and hold a row and it lifts out of the list; drag it up or down and
  let go to move it. Near the top or bottom edge a long list scrolls.
- **Clone** makes an independent copy, with the same symmetry settings, and
  with the mask, the hidden parts and the quads of a quad remesh as they were.
  **Instance** makes a linked copy that shares the mesh (shown in blue):
  sculpt one and all change, and they share one symmetry (see Symmetry).
  **Make Unique** unlinks one (its own copy keeps the mask, hidden parts and
  quads too).
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
  step brings the separate objects back, shown again if they were hidden).
  So is every hide and show: an eye button, H, Shift + H and Alt + H alike,
  so Ctrl + Z brings back what a stray key hid. (The Unity app's keys were not undo
  steps, and the eye buttons here were; now the one rule holds for both.)
  Local View (/) is not: it is a way of looking, and / takes it back. What it
  hides is saved with the scene all the same (a sculpt saved in Local View
  opens with the rest hidden), so while it differs from the file the scene
  counts as unsaved (see Files).
- **Each undo step has a name**, and Undo says what it took back: "Undo
  Remesh", "Undo Clay Strips" (a stroke is named by its brush), "Redo Hide
  Cube". The **History** button (… at the end of the Sculpt panel's
  title line) lists every step Undo would take back, the latest at the top,
  and under them what Redo would bring back: tap one to go straight back to
  just before it, however many presses of Undo that is. Steps made together
  count as one, as Undo takes them. A step recorded by work that finished in
  the background, or inside a tool, is called by what kind of change it is
  ("Rebuild", "Objects"). While SSpheres or another tool with an undo of its
  own is up, Ctrl + Z steps through that first, so the list is not offered.
- Undo keeps the last 500 steps (it was 100), and no more memory than a quarter of the
  computer's (16 GB on a 64 GB machine; never less than 2 GB). An object that
  was deleted, joined into another or opened over stays in memory, whole, for
  as long as a step can bring it back, and counts towards that; so does the
  mesh from before each remesh. A mesh costs about 80 MB a million triangles.
  When undo holds more than the limit, the oldest steps are let go until it
  fits, the meshes only they could bring back leave memory straight away, and
  a line at the bottom of the view says so ("Undo memory is full: 4 oldest
  steps let go"). It says it once while one edit after another pushes a step
  out, not at each.
  - The last three steps are not let go for memory, so a big delete or remesh
    is not lost to the next stroke. If those three alone are over the limit,
    the steps before them go as far as the last one that keeps a whole mesh
    (a delete, a join, a remesh). Strokes made after that one stay: letting a
    stroke go frees next to nothing.
  - Twice the limit is the ceiling: over that the last three go too, oldest
    first. Only the very last step is kept whatever it holds, so the last
    thing done can always be undone; a step that big is let go by the next
    edit.
  - The **Performance** card on the Scene panel's Settings tab shows how much
    undo is holding ("History").

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
  everything showing, as in the Unity app. Cut & Mirror is the same. Decimate
  works on the whole model too, and what was hidden is still hidden
  afterwards (its triangles are the model's own, fewer of them).
- The wireframe draws only the faces that show.
- With Show Backfaces off (the default, see Look) you see through the
  opening to what is behind the model; switch it on to see the inside of the
  part that is left.

## Turntable

Press **O** (or **Spin** in the Turntable card on the Scene panel's File tab) and the
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

**Start Timelapse** (the Timelapse card on the Scene panel's File tab) records a sped-up
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

## The wheel, the panels and their cards

**The wheel shows the brushes pinned to it**, with Mask at the bottom. It
starts with eleven of the base brushes and Clay Strips in the plain Clay's
place (Clay, Erase Layer and Paint start off it). **Right-click a tile** on
the wheel to take that brush off it; the wheel stays open. **Right-click any
brush in the Brushes card** to put it on, and the wheel makes room for
however many there are.
A first run starts with Clay Strips in hand. Brushes of one kind sit together. A brush
that is not one of the base fourteen has its initials in the corner of its
tile, since it shares its kind's picture, and a long name is cut short on the
tile and shown whole in the middle of the wheel when the pen is over it.

**The brush wheel has a Strength bar under it.** Hold Space and the wheel
comes up with a bar beneath it (above it, when the wheel is at the bottom of
the window), the same as the size bar under the SSphere wheel. Press anywhere
on the bar and drag along it: it sets the Strength of the brush in hand, the
same Strength as the slider in the Sculpt panel and F + drag, remembered with
that brush. The bar is named after the brush it belongs to ("Clay Strips Strength"),
and that is always the brush in hand, never the tile the pen happens to be
over. A drag that starts on the bar never picks a tile, wherever it ends, and
letting go of Space over the bar only closes the wheel. While painting a mask
it reads "Mask Strength": a mask is painted at the strength of the brush under
it, so that is the value it sets. The Pose brush makes no use of Strength, so
its wheel has no bar. (The Unity app's wheel had a Size slider there as well;
this one has Strength only.) On a window too short for the bar under the
wheel or over it (a very small window, or a very large interface size), the
bar goes beside the wheel instead, and where there is no room there either
the wheel comes up without it. The wheel itself opens on a window of any
size; on one smaller than the wheel, part of it is off the edge.

**Each panel shows a tab's worth of its cards at a time**, so no card is a
long scroll away.

- The **Sculpt** panel keeps its top in place however far its cards are
  scrolled: the title with the triangle count and the History button (…),
  Undo and Redo, Sculpt / Transform / SSpheres, and three tabs: **Brush**,
  **Mesh** and **Tools**. On the Brush tab the size and strength plate (Add,
  Sub, Mask, Size, Strength, Focal Shift) is held there too, so it never
  scrolls away, as long as the panel is tall enough to keep it and still show
  cards under it (about 220 points of them); on a short window or at a large
  interface size it goes back to the top of the tab and scrolls with the
  rest. **Brush** has the Stroke card, the brushes and the settings of the
  one in hand; **Mesh** has Symmetry, Remesh with Dynamic Topology, Quad
  Remesh and Decimate, Subdivision, Masking, Hidden Parts; **Tools** has the
  tools that work on the model in their own way, under three headings that
  fold away (**Make**: Tube, Muscle, Lathe, Array, Extract; **Mask, Hide &
  Cut**: Mask, Hide, Trim, Split; **Print & Measure**: Measure, Keys). They
  used to sit above the tabs, which is what pushed Size and Strength out of
  sight; their keys work from any tab. A tool with a panel of its own
  (SSpheres, Tube, Lathe...) shows the tool buttons and its panel in place
  of the tabs while it is up. Each tab keeps its own place in its scroll.
- The **Scene** panel has four tabs along its top, which stay put however far
  the cards under them are scrolled: **Scene** (the objects, and Measure &
  Size), **Look** (Material, Lighting, Backdrop & Effects), **File** (the
  file buttons and recent files, Turntable, Timelapse) and **Settings**
  (Theme, Interface, Keyboard Shortcuts, Performance). The status line is
  under the tabs whichever is showing (and takes no room until there is
  something to say), and each tab keeps its own place in
  its scroll.
- **Every slider can be typed into, and moved finely.** Double-tap a slider
  and a box opens over its number: type the number as the slider shows it
  (45 for 45%, 256 for a resolution, 1.2k for 1,200) and press Enter, or tap
  anywhere else; Esc leaves it as it was. The first tap of the double tap
  would have moved the slider, and that is undone before the box opens.
  Shift + drag moves a slider a tenth as far as the pen goes, from where it
  is, for the last small step. (The bars under the Space wheels drag only.)
- **Reset Brush**, at the foot of the brush's settings card, puts every
  setting of the brush in hand back as it came with the app (or as you made
  it, for one of your own): the same as Reset in its menu. The size of the
  ring is shared by all brushes and stays as it is.
- A card that floats over the view stays up whichever tab is showing. Put it
  back while its tab is not the one showing and the panel turns to that tab,
  so the card does not simply vanish.
- Every key works whichever tab is showing: R remeshes from the Brush tab as
  it does from the Mesh tab. While the Mesh tab is not showing, dynamic
  topology's "Triangle limit reached" is said on the plate over the view (see
  below), since the Remesh card that says it is out of sight.
- Which tab each panel is on is remembered between runs.

**The interface comes in eight colour themes.** The **Theme** card on the
Scene panel's Settings tab has a tile for each, painted in its own colours:
Midnight (the one the app starts in), Graphite, Slate (the lightest),
Clay, Forest, Lagoon, Plum and Carbon (black, for a dark room). A tap changes
the panels, the cards, the buttons, the wheels and the accent that marks what
is switched on, at once, and it is remembered between runs. What says
something by its colour is the same in every theme: the axes, Sub's red,
Mask's orange, a result in green and an error in red, and the coloured mark
on each card.

The backdrop behind the model goes with the theme, as long as it is one
nobody chose: pick a colour of your own for it (Backdrop & Effects, on the
Look tab) and it stays yours through every theme, and in the sculpt's file. A
sculpt saved with a theme's own backdrop opens with the backdrop of whichever
theme is up. Every theme has light text on a dark or middling surface; there
is no white one, because the icons and the hover looks are painted in white.
The themes are a list in `crates/toolkit/src/palette.rs`, where one can be
changed or added.

**The interface can be drawn smaller or larger.** The **Interface** card on
the Settings tab has 75%, 100%, 125%, 150% and 200%; Ctrl + = and Ctrl + -
step between 75% and 300% a tenth at a time. Tooltips are switched on and
off in the same card.

**Tooltips can be in another language.** **Tooltip Language**, in the same
card, puts every tooltip and "How it works" note in Spanish, Portuguese
(Brazil), French, German, Italian, Polish, Turkish, Russian, Japanese,
Chinese (Simplified) or Korean, for someone more at home in one of those than
in English. The first time the app is opened on a computer it asks, before
anything else, with the language Windows is in already picked and the
question's own words turning to whichever is picked. The buttons and cards
keep their English names, and a tooltip
names them as they are on screen, so it can be matched to what it is about.
Each language is a plain text file in `crates/toolkit/translations/`, the
English and the translation in pairs of lines; a translation is fixed by
editing its line and rebuilding. A tooltip written since is shown in English
until `python tools/translations.py todo DIR` lists it for translating
(`status` counts what each language is missing).

**Either panel folds away.** Each panel has a small arrow tab on its inner
edge, just under the view gizmo's buttons. Tap it and the whole panel
goes, leaving only a slim tab at the edge of the screen; tap that and the
panel is back as it was, cards and scrolling included. The two sides fold
separately. Everything else stays: the view gizmo and the Flat Shading /
Wireframe row move out to the screen edge, the brush, the tools,
the wheel and every key work as before, and the model has the room. This is
not the clean view (Tab), which is for looking and switches strokes off; Tab
still hides everything, tabs included, and brings the panels back folded or
not as they were. Which panels are folded is remembered between runs. There is
no key for it. A tool whose panel is folded away still works from the
viewport, its wheel and its keys; its settings are a tap on the tab away.

**What a folded panel would have told you is said over the view.** The status
line at the top of the Scene panel is where the app says what it just did or
could not do ("Could not save...", a Remesh that was refused, what a tool's
cut left behind). While that panel is folded away, the same words in the same
colours come up on a small plate at the top of the view, between the panels
and beside the view gizmo; a long message wraps onto several lines.

- **An error (red) stays** until something else is written, however long
  that is: look up a minute later and it is still there.
- **Anything else fades** once there has been time to read it: about six
  seconds for a few words, longer for a long message, half a minute at most.
- **Tap the plate to put the message away** (it has a small cross in its
  corner). It is still on the Scene panel's status line when the panel comes
  back. The plate is the only part of the view that takes the pen; right
  beside it you are sculpting.
- With the Sculpt panel folded away, two notes from its cards join the
  plate: dynamic topology's "Triangle limit reached", for as long as that
  holds (it is there too while the panel is up but on its Brush tab), and the
  SSphere tool's one-off results. They go by themselves, so a
  plate with only those on it lets the pen through to the model.
- The clean view (Tab) hides the plate with everything else, except for an
  error. Ctrl + S works there, and a save that failed must not look like one
  that worked: the red line comes up at the top as it does over a folded
  panel, and stays until it is tapped away or the panels come back. What
  went well is not said there.

**A line over the bottom bar** says what every stroke works under, so it
can be seen from any tab: the brush in hand (or "Mask painting"), the
selected object's mirror axes and radial symmetry, "Masked" when it has a
mask, and how many of its faces and how many objects are hidden. Only what is
on is named; a plain scene shows the brush alone. It is for reading only.

Background work (a merge, a cut, Make Real) shows its progress and its Cancel
at the bottom of the view whatever is folded, and the question a Delete or an
unsaved file asks comes up over everything. A tool's own buttons (Accept,
Create, Done) are in its panel: bring the panel back, or float that card.

**A double tap on a control that goes away does not mark the model.** The
fold tab jumps to the edge of the screen when it is tapped, and the button
that puts a floating card back takes its window with it. A second tap that
follows within a third of a second, on the same spot, would land on the model
underneath; it is ignored. A stroke begun anywhere else, or any later, is a
stroke as always.

**Any card floats.** Every card of both panels (Brushes, the brush's own
settings, Stroke, Masking, Hidden Parts, Symmetry, Remesh, Objects, File,
Material, Lighting and the rest), and every card of a tool's panel (Measure,
Tube, Array, Lathe, Keys, the region tools, the SSphere cards, Measure &
Size), has a small window button in its header, beside the arrow that opens
and closes it. Tap it and the card leaves its panel for a window of its own
beside it. It is the same card, not a copy: everything in it works as it did.

- **Move it** by dragging its title with the pen. The window last touched is
  on top, and floating cards always sit over the panels.
- **Put it back** with the button in its top-right corner (the same picture,
  with the arrow going in). It goes back to its place in its panel.
- **It stays when its panel is folded away**: fold both panels and keep just
  the two or three cards in use. Tab (the clean view) hides the floating
  cards with everything else, and brings them back where they were.
- **Nothing gets through it**: a press, a drag or the scroll wheel on a
  floating card never reaches the model behind it.
- **It cannot be lost.** A window is always drawn whole inside the app's
  window, clear of a folded panel's tab. Make the app's window small, or the
  interface large, and the card comes in with the edge; make room again and
  it goes back to where you left it. A card taller than the window scrolls.
- **It is remembered**: which cards float, and where, are kept in the
  settings file with the rest. An entry there that makes no sense is skipped
  and that card starts in its panel.
- **A card shows when its panel would show it.** Brushes floats while you
  sculpt and steps aside while a tool's panel is up; a tool's card (Tube,
  say) floats while that tool is up, goes when the tool is put away and is
  back in the same place the next time. Measure & Size and the other Scene
  cards are there whatever tool is up. The settings card of the brush in
  hand is one card whichever brush that is, and the two Symmetry cards (the
  sculpt one and the SSphere one) share a place.

## What is here, and what is not yet

Ported from the Unity app, each described in its own section above:

- **Sculpting**: the ten brushes with their settings (the Pose brush is built
  on Blender's rather than the Unity app's), mask painting and mask
  operations, mirror symmetry on X / Y / Z and radial symmetry, dynamic
  topology, voxel remesh, quad remesh, hidden parts, and undo / redo.
  Decimate, Set Origin to Mesh, the buttons for Cut & Mirror, brush
  alphas with Drag to Place, the Pinch, Nudge and Erase Layer brushes and the brush
  library (named brushes, your own and the ones that ship) are this app's own,
  as are subdivision levels, sculpt layers, face groups and Polypaint (paint
  layers, materials as paint, the Paint brush).
- **Objects**: the scene graph (add shapes, show / hide, rename, clone,
  instance, reorder), join and the booleans, live mirror copies, and the
  transform gizmo and view gizmo.
- **Tools**: SSpheres (with the gizmo's arrows on a sphere), Measure, Array,
  Lathe, Tube, Keys, and Mask, Hide, Trim, Split and Extract. The Muscle tool
  is this app's own.
- **Look**: matcaps, the lit materials and finishes, glow, lighting presets,
  HDRI lighting, backdrops and the post effects, cavity included.
- **Files and recording**: `.sclp` files, OBJ and STL import and export,
  recent files, the turntable with its 360 loop as a video or as pictures,
  and the timelapse. Recovery copies against a crash are this app's own: the
  Unity app has none. So is the link to Blender (see Blender).
- **Interface**: the two panels and the tabs that fold them away, cards that
  float over the view, the brush wheel with its strength bar, the clean view,
  tooltips and interface size.

A quad remesh ignores symmetry. A remesh, a join, a boolean and a tube merge
each start with a clear mask and everything showing.

The Unity app's Symmetry Repair panel is all here, at the foot of the
Symmetry card (see Symmetry). Its Match Up row is called Make Symmetric, the
name the status lines use. Check Symmetry says one thing more than the Unity
app's did: whether the pairs it counts are mirror images of each other.

SSpheres, unmerged tubes, rulers, planned keys and the lathe's outline are
scaffolds: they are not kept in `.sclp` files or in recovery copies, so make
the skin, merge the tube, build the keys or create the mesh before closing the
app. The app asks before it closes over any of them but rulers, and before an
Open or a New Scene that would take the tubes or the planned keys with the
scene (see Files).

Left out on purpose: the Unity app's mold maker and hard-surface tool.

## For development

```
cargo build --profile dist    # the program as handed over (target/dist): the whole program optimised as one piece, about 2% faster per brush frame than --release, a minute or so slower to build
cargo test --release          # engine tests
cargo test -p sculpt-geom --release timing_of_decimation -- --ignored --nocapture   # the decimation engine alone at 200k / 500k / 1M triangles, to 50% and 20%: time, closed, same volume
cargo test --release timing_of_a_decimate -- --ignored --nocapture                  # Decimate as the app runs it, sculpt mesh included, at 200k and 1M triangles, with Mirror X, and the preview worked out and a count shown; prints the status lines and times
cargo test --release timing_of_a_clay -- --ignored --nocapture   # what dynamic topology costs a frame at 330k / 1.3M / 3M triangles
sculpt.exe --bench            # engine timings at 200k / 1M / 4M triangles
sculpt.exe --bench-render 1000000 5000000 10000000   # the viewport's drawing alone: ms per frame orbiting and with nothing changing (a pointer move), per finish, half off screen, faces hidden
sculpt.exe --bench-render-ab other\sculpt.exe 3 1000000 5000000 10000000   # this build against another, taking turns, best of 3 each, with the ratio
sculpt.exe --render-shots shots
ew   # a fixed set of viewport pictures (every finish and preset, glows, matcap, a mirrored copy, an opened model...)
sculpt.exe --compare-shots shots\old shots
ew   # two such sets compared pixel by pixel
SCULPT_BENCH_ROWS="Clay,Smooth,Clay|" sculpt.exe --bench 1000000   # only those rows (as labelled; rows ending in | run beside the mirror plane)
sculpt.exe --brush-feel       # each brush along a line on a 200k ball at 10/50/100% Strength, its own, and a 5% pen touch: how tall one and three passes build, ridge sharpness, triangle quality, volume change, how far the back of a thin slab moves, noise left by the smoothing brushes
sculpt.exe --brush-hash       # scripted strokes of every brush, symmetry, mask, hidden faces, layers and dynamic topology, with a hash of the mesh after each: a speed change must leave every line the same
sculpt.exe --bench-levels 1000000 5000000 10000000   # subdivision levels: subdivide, down, up with and without a small edit, at about those finest-level triangle counts; each line prints a fingerprint of what the step made, to compare two builds bit for bit
cargo test -p sculpt-multires --release timing_of_the_two_ways -- --ignored --nocapture   # an edit carried up from what it changed against every vertex looked at, as the edit grows
sculpt.exe --screenshot out.png --size 1600x960 --demo   # scripted strokes (SCULPT_DEMO_BRUSH=focal=-0.8,curve=plateau,fixed sets the brushes up first; spacing=150, scatter=50 and jitter=40 set every brush's Spacing, Scatter and Size Jitter in percent, and alpha=scales+random180 an alpha with Random Rotation)
SCULPT_DEMO_BRUSH=radial=6,axis=z,mirror=off sculpt.exe --screenshot out.png --demo   # the same strokes six times round Z, seen down the axis (axis=x | y | z; leave mirror=off out for Mirror X as well; space=world fixes the symmetry in the world; radial= also works with --demo-mirror and --demo-dyntopo)
SCULPT_DEMO_SYMMETRY=repair sculpt.exe --screenshot out.png --demo-symmetry --demo-ui "float=Symmetry@640:20"   # Check Symmetry, Make Symmetric and Symmetry Cleanup pressed in turn on a model made lopsided for them, each undone and redone; prints the status lines and a line per check (ok / FAIL)
sculpt.exe --screenshot out.png --demo-symmetry      # two objects with a symmetry each (a ball with Mirror X, a turned cube with five repeats about its own Y), a stroke on each, the Symmetry card showing the selected one's (SCULPT_DEMO_SYMMETRY=cube selects the cube; world stands a turned ball astride the world's X plane with Mirror X in World space; select:<name> with a file selects that object of it; cut presses Cut & Mirror on a copy of a lumpy model, then Undo and Redo, and shows the model before and after in wireframe, printing the status line and a line per check, and cut:alone shows the result by itself (add --demo-ui "float=Symmetry@640:20" to see the whole card, buttons and all); corner presses Layer once where Mirror X and Y cross and once clear of the crossing, seen from the side, where both steps should be one height, and corner:front shows the same from where the dabs were made; even draws five short strokes down a ball with Mirror X, on the plane, 0.2, 0.4 and 0.75 of a brush radius from it and well clear of it, which should all stand as tall with no hard line down the middle, and even:clay or even:inflate:0.3 draws them with that brush at that strength)
sculpt.exe --screenshot out.png --demo --demo-hide   # half of the model hidden, cut on a slant
sculpt.exe --screenshot out.png --demo --decimate 8000 --look "wire=on"   # the scripted sculpt decimated to that many triangles by the routine under the Decimate button (it waits for the job), status line printed; add --demo-ui "float=Remesh@440:20" to see the card
sculpt.exe --screenshot out.png --demo --demo-decimate   # Decimate worked through its buttons, its question about radial symmetry, a mirror kept point for point, the preview (opened, slider moved, Apply, Esc, an edit, Undo), Undo, Redo, Cancel and a model edited under it; prints the status lines and a line per check; the picture is the preview left open (add --demo-ui "size=0.75,float=Remesh@1240:40" to see the card; SCULPT_DEMO_DECIMATE=ask stops at the question, for a picture of it)
sculpt.exe --screenshot out.png --demo-dyntopo       # a spike pulled out of a coarse ball, then refined by an Inflate stroke with dynamic topology on; wireframe showing (SCULPT_DEMO_DYNTOPO=off | dense | instance | mirror | undo | live for more)
sculpt.exe --screenshot out.png --clean-view --demo-feel Standard   # a brush (any library name) at 10%, 50% and 100% Strength and then tapering with the pen, four strokes across a ball; Standard:rough lays a fine ripple first (for the smoothing brushes), Standard:fin strokes a thin slab and Standard:back shows that slab from behind; Standard:thrice makes each stroke three passes there and back
sculpt.exe --screenshot out.png --demo-scene   # one of each shape + instances
sculpt.exe --screenshot out.png --demo-select  # the same with three objects selected (SCULPT_DEMO_SELECT=ask | local | subtract | union | intersect | play for more; apart asks for a union that is not started, to see what the status line says; origin stands the centre of the cube, which has two instances, a live mirror and a live array, 400 lengths off, then presses Set Origin to Mesh, Undo, Redo and Remesh, printing the status lines and a line per check; add --demo-ui "float=Objects@440:20" --size 1600x1400 to see the button)
sculpt.exe --screenshot out.png --demo --look "panels=off,matcap=off,finish=metal:rust,light=moody"
sculpt.exe --screenshot out.png --demo --demo-matcap big.png   # a picture imported as a matcap by the routine under Import Matcap, with no file box: any size (it is copied into assets/matcaps/Imported)
sculpt.exe --screenshot out.png --demo-spheres rig   # SSphere figure: rig | preview | skin | back | again | attached | followed | followed-skin | wheel | new | play | gizmo | gizmo-held | gizmo-play
sculpt.exe --screenshot out.png --demo-mirror  # the sphere off centre, its live mirror on, strokes laid on the copy (SCULPT_DEMO_MIRROR=xz | gizmo | validate | front | back | play)
sculpt.exe --screenshot out.png --demo-pose bend     # a figure posed with the Pose brush: hover | bend | chain | twist | resize | slide | stretch | done | wheel | play (bend:close moves the camera in; :soft and :offset change those settings)
sculpt.exe --screenshot out.png --demo-gizmo   # the gizmo on a turned object (SCULPT_DEMO_GIZMO=global | local | three | three-turn | masked | play for more)
sculpt.exe --screenshot out.png --demo-tool measure  # a hosted tool: id or id:what (measure:inches, array:curve, keys:applied, studs:job, any:play)
sculpt.exe --screenshot out.png --demo-tool trim:mirror   # a shape drawn over the object's live mirror copy: also hide:mirror, mask:mirror, split:mirror
sculpt.exe --screenshot out.png --demo-tool array:mirror  # a mirrored column of copies; array:copy picks one (the gizmo on it), array:posed moves and turns it, array:onpath puts the move back
sculpt.exe --screenshot out.png --demo-tool hide:array    # a shape drawn over one copy of an array (the Array tool lays a column out first): the same hole in every copy; also mask:array, trim:array, split:array
sculpt.exe --screenshot out.png --demo-tool array:point   # the gizmo's arrows on the end point of the path; array:arrows drags them sideways, array:start puts them on the square start point and drags that; array:arrow drags an arrow with the pointer and Z, printing a line per check
sculpt.exe --demo-tool array:stroke                       # brush strokes begun on an array's copies by way of the pointer routines; prints a line per check (ok / FAIL), add --screenshot out.png to see the result
sculpt.exe --demo-tool measure:focus                    # a stroke, a camera drag and a tool's drag cut off by the window losing focus; prints a line per check (ok / FAIL), add --screenshot out.png to see the result
sculpt.exe --demo-tool measure:turn                     # the view turned by a drag, the view gizmo and an axis view with the camera's pivot left far behind the model, a turn out of the axis view (back to perspective) and the Ortho / Persp button both ways, then a double-click on the background, the gizmo's pan and zoom handles and Home: the model stays where it is on screen; prints a line per check (ok / FAIL)
sculpt.exe --screenshot out.png --demo-tool array:several --demo-files array   # three arrays (a ring, a curve, a mirrored column) saved, opened, undone, redone, added and exported; prints a line per check; the picture is the reopened scene
sculpt.exe --screenshot out.png --demo-tool array:as-is some.sculpt            # the Array tool up on the arrays a file brought with it (a Unity sculpt that has one): the first object with an array is selected, its path and handles show
sculpt.exe --screenshot out.png --demo-tool tube:radial   # a horn repeated six times round Y; also tube:mirror, tube:radial-mirror (three repeats and a mirror), tube:surface, tube:object (kept separate), tube:merged, tube:radial-merged
sculpt.exe --screenshot out.png --demo-tool lathe:radial  # Create with Radial Symmetry on Create ticked (the vase gets the radial guides); also lathe:created, lathe:edit, lathe:bowl, lathe:ring, lathe:sketch
sculpt.exe --screenshot out.png --demo --clean-view  # the Tab view with its hint (SCULPT_DEMO_CAMERA=turntable | timelapse | recording shows a card or the recording bar instead)
sculpt.exe --record-loop folder --demo               # writes a short 360 loop and closes: 12 pictures at 640x360 (--loop-size 1080x1920 --loop-frames 24 for others)
sculpt.exe --record-loop out.mp4 --demo --loop-fps 60 # the same as a video (a name ending in .mp4): then reads the file back and checks its frames, size, frame rate and length
sculpt.exe --record-loop out.mp4 --demo --loop-cancel 15   # cancels after 15 frames (SCULPT_DEMO_CAMERA=close closes the app instead) and checks nothing of the loop is left; a folder or file of that name that was already there is checked to be exactly as it was
sculpt.exe --demo-timelapse out.mp4                  # sculpts a few scripted strokes while recording, prints what the file holds, closes (SCULPT_DEMO_CAMERA=close closes the window mid-recording, SCULPT_DEMO_CAMERA=exit presses the File card's Exit and answers Exit Without Saving: either way the video must be whole)
SCULPT_DEMO_CAMERA=check sculpt.exe --record-loop folder --demo   # presses the turntable's own keys and buttons and prints a line per check
sculpt.exe --screenshot out.png --demo --demo-files recent  # the File card's recent tiles; or ask-open-clean | play
sculpt.exe --screenshot out.png --demo-blender play   # the Blender link worked without Blender: this run writes what Blender's side would; prints the status lines and a line per check (round and live put a real Blender at the other end: tests/blender_link/README.md)
sculpt.exe --screenshot out.png --demo --demo-tool tube --demo-files ask-exit   # the question the close button would ask of the scene the other options left: here unsaved strokes and a live tube; ask-open is the one choosing a file would ask
sculpt.exe --screenshot out.png --demo-spheres attached --demo-tool tube --demo-files ask-exit:saved   # the same with the scene taken for saved: only what is in no file is left to ask about
sculpt.exe --screenshot out.png --demo-files ask-exit:mirror   # a change made first: mirror (the X key) | local (/), saved without an undo step | hide (H), which is one
sculpt.exe --screenshot out.png --demo-files lost   # what closing and opening would lose, worked through the keys and every tool; prints a line per check
sculpt.exe --screenshot out.png --demo --demo-files open-undo   # Open, Undo, Redo and Save in every order, looking at what is in the files after each, and at the look and the view the scene is seen with; prints a line per check (name a .sclp or .sculpt to start from that file; open-undo:opened and open-undo:undone stop after an Open, and after its Undo, for a picture of the File card)
sculpt.exe --screenshot out.png --demo --demo-files new         # New Scene and Exit through the routines the buttons run: unsaved work, a saved sculpt, Save and Undo in every order (looking at what is in the files), an array, a tube, spheres, a job at work, a stroke in progress; prints a line per check (new:fresh and new:undone stop after New Scene on a saved sculpt, and after its Undo, for a picture of the File card)
sculpt.exe --screenshot out.png --demo --demo-tool tube --demo-files ask-new   # the question New Scene would ask of the scene the other options left (ask-new:saved, ask-new:mirror as for ask-exit)
sculpt.exe --screenshot out.png --demo-levels play          # works the Subdivision card, everything that drops levels, undo, saving and export, and prints a line per check; or a picture: base | fine | down | bent | up | card | quads (add --demo-ui "float=Subdivision@330:40" to see the card)
sculpt.exe --screenshot out.png --demo-layers play          # works the Layers card, every brush on a layer, the model underneath, everything that merges layers, levels, the bridge, saving and a damaged file, and prints a line per check; or a picture: card | full | half | off | under | under-off | dyntopo | dyntopo-off | levels | levels-off | levels-low (add --demo-ui "float=Layers@330:60" to see the card)
sculpt.exe --screenshot out.png --demo-groups play          # works the Face Groups card, the click on the model, Stay in Group for each kind of brush, everything that rebuilds a mesh, levels, saving, a damaged file, OBJ and the bridge, and prints a line per check and some timings; or a picture: parts | figure | figure-wire | figure-flat | figure-lit | isolate | stay | stay-off | quads | card (add --demo-ui "float=Face Groups@370:60" to see the card)
sculpt.exe --screenshot out.png --demo-deform play          # works the Deform card's deformers through the pointer, the keys and undo: Esc and Undo to the bit, handles dragged, a mirrored model warned of, a hidden layer refusing, the bridge, three objects as one; prints a line per check; or a picture: bend | arc | twist | taper | flatten | inflate | stretch | lattice | masked | three (add --demo-ui "size=0.75,float=Deform@1240:40" to see the card)
sculpt.exe --bench-deform 1000000 5000000 10000000         # one preview frame of each deformer (the pass over the mesh, its normals and the ranges to send), best of six, at each size
sculpt.exe --screenshot out.png --demo-references photo.jpg,front,ortho   # a reference picture added by the routine under Add Picture..., with no file box, after the camera turns to that view (front | back | left | right | top | bottom; ortho for orthographic); in-front and a number (opacity in percent) set those, orbit=35:-15 then turns round the model in perspective (orbit=180:0 looks from behind, where the picture is not drawn); add --demo-ui "float=Reference Images@560:20" to see the card
sculpt.exe --screenshot out.png --demo-references play     # works the pictures through the facing a view gives, fitting, the unsaved mark, Save, Delete, Open, Undo and Redo of the Open, Import and the bridge's commands, and prints a line per check
sculpt.exe --screenshot out.png --demo-bake                 # Bake Maps refused on a model without UVs, Unwrap UVs on a quad remesh of a rippled ball, a bake of all four maps into a scratch folder (the files there, the normal map tilted where the ripples are and flat outside the islands), and Undo; prints a line per check
sculpt.exe --screenshot out.png --demo-print check          # the sphere with two holes cut in it and a face doubled, and Check for Printing pressed; or closed | export (the question) | remesh | quads | damaged (the damage alone, for --demo-tool trim to work on) | play (prints a line per check)
cargo test -p sculpt-geom --release timing_of_the_census -- --ignored --nocapture   # what the printing check costs at 1M / 10M / 40M triangles
cargo test --release a_closed_model_comes_out_closed -- --nocapture                 # quad remesh on six closed models at seven sizes, a line a run: every result must be closed
cargo test --release a_model_with_openings_keeps_them -- --nocapture                # quad remesh on five models with one or two openings at six sizes: each opening must still be one
sculpt.exe --screenshot out.png --demo --demo-ui "brush=Clay,strength=0.35,wheel"   # the brush wheel with its strength bar; a list of: wheel | wheel=low (at the bottom, the bar above it) | wheel=left (at the left edge) | brush=<name of any brush of the library> | copy=<name> (a brush of the user's own, made from the one in hand) | pin=<name> (that brush on the wheel) | shiftbrush=<name> (the brush Shift uses) | hover=<x>:<y> (the pointer over the view, that far across and down it, so the ring and the alpha under it show; add --test-keep-pointer) | shift (Shift held) | hand=<brush>@<thousands of triangles>@<ring px>@<seconds>[@<spacing %>][:x<crossings>][:lift] (a stroke drawn by hand on a dense ball and measured; it waits for the bridge's `command hand_stroke`, and `command hand_report` gives the numbers) | mask | strength=<0..1> | size=<interface size, 1.5> | fold=left / right / both (panels folded away) | float=<card>@<x>:<y> (that card floating, its corner there in the interface's points; as many as wanted) | status=error / status=ok / status=fading (a made-up long error, a short success, or that success part-way through fading, on the status line) | limit (dynamic topology at its triangle limit) | pen=<0..1> / pen=up (the Stroke card's pen readout with a pen pressing that hard, or lifted; without it, the line that says no pressure has arrived)
sculpt.exe --screenshot out.png --demo --demo-ui "fold=right,status=error"   # the status line over the view while the Scene panel is folded away (fold=both,status=error,limit for both panels' notes; --size 1600x330 with wheel for the bar beside the wheel)
sculpt.exe --screenshot out.png --demo --demo-ui "fold=both,float=Brushes@60:40,float=Objects@700:60"   # two cards floating with both panels folded away (add --size 700x420 to see them follow a small window in)
sculpt.exe --demo --demo-ui play                        # works the interface with a scripted pen (the bar under the wheel, the panels' fold tabs, a card pulled out, dragged, kept through folded panels and put back, the status line over the view with a panel folded, a card asked to open while folded away, the wheel on a short window, a double tap on a fold tab), prints a line per check (ok / FAIL) and closes; keep the real pointer off its window, and give it an empty SCULPT_SETTINGS_DIR (it changes the interface size as it goes)
sculpt.exe --screenshot out.png --view 0,0 some.sclp        # a file (.sclp, .sculpt, .obj, .stl) seen whole from a yaw and pitch: 0,0 front | -90,0 right | 180,0 back | 0,89 top
sculpt.exe --screenshot out.png --demo-files unity some.sculpt   # a sculpt from the Unity app: saved as a .sclp, reopened, added; prints a line per check
SCULPT_UNDO_BYTES=1 sculpt.exe --screenshot out.png --demo   # undo's memory limit set in bytes (here to nothing): the second stroke lets the first go, and the line that says so is showing
SCULPT_UNITY_SAMPLES=folder cargo test unity_import -- --nocapture   # the Unity sculpt reader against every .sculpt in a folder, with what each holds (they are not in the repository)
```

A run started with any `--option` is a scripted one: it neither reads nor
writes the user's settings file, so a visual check never depends on how the
app was last left and never leaves its brush settings behind. Set
`SCULPT_SETTINGS_DIR` to a folder to give such a run settings of its own.
The same goes for the log (it is written beside the settings, so nowhere
without that folder), for the Claude bridge (its key is kept there too, so a
scripted run without the folder has no bridge) and for recovery: a scripted
run keeps no recovery copies and offers none, even with a settings folder, unless
`SCULPT_TEST_RECOVERY` is set, which makes the run a check of recovery
itself. That variable does nothing at all unless `SCULPT_SETTINGS_DIR` is
set too: a check answers the question by itself, Discard It included, and
must never be let loose on the copies in your own `%APPDATA%\Sculpt`. It is
a list separated by commas: `every=0.3` (seconds between
copies), `answer=restore | earlier | discard | later` (the answer to the
question at start-up), `message` (the message the app closes with on a fault
is really put up, and waits to be closed), `restore-fault` (a fault strikes
while the copy is being restored, when its objects have taken the scene's
place and nothing else has been done),
and steps to take in a numbered frame: `stroke@30`, `scene@40` (prints a
fingerprint of the scene),
`save@45` (to `saved by the check.sclp` in the settings folder), `open@46`
(opens `to open.sclp` from that folder in place of the scene), `new@46`
(New Scene, answered "New Scene Without Saving"), `undo@47`,
`redo@48`, `file@49` (prints the name of the scene's file and whether
anything is unsaved), `status@49` (prints the status line), `mirror@50`, `hide@50`, `show@50` and `local@50` (the
X, H, Alt + H and / keys: changes the file keeps; X and / are no undo step, H and Alt + H are),
`kept@50` (prints a fingerprint of what the file would keep of the objects
besides their meshes: the same after a restore as before the crash),
`close@50` (prints what the close button would ask), `quit@50` (a clean exit), `kill@50` (ends the program on the
spot), `panic@50`, `stroke-panic@50` (a panic half way through a stroke),
`job-panic@50` (a panic in a background job, which must come back as an
error and leave the app running), `graphics-error@50` and `device-lost@50`.
A crash and what follows it, end to end:

```
export SCULPT_SETTINGS_DIR=D:/some/empty/folder   # every line below needs it
SCULPT_TEST_RECOVERY=every=0.3,stroke@20,scene@200,stroke@205,scene@210,panic@211 sculpt.exe --size 1600x960 --demo   # two fingerprints, then "stopped: Kept"; log.txt and recovery/ appear in the folder
SCULPT_TEST_RECOVERY=on sculpt.exe --size 1600x960 --screenshot out.png                     # the question about the copy
SCULPT_TEST_RECOVERY=answer=restore,scene@5,quit@8 sculpt.exe --size 1600x960               # prints the second fingerprint again; restoring and closing without saving removes nothing, so recovery/ still holds the copies
SCULPT_TEST_RECOVERY=answer=restore,undo@5,close@6,quit@8 sculpt.exe --size 1600x960        # Restore It, one Undo, close: "closes without asking", and the copies are still there
SCULPT_TEST_RECOVERY=answer=restore,restore-fault,file@5,status@6,quit@8 sculpt.exe --size 1600x960 some.sclp   # a fault while restoring: "file: none, unsaved true", never some.sclp, and a status line that says what happened
SCULPT_TEST_RECOVERY=answer=restore,scene@5,save@6,quit@8 sculpt.exe --size 1600x960        # saving the restored scene is what retires the copies: recovery/ is then empty
SCULPT_TEST_RECOVERY=every=600,stroke@20,scene@30,new@40,scene@50,kill@60 sculpt.exe --size 1600x960 --demo   # work too young for a timed copy, then New Scene Without Saving: "copy copy-0001.sclp ..." is written first; answer=restore at the next start prints the first fingerprint again
SCULPT_TEST_RECOVERY=every=0.3,mirror@20,kept@26,kill@200 sculpt.exe --size 1600x960        # a change that is no undo step still gets a copy ("copy copy-0001.sclp ...")
SCULPT_TEST_RECOVERY=answer=restore,kept@5,quit@8 sculpt.exe --size 1600x960                # and comes back with it: the same "kept" line
cargo test --release timing_of_a_recovery_copy -- --ignored --nocapture                     # what a copy costs at 1M / 4M / 12M triangles
cargo test --release recovery::rule -- --nocapture                                          # when a copy may be removed, walked through random runs that crash, close and save
```

The release build unwinds on a panic (`panic = "unwind"` in `Cargo.toml`)
rather than aborting: that is what lets a panic in a tool's background job
come back to the tool as an error, and a panic on the interface thread be
caught at the event loop so the scene can be written before the app closes.
A build with `panic = "abort"` is refused (`src/main.rs`).

Tools are added as crates against `sculpt-toolkit`, not wired into the app:
see `crates/toolkit/README.md`.

### Claude bridge

Claude Code sessions opened in this folder get a `sculpt` MCP server (set up in
`.mcp.json`), which lets Claude see and drive the running app: start and close
it, read its state, take screenshots, set the brush, draw strokes, move the
camera, manage objects, and open, save and export files. Everything it changes
is an ordinary undo step.

`set_brush` works on the brush library (see Brush library): `brush` picks any
brush by name, base, shipped or the user's own; `list: true` returns every
brush with its kind and where it came from; `duplicate_as`, `rename`, `reset`,
`delete`, `pinned` and `shift_uses` do what the buttons under the list do; and
every value of a brush's record can be set (its kind's own numbers, such as
`clay_depth`, are refused on a brush of another kind rather than silently
doing nothing). `import_file` and `export_file` take a full path to a `.json`
brush file, held to the same rule about paths as `open_file`; the bridge never
opens a file box. `stroke` answers with the name of the brush it drew with.
`command` has `mask_recesses` and `mask_peaks` (with `amount`, 0 to 1) beside
the other mask commands.

`command` also works subdivision levels and sculpt layers on the active
object: `subdivide` (with `flat`), `level` (with `level`, 1 for the
coarsest), `level_up`, `level_down`, `delete_lower_levels` and
`delete_higher_levels`; and `layer_new`, `layer_record`, `layer_strength`
(with `percent`), `layer_show`, `layer_hide`, `layer_rename` (with
`new_name`), `layer_delete`, `layer_bake` and `layer_bake_all`, where `layer`
is a layer's name or its id. `get_state` gives each object's `levels` (which
one it is on, and the triangles of each) and its `layers` (id, name, whether
it shows, its strength and which records). A stroke is refused, with the
reason, while the layer that records is hidden or at 0%.

And face groups: `group_masked` (with `keep_mask`), `group_visible`,
`auto_groups`, `groups_by_angle` (with `angle`) and `clear_groups` make and
clear them; `pick_group`, `isolate_group`, `hide_group`, `mask_group` and
`mask_all_but_group` work on one group, given as `x` and `y` (a window
position on the active object, as `pick` takes), as `group` (its number), or
with neither the group picked last; `merge_groups` also takes `into_x` and
`into_y`, or `into_group`; `show_all_faces` shows every hidden face and
`show_group_colours` (with `on`) switches the colours. `get_state` gives each
object's `groups` (how many, whether the colours show, and the group picked),
`pick` gives the `group` under a window position (0 for none), and
`set_brush` takes `stay_in_group`.

`deform` runs one of the Deform card's deformers on the selected objects:
`kind` (bend, bend_arc, twist, taper, flatten, inflate, stretch, lattice),
`axis` (x, y or z), and the card's values in the card's units: `angle` and
`direction` in degrees, `start_size`, `end_size`, `length`, `amount`,
`softness` and `range` (start and end) in percent, `tip` for Bend Arc and
`lattice_points` and `lattice_moves` for Lattice. It is applied as one undo
step, or with `preview: true` left on the model with the card up for the user
to apply or cancel.

And materials: `materials` lists the material library, the scene's palette
by slot and the `active_material` painting lays; `paint_material` picks that
by a library material's name (`material`, which joins the palette), by a
palette `slot`, or `"none"` for colour only.

And Polypaint: `set_brush` with `brush: "Paint"` (or `"Airbrush"`) and then
`stroke` paints, making a paint layer first if the object has none (the reply
says so). `command` `paint_settings` sets the Paint brush's `colour` (`[r, g,
b]`, 0 to 255, or `"#rrggbb"`), `mode` (Paint, Erase, Blur, Smear, Sample),
`opacity`, `channels` (`rgb`, `material`, `roughness`, `metallic`),
`roughness` and `metallic` (the values laid, in percent), `zadd`,
`zadd_brush`, `layer_mask` and `tolerance`, and answers with them all.
`paint_layer` works the Paint Layers card: `do` is `new` (with
`layer_name`), `delete`, `select`, `rename` (with `new_name`), `move` (with
`to`, 1 for the top),
`duplicate`, `merge_down`, `flatten`, `show`, `hide`, `lock`, `unlock`,
`opacity` (with `percent`), `blend` (with `blend`, a mode's name),
`material_blend` (with `blend`, normal or add), `channels`, `mask` (with
`mask`: none, paintable, sculpt_mask, sculpt_mask_inverted, cavity, or
face_group with `group`), `recompute_cavity`, `fill` (with `area`: all,
masked, unmasked, or group with `group`; and `fill_colour` and
`fill_material`, true or false) or `clear`, on the layer named by `layer` (its name or id) or the one
painted on; it answers with the layers. `pick_paint` (with `x` and `y`) is the
eyedropper, and `mask_by_colour`, `mask_by_material` and `mask_by_layer` are
the Masking card's. `get_state` gives each object's `paint_layers`, top
first, and the brush's `paint` settings.

And reference pictures (see Reference images): `add_picture` stands the
`.png` or `.jpg` at `path` (a full path, held to the same rule as
`open_file`; the bridge never opens a file box) in the scene, facing the axis
view the camera is in or the `facing` given, fitted to the scene;
`list_pictures` gives each with its settings; `edit_picture` changes those of
`picture` (its number in the list, 1 for the first, or its name); and
`delete_picture` takes it out of the scene, for good. `add_picture` and
`edit_picture` take `facing`, `size`, `across`, `up` and `depth` (in the
scene's units), `opacity` (percent), `flip`, `in_front` and `show`.

And UVs and maps (see UVs and maps): `command` `unwrap_uvs` unwraps the
active object (with `seams_on_groups`), and `bake_maps` bakes onto it into
`folder` (a full path, held to the same rule as `open_file`), with `from`
(an object's name), `size`, `reach` and `maps` (any of `normal`,
`displacement`, `ao` and `colour`). The bake runs in the background, as the
button's does; the status line says where the files went. `get_state` gives
each object's `uvs`.

There are two halves. The app listens on `127.0.0.1:47823` (`src/app/bridge.rs`;
this machine only, and only the first copy of the app if several are open).
`sculpt.exe --mcp` (`src/mcp.rs`) is the server Claude talks to, and passes
each tool call on to the app. `Sculpt-mcp.bat` starts that server from a copy
of the program in `target\mcp`, so a connected session never blocks a rebuild.
A new tool needs its description in `mcp.rs` and its code in `bridge.rs`.
Set `SCULPT_BRIDGE_PORT` to use another port.

**Who may connect** (`src/bridge_guard.rs`). "This machine only" is not enough
by itself: a web page open in a browser can make the browser send a request to
`127.0.0.1`, and the bridge used to run a line of JSON it found in one. So
every time the app starts it makes a new random key and leaves it in
`%APPDATA%\Sculpt\claude-bridge-47823.json` (the number is the port; the file
also holds the app's process number, and is removed when the app closes). The
server reads that file each time it connects and shows the key as its first
line. The app runs nothing for a connection until it has seen the key, and
closes one whose first line is anything else: a wrong key, a web request,
noise, a line over a kilobyte, or nothing at all for five seconds. After the
key, the first line that is not a tool call closes the connection too.

Connections that have shown the key and connections that have not are counted
apart, because any program on the machine can open connections and say
nothing, and with one count for both enough of them kept Claude out. Up to 64
sessions can be connected with the key; a 65th is told there is no room. Up
to 64 connections can be waiting to show a key, and when a 65th arrives the
one that has waited longest is closed to make room. A real server shows its
key the moment it connects, so it is never the one that has waited longest. If
it is ever turned out all the same it tries again by itself, and if it still
cannot get in it says that it "could not get a turn", not that the builds do
not match.

In use there is nothing to set up, and several Claude sessions can be
connected at once:

- Close the app and start it again, and connected sessions pick the new key up
  by themselves at their next call.
- **After rebuilding the app, restart any Claude session that was already
  open** (or reconnect its `sculpt` server with `/mcp`). Its server is the copy
  made when the session started, and one from before this key existed is
  turned down with a message saying so. The other way round, a new server on
  an app that has not been rebuilt and restarted, says to close the app and
  start it again. Neither waits or hangs.
- If the app crashes, its key file is left behind. That is harmless: the
  server goes by whether the port answers, not by the file, and says "Sculpt
  is not running" as before.
- A run started with an `--option` keeps no settings folder, so it has
  nowhere to leave a key and has no bridge. Give it one with
  `SCULPT_SETTINGS_DIR` (and a port of its own with `SCULPT_BRIDGE_PORT`) to
  drive a scripted run; set the same two for `sculpt.exe --mcp`.

**What the bridge will not do behind your back.**

- `quit` does not close the app over unsaved work. It answers that there is
  unsaved work and that the user is to be asked; then `save: true` saves to the
  scene's own `.sclp` first (never through a dialog: a scene with no file yet
  has to be saved with `save_file` first), and `discard: true` closes without
  saving. Nor does it close over work that is in no file (SSpheres not yet
  made into a skin, unmerged tubes, planned keys, a lathe outline: what the
  close button's own question names). Saving does not keep that work, so
  `save: true` is no answer to it: `quit` names the work and stays open, and
  only `discard: true` closes over it.
- `open_file` does not open a sculpt in place of a scene with unsaved work
  unless told `discard: true`, for the same reason: the interface asks, so
  Claude is told to. The same goes for unmerged tubes and planned keys, which
  stand on the scene's objects and would go with them: it names them and
  opens nothing. Adding an `.obj` or `.stl` needs no such answer.
- A sculpt saved through the bridge says what the Save button says,
  including that damaged points were written mended, if any were.
- `open_file`, `save_file` and `launch` take only full paths on this
  computer's own drives. A network share (`\\server\folder`, `//server/folder`,
  `\\?\UNC\...`), a drive letter that stands for one (`Z:\head.sclp`, with `Z:`
  mapped to a share), a device (`\\.\COM1`, or any of the names Windows keeps
  for devices, such as `NUL`, `COM0` to `COM9` and `LPT0` to `LPT9`, as any
  part of the path and with any ending) and a name with a hidden stream
  (`x.sclp:other`) are refused before the disk is looked at, since merely
  asking Windows about a share signs in to that server. So Claude cannot open
  or save a sculpt on a network drive; the app's own Open and Save still can.
  (A folder on a local drive that is really a link to a share is not seen for
  what it is.)
- `launch` starts the program itself with the file as one argument, never
  through a command line, so nothing in a file's name can be run. The name
  must be a full path to a file that is there. (The app is started by way of
  a copy of the program that exits at once, `sculpt.exe --start-app`, so it
  is not tied to the Claude session and stays open when the session ends.)

Left as they were: `save_file` replaces a file that is already there (a
`.sclp` or an export is written whole beside the old one and then swapped
in), and `delete_object`, like every other change, is one undo step.

Matcap images in `assets/matcaps` keep their original credits files. Icons in
`assets/icons` come from the Unity project's `Design/Icons`.
