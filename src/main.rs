// Release builds are a windowed app: no console window behind it.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod backups;
mod bench;
mod bench_levels;
mod bench_matched;
mod bench_render;
mod brush_hash;
mod bridge_guard;
mod brush_files;
mod camera;
mod capture;
mod decimate;
mod deform;
mod doc;
mod gizmo;
mod history;
mod io;
mod look;
mod materials;
mod mcp;
mod mesh;
mod prefs;
mod printable;
mod quadremesh;
mod recent;
mod recovery;
mod remesh;
mod render;
mod scene;
mod sculpt;
mod ssphere;
mod still;
#[cfg(test)]
mod tests;
mod timelapse;
mod turntable;
mod ui;
mod unity_import;
mod util;

use std::path::PathBuf;

// A panic has to unwind, not abort. `[profile.release]` in Cargo.toml says
// what stands on that; this stops a build that would quietly switch it off.
#[cfg(panic = "abort")]
compile_error!(
    "build with panic = \"unwind\": a panic in a tool's job has to come back as an error, \
     and one on the interface thread has to reach the recovery copy (see Cargo.toml)"
);

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

/// What a run is for when it is not the window, going by its arguments.
#[derive(Debug, PartialEq)]
enum Errand {
    /// `--start-app`, which must come first: start the app named next, and go.
    StartApp,
    /// `--mcp`: the server Claude talks to.
    Server,
    /// `--bench`, with what follows it: triangle counts, e.g. `--bench 1000000`.
    Bench(Vec<usize>),
    /// `--bench-matched`, with what follows it: icosphere subdivisions.
    BenchMatched(Vec<usize>),
    /// `--bench-volume`: remesh and boolean costs, on the source the Unity engine benchmarks use.
    BenchVolume,
    /// `--bench-render`, with triangle counts: the viewport's drawing (see bench_render.rs).
    BenchRender(Vec<usize>),
    /// `--bench-render-ab <other exe> [rounds] [triangles...]`: that bench, this build against another.
    BenchRenderAb(Vec<String>),
    /// `--render-shots <folder>`: a fixed set of viewport pictures, for comparing two builds.
    RenderShots(PathBuf),
    /// `--compare-shots <folder> <folder>`: two sets of those pictures, pixel by pixel.
    CompareShots(PathBuf, PathBuf),
    /// `--brush-hash`, with the main ball's triangle count if given: scripted
    /// strokes and a hash of the mesh after each, to show a speed change
    /// left every result as it was (see brush_hash.rs).
    BrushHash(Vec<usize>),
    /// `--bench-levels`, with what follows it: the finest level's triangles, e.g. `--bench-levels 1000000`.
    BenchLevels(Vec<usize>),
    /// `--bench-deform`, with what follows it: triangle counts. One preview frame of each gizmo deformer.
    BenchDeform(Vec<usize>),
}

