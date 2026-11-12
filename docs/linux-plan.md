# GolemSculpt on Linux

Written 2026-11-02. Nothing built yet; this is the plan.

## Where it stands

- The whole workspace type-checks for Linux (`cargo check --workspace --all-targets
  --target x86_64-unknown-linux-gnu --target-dir target/w/linux-check`): no errors,
  only "never used" warnings for code that only Windows calls.
- Every Windows-only call already has a stand-in for other systems (`#[cfg(not(windows))]`),
  so nothing has to be torn out. The work is making the stand-ins as good as the Windows side.
- Already fine on Linux: the settings folder (`~/.config/sculpt`), the interface language
  (from `LANG`), the Claude bridge's random key, file dialogs (rfd through the desktop portal).
- Nothing here can be run yet: this PC has no WSL and no Linux machine is set up.
  The program cannot even be linked for Linux on Windows without one.

## What is missing or worse on Linux

| What | Today on Linux | Fix |
|---|---|---|
| Pen pressure | none: winit 0.30 reports no pressure on X11 or Wayland | read the pen ourselves: XInput2 on X11, the tablet protocol on Wayland. The biggest job |
| Interface font | Segoe UI is read from `C:\Windows\Fonts`, so egui's own font shows instead; Japanese/Chinese/Korean tooltips have no letters | ship an open font (OFL) with the app, or find Noto fonts under `/usr/share/fonts` |
| Recording video / time-lapse MP4 | "needs Windows" (Media Foundation) | hand the frames to `ffmpeg` when it is installed; say so plainly when it is not |
| Times in file names and Older Versions | UTC, not local time | ask the C library for local time (`localtime_r`; `libc` is already in the build through winit) |
| Undo memory cap | always the 2 GB floor | read the total from `/proc/meminfo` |
| Send to Blender | cannot tell whether Blender is running; looks only in Windows places | look through `/proc`; look in `/usr/bin`, Flatpak, Snap and `~/.steam/steam/steamapps` |
| Claude bridge | Windows path rules only; `launch` does not detach the app | Unix path rules; start the app in its own process group |
| Native message boxes (crash recovery) | need `zenity` installed | check what rfd does without it; fall back to the log |
| Pen gestures switch-off, console attach | not needed on Linux | nothing |
| Launchers | `Sculpt.bat`, `Sculpt-mcp.bat` | `sculpt.sh`, a `.desktop` file and an icon |
| Tests | some use `C:\` paths | mark those Windows-only, add Linux versions where they matter |

## Building and testing without a Linux PC

1. **GitHub Actions** builds and tests the Linux version on every push (Ubuntu 22.04, so it
   runs on distributions back to that age) and keeps the `.tar.gz` for itch.io. Free for this
   repository; needs no software on this PC.
2. **WSL2 on this PC** (Ubuntu, with its built-in Linux windows) to open the app and look at it.
   Zane has to install it (`wsl --install` as administrator, then a restart). It runs the
   graphics through the 4090 but has no pen, so it checks everything except pressure.
3. **A real Linux machine or a Linux USB stick** for pen pressure on the Huion, only once that
   phase is reached.

## Order

1. **Mouse-only Linux build** (about 2-3 days): font, local time, memory cap, Blender, bridge,
   message boxes, tests, launchers, GitHub Actions build, `GolemSculpt-linux.tar.gz`.
2. **Video on Linux** through ffmpeg (about a day).
3. **Pen pressure on Linux** (1-2 weeks): X11 first, then Wayland.

Each phase is built in its own worktree and changes nothing on Windows; the Windows tests
run before every merge.
