# Checking the Blender link

The link is `src/app/blender.rs` (the app), `src/io/link.rs` (the package
both sides write) and `assets/blender/golemsculpt_link.py` (Blender). The
unit tests cover the app's side (`cargo test --release -- link blender`).
What is here checks the two programs against each other.

Everything below works in folders of its own. Nothing touches the user's
settings, their Blender settings, or a Blender they have open. `W` is a build
folder (`target/w/<name>`), and Blender is wherever it is installed.

```
export W=D:/Sculptingapp2/target/w/blender
export BLENDER="D:/SteamLibrary/steamapps/common/Blender/blender.exe"
export SCULPT_SETTINGS_DIR=$W/settings SCULPT_BRIDGE_PORT=47951 TMP=$W/tmp TEMP=$W/tmp
```

## Blender's side alone

No window. Writes packages as the app would, takes them in, sends them back,
and checks sizes, axes, facing, the marks, modifiers, units and the tray.

```
GOLEMSCULPT_LINK_DIR=$W/link "$BLENDER" --background --factory-startup --python tests/blender_link/check_link.py
```

## The app's side alone

The app stands in for Blender. Prints a line per check.

```
$W/release/sculpt.exe --demo-blender play --screenshot $W/play.png
```

## Both, with no Blender window

The app sends the demo scene and waits; a Blender with no window takes it,
widens the first object, makes a new one, and sends everything back
(`reply.py`). Start the app first, then Blender, on the same folder. Add
`--remesh 640` in place of `--demo-scene` for one object of 3.9 million
triangles; both sides print how long each step took.

```
export GOLEMSCULPT_LINK_DIR=$SCULPT_SETTINGS_DIR/scratch/sculpt_blender_demo
$W/release/sculpt.exe --demo-scene --demo-blender round --screenshot $W/round.png &
"$BLENDER" --background --factory-startup --python tests/blender_link/reply.py
```

## The whole road, window and all

The app looks for Blender where it is installed and starts it as the button
does. **A Blender window opens on the screen for some seconds.** It runs with
a scripts folder and a settings folder of the run's own, in which
`answer_and_quit.py` stands in for the person: it waits for the objects,
presses Send to GolemSculpt, and quits. What it saw is in `answer.log` in the
link's folder.

```
export GOLEMSCULPT_LINK_DIR=$SCULPT_SETTINGS_DIR/scratch/sculpt_blender_demo
export BLENDER_USER_SCRIPTS=$W/userscripts BLENDER_USER_CONFIG=$W/userconfig
mkdir -p $BLENDER_USER_SCRIPTS/startup $BLENDER_USER_CONFIG
cp tests/blender_link/answer_and_quit.py $BLENDER_USER_SCRIPTS/startup/zz_answer_and_quit.py
$W/release/sculpt.exe --demo-scene --demo-blender live --screenshot $W/live.png
cat $GOLEMSCULPT_LINK_DIR/answer.log
```

## Paint

No window. The app's own code writes a package with paint and says what it
holds (`expect.json`); Blender's side reads it, checks every byte of every
colour and weight, takes it into a scene, checks the attributes it made,
sends it back unchanged (it says it is the same), paints ten points yellow,
sends it again, and leaves that answer; the app's code takes the answer in
as the app does (its layers carried, a "From Blender" layer on the ten
points). With `OLD_LINK_SCRIPT`, the script from before paint reads the
package too (its shapes whole, the paint passed over), and an old package
reads with the new one.

```
export GOLEMSCULPT_PAINT_CHECK=$W/paintcheck GOLEMSCULPT_LINK_DIR=$W/link
git show 5968b86:assets/blender/golemsculpt_link.py > $W/old_link.py
export OLD_LINK_SCRIPT=$W/old_link.py
cargo test --offline --target-dir $W -- --ignored blender_paint_check_package
"$BLENDER" --background --factory-startup --python tests/blender_link/check_paint.py
cargo test --offline --target-dir $W -- --ignored blender_paint_check_answer
```

The exports, read by Blender's own importers: an FBX of a painted ball and
its mirror copy (the colour set, each face's material, the weights in the UV
sets), and an OBJ and a PLY (each point's colour).

```
cargo test --offline --target-dir $W -- --ignored blender_fbx_check_file blender_obj_ply_check_files
"$BLENDER" --background --factory-startup --python tests/blender_link/check_fbx_paint.py
"$BLENDER" --background --factory-startup --python tests/blender_link/check_obj_ply_paint.py
```