/// The errand the arguments ask for, if any; `None` is the app itself, with
/// its window. An errand is always asked for by an option, one of its own
/// arguments spelt exactly, so a file handed to the app to open is never
/// taken for one whatever it is called (see `mcp::launch_file`).
fn errand(args: &[String]) -> Option<Errand> {
    let after = |option: &str| args.iter().position(|a| a == option).map(|i| &args[i + 1..]);
    let numbers = |rest: &[String]| rest.iter().filter_map(|a| a.parse().ok()).collect::<Vec<usize>>();
    if args.first().is_some_and(|a| a == mcp::START_APP) {
        Some(Errand::StartApp)
    } else if after("--mcp").is_some() {
        Some(Errand::Server)
    } else if let Some(rest) = after("--bench") {
        Some(Errand::Bench(numbers(rest)))
    } else if let Some(rest) = after("--bench-matched") {
        Some(Errand::BenchMatched(numbers(rest)))
    } else if let Some(rest) = after("--bench-render") {
        Some(Errand::BenchRender(numbers(rest)))
    } else if let Some(rest) = after("--bench-render-ab") {
        Some(Errand::BenchRenderAb(rest.to_vec()))
    } else if let Some(rest) = after("--render-shots") {
        Some(Errand::RenderShots(rest.first().map(PathBuf::from).unwrap_or_else(|| PathBuf::from("shots"))))
    } else if let Some([a, b, ..]) = after("--compare-shots") {
        Some(Errand::CompareShots(PathBuf::from(a), PathBuf::from(b)))
    } else if let Some(rest) = after("--brush-hash") {
        Some(Errand::BrushHash(numbers(rest)))
    } else if let Some(rest) = after("--bench-levels") {
        Some(Errand::BenchLevels(numbers(rest)))
    } else if let Some(rest) = after("--bench-deform") {
        Some(Errand::BenchDeform(numbers(rest)))
    } else {
        after("--bench-volume").map(|_| Errand::BenchVolume)
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let errand = errand(&args);
    match errand {
        Some(Errand::StartApp) => return mcp::start_and_leave(&args[1..]),
        Some(Errand::Server) => {
            // It owns stdin and stdout, so this comes before anything that
            // could touch a console. `--app` names the program its launch
            // tool starts.
            let app = args.iter().position(|a| a == "--app").and_then(|i| args.get(i + 1)).map(PathBuf::from);
            return mcp::run(app);
        }
        _ => {}
    }
    if !args.is_empty() {
        attach_console();
    }
    // The log goes beside the settings, so a scripted run (any `--option`)
    // writes none unless it was given a settings folder of its own.
    let home = prefs::Home::for_run(args.iter().any(|a| a == "--screenshot"));
    recovery::start_log(home.folder().filter(|_| home.writable()));
    recovery::Logger::install();

    let or = |given: Vec<usize>, usual: &[usize]| if given.is_empty() { usual.to_vec() } else { given };
    match errand {
        Some(Errand::Bench(sizes)) => return bench::run(&or(sizes, &[200_000, 1_000_000, 4_000_000])),
        // 7 subdivisions (327k triangles) and 8 (1.3M) unless told otherwise.
        Some(Errand::BenchMatched(subdivisions)) => return bench_matched::run(&or(subdivisions, &[7, 8])),
        Some(Errand::BenchVolume) => return bench_matched::run_volume(),
        Some(Errand::BenchRender(sizes)) => {
            return bench_render::run(&or(sizes, &[1_000_000, 5_000_000, 10_000_000]));
        }
        Some(Errand::BenchRenderAb(args)) => return bench_render::run_ab(&args),
        Some(Errand::RenderShots(folder)) => return bench_render::run_shots(&folder),
        Some(Errand::CompareShots(a, b)) => return bench_render::compare(&a, &b),
        Some(Errand::BrushHash(tris)) => return brush_hash::run(&tris),
        Some(Errand::BenchLevels(sizes)) => return bench_levels::run(&or(sizes, &[1_000_000, 5_000_000, 10_000_000])),
        Some(Errand::BenchDeform(sizes)) => return deform::bench::run(&or(sizes, &[1_000_000, 5_000_000, 10_000_000])),
        _ => {}
    }

    if let Err(e) = app::run(window_options(&args)) {
        eprintln!("{e}");
        recovery::log(&format!("The app could not start: {e}"));
        rfd::MessageDialog::new()
            .set_level(rfd::MessageLevel::Error)
            .set_title("GolemSculpt")
            .set_description(e)
            .show();
        std::process::exit(1);
    }
}

/// What the arguments ask of the app and its window. An argument that is not
/// an option (`prefs::is_option`) is the file to open.
fn window_options(args: &[String]) -> app::Args {
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
            // Two objects, each with a symmetry of its own and a stroke under it.
            "--demo-symmetry" => opts.demo_symmetry = true,
            // Half of the active object's faces hidden, cut on a slant.
            "--demo-hide" => opts.demo_hide = true,
            // A long shape pulled out of a coarse ball, then refined by dynamic topology; wireframe on.
            "--demo-dyntopo" => opts.demo_dyntopo = true,
            // rig | preview | skin | back | again | play
            "--demo-spheres" => opts.demo_spheres = it.next().cloned(),
            // A figure posed with the Pose brush: hover | bend | chain | twist | resize | slide | stretch | done | wheel | play
            "--demo-pose" => opts.demo_pose = it.next().cloned(),
            // Saving, opening and the questions about unsaved work: recent | ask-exit | ask-open | ask-open-clean | play | open-undo
            "--demo-files" => opts.demo_files = it.next().cloned(),
            // A picture imported as a matcap and shaded with, without the file box: any size.
            "--demo-matcap" => opts.demo_matcap = it.next().map(PathBuf::from),
            // The printing check on a damaged object: check | closed | export | remesh | quads | damaged | play
            "--demo-print" => opts.demo_print = it.next().cloned(),
            // The interface's own furniture, set up for a look or worked and checked:
            // wheel | wheel=low | brush=<name> | mask | strength=<0..1> | size=<1.5> | fold=left|right|both |
            // float=<card>@<x>:<y> | play, with commas between.
            "--demo-ui" => opts.demo_ui = it.next().cloned(),
            // The Blender link, worked without Blender (play) or with one that has no window (round).
            "--demo-blender" => opts.demo_blender = it.next().cloned(),
            // Subdivision levels: play (worked and checked), or a picture:
            // card | base | fine | down | bent | up | quads (see app/levels.rs).
            "--demo-levels" => opts.demo_levels = it.next().cloned(),
            // play | card | full | half | off | under | under-off | dyntopo | dyntopo-off | levels | levels-off |
            // levels-low (see app/layers.rs).
            "--demo-layers" => opts.demo_layers = it.next().cloned(),
            // Face groups: play (worked and checked), or a picture (see app/groups/play.rs).
            "--demo-groups" => opts.demo_groups = it.next().cloned(),
            // bend | arc | twist | taper | flatten | inflate | stretch | lattice | play
            "--demo-deform" => opts.demo_deform = it.next().cloned(),
            // A sphere painted with materials and colour: lit | matcap | no-materials | no-polypaint | card |
            // custom | broken; the Paint brush: play | brush | layers (see app/paint_demo.rs, app/paint_play.rs).
            "--demo-paint" => opts.demo_paint = it.next().cloned(),
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
            // Render Picture to this PNG, then close; the size (times the window's), samples and backdrop as given.
            "--render-picture" => opts.render_picture = it.next().map(PathBuf::from),
            "--render-scale" => opts.render_scale = it.next().and_then(|s| s.parse().ok()),
            "--render-samples" => opts.render_samples = it.next().and_then(|s| s.parse().ok()),
            "--render-transparent" => opts.render_transparent = true,
            // Shading settings for visual checks, e.g. "matcap=off,finish=metal:rust".
            "--look" => opts.look = it.next().cloned(),
            // Look at the whole scene from a yaw and pitch in degrees, e.g. "0,0" for the front.
            "--view" => opts.view = it.next().and_then(|s| s.split_once(',')).and_then(|(y, p)| Some((y.parse().ok()?, p.parse().ok()?))),
            "--test-keep-pointer" => opts.keep_pointer = true,
            "--remesh" => opts.remesh = it.next().and_then(|s| s.parse().ok()),
            "--quad-remesh" => opts.quad_remesh = it.next().and_then(|s| s.parse().ok()),
            // Decimate to this many triangles after that, by the routine the Decimate button runs.
            "--decimate" => opts.decimate = it.next().and_then(|s| s.parse().ok()),
            // Work Decimate through its button, question, Cancel and Undo; prints a line per check.
            "--demo-decimate" => opts.demo_decimate = true,
            other if !prefs::is_option(other) => opts.open = Some(PathBuf::from(other)),
            other => eprintln!("unknown option {other}"),
        }
    }
    opts
}
