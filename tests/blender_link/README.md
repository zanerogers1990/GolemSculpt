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
