// Release builds are a windowed app: no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod bench;
mod camera;
mod capture;
mod doc;
mod gizmo;
mod history;
mod io;
mod look;
mod mcp;
mod mesh;
mod prefs;
mod quadremesh;
mod recent;
mod remesh;
mod render;
mod scene;
mod sculpt;
mod ssphere;
#[cfg(test)]
mod tests;
mod timelapse;
mod turntable;
mod ui;
mod unity_import;
mod util;

use std::path::PathBuf;

/// A windowed app has no console of its own; borrow the launching terminal's
/// so command-line modes can print.
#[cfg(windows)]
fn attach_console() {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn AttachConsole(process: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    // SAFETY: plain Win32 call with no pointers; failure is harmless.
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

#[cfg(not(windows))]
fn attach_console() {}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--mcp") {
        // The server Claude talks to. It owns stdin and stdout, so this comes
        // before anything that could touch a console. `--app` names the
        // program its launch tool starts.
        let app = args.iter().position(|a| a == "--app").and_then(|i| args.get(i + 1)).map(PathBuf::from);
        mcp::run(app);
        return;
    }
    if !args.is_empty() {
        attach_console();
    }
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("warn")).init();

    if let Some(i) = args.iter().position(|a| a == "--bench") {
        // Optional triangle counts after the flag, e.g. `--bench 1000000`.
        let mut sizes: Vec<usize> = args[i + 1..].iter().filter_map(|a| a.parse().ok()).collect();
        if sizes.is_empty() {
            sizes = vec![200_000, 1_000_000, 4_000_000];
        }
        bench::run(&sizes);
        return;
    }

    let mut opts = app::Args::default();
    let mut it = args.iter();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--screenshot" => opts.screenshot = it.next().map(PathBuf::from),
            "--size" => {
                opts.size = it.next().and_then(|s| {
                    let (w, h) = s.split_once('x')?;
                    Some((w.parse().ok()?, h.parse().ok()?))
                });
            }
            "--demo" => opts.demo = true,
            "--demo-scene" => opts.demo_scene = true,
            // The demo scene with three of its objects selected.
            "--demo-select" => (opts.demo_scene, opts.demo_select) = (true, true),
            "--demo-gizmo" => opts.demo_gizmo = true,
            // A hosted tool brought up for a visual check: measure | measure:inches | studs:job ...
            "--demo-tool" => opts.demo_tool = it.next().cloned(),
            // The blue selection flash, held at half strength on the active object.
            "--demo-flash" => opts.demo_flash = true,
            // The sphere off the middle with its live mirror on and strokes on the copy.
            "--demo-mirror" => opts.demo_mirror = true,
            // Half of the active object's faces hidden, cut on a slant.
            "--demo-hide" => opts.demo_hide = true,
            // A long shape pulled out of a coarse ball, then refined by dynamic topology; wireframe on.
            "--demo-dyntopo" => opts.demo_dyntopo = true,
            // rig | preview | skin | back | again | play
            "--demo-spheres" => opts.demo_spheres = it.next().cloned(),
            // A figure posed with the Pose brush: hover | bend | chain | twist | resize | slide | stretch | done | wheel | play
            "--demo-pose" => opts.demo_pose = it.next().cloned(),
            // Saving, opening and the questions about unsaved work: recent | ask-exit | ask-open | ask-open-clean | play
            "--demo-files" => opts.demo_files = it.next().cloned(),
            "--flat" => opts.flat = true,
            // The clean view (Tab) from the start, with its hint.
            "--clean-view" => opts.clean_view = true,
            // Record a short 360 loop, then close: 12 pictures at 640x360 and 30 fps unless told otherwise.
            // A folder gets numbered pictures, a name ending in .mp4 gets a video.
            "--record-loop" => opts.record_loop = it.next().map(PathBuf::from),
            "--loop-size" => opts.loop_size = it.next().and_then(|s| s.split_once('x')).and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))),
            "--loop-frames" => opts.loop_frames = it.next().and_then(|s| s.parse().ok()),
            "--loop-fps" => opts.loop_fps = it.next().and_then(|s| s.parse().ok()),
            "--loop-cancel" => opts.loop_cancel = it.next().and_then(|s| s.parse().ok()),
            // Sculpt a few scripted strokes while recording a timelapse to this file, then close.
            "--demo-timelapse" => opts.demo_timelapse = it.next().map(PathBuf::from),
            // Shading settings for visual checks, e.g. "matcap=off,finish=metal:rust".
            "--look" => opts.look = it.next().cloned(),
            // Look at the whole scene from a yaw and pitch in degrees, e.g. "0,0" for the front.
            "--view" => opts.view = it.next().and_then(|s| s.split_once(',')).and_then(|(y, p)| Some((y.parse().ok()?, p.parse().ok()?))),
            "--test-keep-pointer" => opts.keep_pointer = true,
            "--remesh" => opts.remesh = it.next().and_then(|s| s.parse().ok()),
            "--quad-remesh" => opts.quad_remesh = it.next().and_then(|s| s.parse().ok()),
            other if !other.starts_with("--") => opts.open = Some(PathBuf::from(other)),
            other => eprintln!("unknown option {other}"),
        }
    }

    if let Err(e) = app::run(opts) {
        eprintln!("{e}");
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("Sculpt")
            .set_description(e)
            .show();
        std::process::exit(1);
    }
}
