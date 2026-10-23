//! The running application: window, input routing and the frame loop.
//!
//! Frames are drawn on demand. Pointer events between frames are queued and
//! every one of them is fed to the stroke, so a slow frame never loses part
//! of the path or its pen pressure.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use egui::Color32;
use glam::{Vec2, Vec3};
use winit::application::ApplicationHandler;
use winit::event::{ElementState, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowId};

use crate::camera::{self, View};
use crate::doc::{Action, Doc, MaskOp, Options, Stats, Tool};
use crate::history::Next;
use crate::mesh::Mesh;
use crate::mesh::primitives::Primitive;
use crate::render::flash;
use crate::render::{DrawItem, Globals, Gpu, Renderer};
use crate::scene::{Object, Scene};
use crate::sculpt::{Brush, PointerSample, Sculptor};
use crate::ui::{self, Cursor, UiState, theme};
use crate::io;

mod alpha;
mod blender;
mod bridge;
mod brush_keys;
mod brushes;
mod cut_mirror;
mod decimate;
mod deform;
mod drop;
mod dyntopo;
mod echo;
mod escape;
mod export_fbx;
mod exports;
mod recording;
mod faces;
mod file_arrays;
mod file_undo;
mod feel_demo;
mod files;
mod focus;
mod groups;
mod hover;
mod layers;
mod levels;
mod live_copies;
mod mask_shape;
mod materials;
mod paint;
mod paint_demo;
mod paint_play;
mod masked;
mod mirror_copies;
mod new_scene;
mod older;
mod origin;
mod pen;
mod pose;
mod print_check;
mod recovery;
mod remesh_job;
mod selection;
mod spheres;
mod still;
mod symmetry;
pub(crate) mod symmetry_repair;
mod title;
pub(crate) mod tools;
mod transform;
mod turn;
mod undo;
mod undo_names;
mod ui_demo;
mod unity;
mod unsaved;
mod view_moves;
mod visibility;

/// Two clicks (or F twice) this close together are a double: Windows' own
/// starting double-click time, and room for a pen tip, which lands a little
/// apart each time. A drag between them breaks the pair (`pointer_released`,
/// `key_released`), so quick orbits and F + drags one after another do not
/// frame the view.
const DOUBLE_CLICK: Duration = Duration::from_millis(500);
const DOUBLE_CLICK_PX: f32 = 20.0;

#[derive(Default, Clone, Debug, PartialEq)]
pub struct Args {
    /// Render one frame to this PNG and exit.
    pub screenshot: Option<PathBuf>,
    pub size: Option<(u32, u32)>,
    /// Sculpt a scripted set of strokes first (for visual checks).
    pub demo: bool,
    /// Voxel-remesh at this resolution after the demo strokes.
    pub remesh: Option<u32>,
    /// Quad-remesh to about this many quads after that.
    pub quad_remesh: Option<u32>,
    /// Decimate to this many triangles after that, and wait for it.
    pub decimate: Option<u32>,
    /// Work Decimate through its button, its question, Cancel and Undo, and
    /// print a line per check (see app/decimate.rs).
    pub demo_decimate: bool,
    pub flat: bool,
    /// Shading settings to start with (see `look::apply_spec`).
    pub look: Option<String>,
    /// Fill the scene with one of each shape, spread out, plus an instance
    /// (for visual checks of the object list).
    pub demo_scene: bool,
    /// With `demo_scene`: select three of its objects (for visual checks of
    /// a selection of several).
    pub demo_select: bool,
    /// Build a small SSphere figure and show it as `rig`, `preview` or `skin`
    /// (for visual checks of the SSphere tool).
    pub demo_spheres: Option<String>,
    /// Turn and stretch the object and bring up the transform gizmo (for
    /// visual checks).
    pub demo_gizmo: bool,
    /// Bring a hosted tool up in a state worth looking at: `id` or `id:what`
    /// (for visual checks; see app/tools.rs).
    pub demo_tool: Option<String>,
    /// Hold the blue selection flash on the active object at half strength
    /// (for visual checks).
    pub demo_flash: bool,
    /// Move the sphere off the middle, turn its live mirror on and sculpt on
    /// the copy (for visual checks; see app/mirror_copies.rs).
    pub demo_mirror: bool,
    /// Two objects with a symmetry each, selected in turn and sculpted (for
    /// visual checks of symmetry belonging to objects; see app/symmetry.rs).
    pub demo_symmetry: bool,
    /// Hide half of the active object's faces after the demo strokes (for
    /// visual checks of hidden faces).
    pub demo_hide: bool,
    /// Pull a long shape out of a coarse ball and work along it with dynamic
    /// topology on, wireframe showing (for visual checks; see app/dyntopo.rs).
    pub demo_dyntopo: bool,
    /// Stand a figure in the scene and pose it: `hover`, `bend`, `chain`...
    /// (for visual checks of the Pose brush; see app/pose.rs).
    pub demo_pose: Option<String>,
    /// A brush drawn at three strengths and with a pen taper, on a ball or a
    /// thin slab (for comparing how brushes feel; see app/feel_demo.rs).
    pub demo_feel: Option<String>,
    /// Testing aid: keep treating the pointer as inside the window after the
    /// OS says it left, so input posted to the window by a script (while the
    /// real cursor is elsewhere) still gets hover behaviour.
    pub keep_pointer: bool,
    /// Start in the clean view (for visual checks of it and its hint).
    pub clean_view: bool,
    /// Record a 360 loop into this folder, then close (see app/recording.rs).
    pub record_loop: Option<PathBuf>,
    /// With `record_loop`: the picture size and how many pictures.
    pub loop_size: Option<(u32, u32)>,
    pub loop_frames: Option<u32>,
    /// With `record_loop`: frames a second, and stop (cancel, or close the
    /// app) once this many pictures are in.
    pub loop_fps: Option<u32>,
    pub loop_cancel: Option<u32>,
    /// Record a timelapse of a few scripted strokes to this file, then close.
    pub demo_timelapse: Option<PathBuf>,
    /// Render Picture to this PNG, then close (see app/still.rs); with
    /// `render_scale` times the window's size, `render_samples` samples,
    /// `render_transparent` no backdrop and `render_no_rays` without ray
    /// tracing.
    pub render_picture: Option<PathBuf>,
    pub render_scale: Option<u32>,
    pub render_samples: Option<u32>,
    pub render_transparent: bool,
    pub render_no_rays: bool,
    /// Import this picture as a matcap and shade with it, by the routine
    /// under Import Matcap (for visual checks; no file box is opened).
    pub demo_matcap: Option<PathBuf>,
    /// Save and open scripted scenes, or put up a question about unsaved
    /// work (for visual checks; see app/files.rs).
    pub demo_files: Option<String>,
    /// Damage the object so it will not print and run the printing check on
    /// it: `check`, `closed`, `export`, `remesh`, `quads`, `damaged` or
    /// `play` (for visual checks; see app/print_check.rs).
    pub demo_print: Option<String>,
    /// Set the interface up for a look, or work it and print a line per check:
    /// a list such as `wheel,brush=Clay` or `play` (see app/ui_demo.rs).
    pub demo_ui: Option<String>,
    /// Work the Blender link without Blender, or with one that has no window:
    /// `play` or `round` (see app/blender.rs).
    pub demo_blender: Option<String>,
    /// Subdivision levels: `play` works them and prints a line per check;
    /// the other words set up a picture (see app/levels.rs).
    pub demo_levels: Option<String>,
    /// Sculpt layers: `play` works them and prints a line per check; the
    /// other words set up a picture (see app/layers.rs).
    pub demo_layers: Option<String>,
    /// Face groups: `play` works them and prints a line per check; the other
    /// words set up a picture (see app/groups/play.rs).
    pub demo_groups: Option<String>,
    /// The gizmo's deformers: `play` works them and prints a line per check;
    /// a deformer's name sets up a picture of it (see app/deform/play.rs).
    pub demo_deform: Option<String>,
    /// A sphere painted with materials and colour: `lit`, `matcap`,
    /// `no-materials`, `no-polypaint`, `card`, `custom`, `broken` (for
    /// visual checks; see app/paint_demo.rs).
    pub demo_paint: Option<String>,
    /// Look at the whole scene from this yaw and pitch, in degrees, whatever
    /// view a file that was opened had (for visual checks): 0,0 is the front.
    pub view: Option<(f32, f32)>,
    pub open: Option<PathBuf>,
}

impl Args {
    /// A run that records by itself and then closes. Like a screenshot run,
    /// it must not take the keyboard from whoever is at the machine, nor the
    /// Claude bridge's port from the real app.
    fn records(&self) -> bool {
        self.record_loop.is_some()
            || self.demo_timelapse.is_some()
            || self.render_picture.is_some()
            || ui_demo::plays(self.demo_ui.as_deref())
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Drag {
    #[default]
    None,
    Sculpt,
    Orbit,
    Pan,
    Zoom,
    /// Press landed on the interface; the viewport ignores it.
    Ui,
    /// The SSphere tool has the pointer.
    Spheres,
    /// A handle of the transform gizmo.
    Gizmo,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum GaugeKind {
    Size,
    Strength,
    Focal,
}

/// A hold-key-and-drag adjustment. The ring freezes where the key went down
/// so it grows and shrinks in place.
#[derive(Clone, Copy)]
struct Gauge {
    kind: GaugeKind,
    anchor: Vec2,
    start: f32,
}

#[derive(Default)]
struct Input {
    pointer: Vec2,
    inside: bool,
    mods: ModifiersState,
    drag: Drag,
    button: Option<MouseButton>,
    samples: Vec<PointerSample>,
    pen_down: bool,
    pen_pressure: Option<f32>,
    /// The touch that has the pointer, from when it lands to when it lifts:
    /// the pen's tip. What any other touch does meanwhile is not the
    /// stroke's business (app/pen.rs).
    pen_touch: Option<u64>,
    last_click: Option<(Instant, Vec2)>,
    /// Where the press under way began, if that was on an interface
    /// control, and the last tap on one: a press on the view straight after
    /// a tap, in the same place, is its echo and not a stroke (app/echo.rs).
    on_control: Option<Vec2>,
    ui_tap: Option<echo::Tap>,
    last_f: Option<Instant>,
    gauge: Option<Gauge>,
    hover: Option<Vec3>,
    /// What `hover` was last worked out from (app/hover.rs).
    hover_last: hover::Last,
    /// Keys that went down and have not come up, so a lost focus can let go of them (app/focus.rs).
    held: Vec<KeyCode>,
    /// The point on the model the turn of the view under way goes about (app/turn.rs).
    turn_about: Option<Vec3>,
}

struct Running {
    window: Arc<Window>,
    gpu: Gpu,
    renderer: Renderer,
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,
    ui: UiState,
    doc: Doc,
    input: Input,
    /// The camera's view of the world.
    view: View,
    /// The same view in the active object's own space: what the mesh and the
    /// brushes work in.
    local: View,
    view_depth: f32,
    scene_size: f32,
    start: Instant,
    last_frame: Instant,
    redraw: bool,
    repaint_at: Option<Instant>,
    frames: u32,
    pivot_seen: Option<Instant>,
    stroke_ended: Option<Instant>,
    /// How closed the mesh was before a stroke that can add triangles, so
    /// the stroke can be held to leaving it no worse (app/print_check.rs).
    print_watch: print_check::StrokeWatch,
    /// A Decimate at work in the background (see app/decimate.rs).
    decimate: decimate::State,
    /// A Remesh at work in the background (see app/remesh_job.rs).
    remesh: remesh_job::State,
    /// A screenshot the Claude bridge is waiting for.
    capture: Option<bridge::Capture>,
    transform: transform::State,
    /// The blue and white pulses over picked and undone objects.
    flash: flash::State,
    /// The turntable, the 360 loop and the timelapse (see app/recording.rs).
    recording: recording::State,
    /// A picture being rendered (app/still.rs).
    still: Option<still::StillRun>,
    /// Settings, recent files and whether the scene has unsaved work.
    files: files::State,
    /// The copies kept against a crash (see app/recovery.rs).
    recovery: recovery::State,
    /// Earlier saves, and the objects deleted since the last save (see app/older.rs).
    older: older::State,
    /// The interface's self-check, while one runs (see app/ui_demo.rs).
    ui_demo: ui_demo::State,
    /// What has gone to Blender and come back (see app/blender.rs).
    blender: blender::State,
    args: Args,
}

pub struct App {
    args: Args,
    running: Option<Running>,
    error: Option<String>,
    bridge: Option<bridge::Bridge>,
    /// The watch on what Blender sends (see app/blender.rs).
    blender: Option<blender::Watch>,
}

pub fn run(args: Args) -> Result<(), String> {
    let event_loop = EventLoop::new().map_err(|e| format!("Could not start the event loop: {e}"))?;
    // A one-frame screenshot run would only take the port from the real app.
    let scripted = args.screenshot.is_some() || args.records() || recovery::checking();
    // The bridge's key is kept in the settings folder: a run with none has no bridge.
    let home = crate::prefs::Home::for_run(false);
    let folder = home.folder().filter(|_| home.writable());
    let bridge = if scripted { None } else { bridge::start(event_loop.create_proxy(), folder) };
    // Nor does a scripted run take what Blender sends the real app.
    let blender = if scripted { None } else { blender::watch(event_loop.create_proxy(), folder) };
    let mut app = App { args, running: None, error: None, bridge, blender };
    event_loop.run_app(&mut app).map_err(|e| e.to_string())?;
    app.error.map_or(Ok(()), Err)
}

fn srgb_to_linear(c: [f32; 3]) -> [f32; 3] {
    c.map(|v| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) })
}

impl App {
    /// Runs one call of the event loop's. A panic anywhere under it (a bug
    /// in a brush, a tool, the interface) unwinds to here instead of ending
    /// the program where it stood; by then nothing is still at work on the
    /// scene, so what can be kept of it is written to a recovery copy before
    /// the app closes. Everything the app does on the interface thread runs
    /// under one of these.
    fn guarded(&mut self, call: impl FnOnce(&mut App)) {
        if catch_unwind(AssertUnwindSafe(|| call(self))).is_err() {
            recovery::stop_after_panic(self.running.as_mut());
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.running.is_some() {
            return;
        }
        self.guarded(|app| match Running::new(event_loop, app.args.clone()) {
            Ok(running) => app.running = Some(running),
            Err(e) => {
                app.error = Some(e);
                event_loop.exit();
            }
        });
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _: WindowId, event: WindowEvent) {
        self.guarded(|app| {
            if let Some(running) = &mut app.running {
                running.on_event(event_loop, event);
            }
        });
    }

    /// The Claude bridge woke the loop: it has requests to run. Or the watch
    /// on Blender's tray did: something has arrived.
    fn user_event(&mut self, event_loop: &ActiveEventLoop, _: ()) {
        self.guarded(|app| {
            if let (Some(bridge), Some(running)) = (&app.bridge, &mut app.running) {
                bridge.pump(running, event_loop);
            }
            if let (Some(blender), Some(running)) = (&app.blender, &mut app.running) {
                blender.pump(running);
            }
        });
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.guarded(|app| app.before_waiting(event_loop));
    }

    /// The loop is closing, whichever way it was asked to: the last chance
    /// to write the settings. If this is the app closing properly, this
    /// run's recovery copies go (`recovery_exit` says when it is).
    fn exiting(&mut self, _: &ActiveEventLoop) {
        self.guarded(|app| {
            if let Some(running) = &mut app.running {
                running.save_prefs();
                running.recovery_exit();
            }
        });
    }
}

impl App {
    /// Between batches of events: closes if told to, and says when the next
    /// frame is wanted.
    fn before_waiting(&mut self, event_loop: &ActiveEventLoop) {
        let Some(running) = &mut self.running else { return };
        // The graphics card reported a fault: a copy of the scene, and out.
        running.stop_on_graphics_fault();
        // The question about unsaved work was answered with a way to close.
        if running.quitting() {
            event_loop.exit();
            return;
        }
        // Every few minutes, a copy of unsaved work against a crash.
        let copy_at = running.recovery_tick();
        let now = Instant::now();
        if running.repaint_at.is_some_and(|t| t <= now) {
            running.repaint_at = None;
            running.redraw = true;
        }
        let frame_asked = running.redraw || running.busy();
        if frame_asked {
            running.window.request_redraw();
        }
        event_loop.set_control_flow(wait_for(frame_asked, running.repaint_at, copy_at));
    }
}

/// How the event loop should wait for what comes next. `frame_asked`: a
/// frame has just been asked for, and drawing it brings the loop round
/// again. `repaint_at`: when a frame is next wanted otherwise. `copy_at`:
/// when to look again at whether a recovery copy is due.
///
/// The look needs no frame and must not wait for one. A window that is
/// minimised draws none however often it is asked, so with a frame asked for
/// and nothing else to wake the loop, the copies would stop for as long as
/// the window stayed down.
fn wait_for(frame_asked: bool, repaint_at: Option<Instant>, copy_at: Option<Instant>) -> ControlFlow {
    // The frame that has been asked for is the repaint, when it comes.
    let frame = repaint_at.filter(|_| !frame_asked);
    let wake = match (frame, copy_at) {
        (Some(frame), Some(copy)) => Some(frame.min(copy)),
        (frame, copy) => frame.or(copy),
    };
    wake.map_or(ControlFlow::Wait, ControlFlow::WaitUntil)
}

impl Running {
    fn new(event_loop: &ActiveEventLoop, args: Args) -> Result<Running, String> {
        let mut attrs = Window::default_attributes().with_title("GolemSculpt");
        attrs = match args.size {
            Some((w, h)) => attrs.with_inner_size(winit::dpi::PhysicalSize::new(w, h)),
            None => attrs
                .with_inner_size(winit::dpi::LogicalSize::new(1600.0, 960.0))
                .with_maximized(args.screenshot.is_none()),
        };
        // A screenshot run opens for a frame while the user may be typing
        // somewhere else: it must not take the keyboard from them.
        if args.screenshot.is_some() || args.records() || recovery::checking() {
            attrs = attrs.with_active(false);
        }
        let window = Arc::new(
            event_loop.create_window(attrs).map_err(|e| format!("Could not open a window: {e}"))?,
        );
        disable_pen_gestures(&window);
        let gpu = pollster::block_on(Gpu::new(window.clone()))?;
        let recovery = recovery::State::new(args.screenshot.is_some());
        // From here on a fault of the graphics card's is noted, not a panic.
        gpu.report_faults_to(recovery.faults.clone());

        let egui_ctx = egui::Context::default();
        theme::install(&egui_ctx);
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            Some(gpu.max_texture_side()),
        );
        let egui_renderer =
            egui_wgpu::Renderer::new(&gpu.device, gpu.format, egui_wgpu::RendererOptions::default());
        let ui = UiState::new(&egui_ctx);

        // The settings are read before the scene is made: the sphere the app
        // starts with is at the Starting Detail they remember, as the one a
        // New Scene makes is. (The rest of them go in place in `files_start`.)
        let files = files::State::new(args.screenshot.is_some());
        let mut opts = Options::default();
        opts.matcap = ui.assets.default_matcap_index();
        opts.new_shape_tris = files.starting_detail();
        let mut renderer = Renderer::new(&gpu, &ui::assets::default_matcap());
        renderer.prewarm();

        let mut mesh = new_scene::starting_mesh(opts.new_shape_tris);
        let mut scene = Scene::new(Primitive::Sphere.name());
        let mut path = None;
        // The look, view and mirror settings of a file opened here; put in
        // place once the app is up (`files_start`).
        let mut opened = None;
        // What the status line says once the app is up: a file named on the
        // command line that would not open must not fail without a word.
        let mut open_status = None;
        if let Some(file) = &args.open {
            if is_native(file) {
                match io::load_scene(file) {
                    Ok(saved) => {
                        let note = io::notes_opened(saved.repaired, saved.left_out);
                        opened = Some(install(&mut scene, &mut mesh, saved));
                        path = Some(file.clone());
                        open_status = note.map(|note| (format!("Opened {}. {note}", file.display()), true));
                    }
                    Err(e) => open_status = Some((files::start_up_failure(file, &e), false)),
                }
            } else {
                match import_mesh(file, None) {
                    Ok((m, at)) => {
                        mesh = m;
                        scene = Scene::new(&stem(file));
                        scene.active_mut().position = at;
                    }
                    Err(e) => open_status = Some((files::start_up_failure(file, &e), false)),
                }
            }
        }
        let size = window.inner_size();
        let aspect = size.width.max(1) as f32 / size.height.max(1) as f32;
        let bounds = scene.bounds(&mesh);
        let camera = new_scene::starting_view(bounds, aspect);
        let scene_size = bounds.size().max_element().max(1e-4);
        let view = camera.view(Vec2::ZERO, Vec2::new(size.width as f32, size.height as f32), camera.distance, scene_size);
        let stats = Stats { adapter: gpu.adapter_name.clone(), ..Default::default() };
        let now = Instant::now();
        let mut running = Running {
            window,
            gpu,
            renderer,
            egui_ctx,
            egui_state,
            egui_renderer,
            ui,
            doc: Doc {
                mesh,
                scene,
                mesh_replaced: true,
                history: undo::history(),
                sculptor: Sculptor::default(),
                camera,
                opts,
                path,
                stats,
                tool: Tool::Sculpt,
                spheres: Default::default(),
                tools: Default::default(),
                turntable: Default::default(),
                timelapse: Default::default(),
                still: Default::default(),
            },
            input: Input {
                pointer: Vec2::new(-1.0, -1.0),
                inside: false,
                mods: ModifiersState::empty(),
                drag: Drag::None,
                button: None,
                samples: Vec::new(),
                pen_down: false,
                pen_pressure: None,
                pen_touch: None,
                last_click: None,
                on_control: None,
                ui_tap: None,
                last_f: None,
                gauge: None,
                hover: None,
                hover_last: Default::default(),
                held: Vec::new(),
                turn_about: None,
            },
            view,
            local: view,
            view_depth: 1.0,
            scene_size,
            start: now,
            last_frame: now,
            redraw: true,
            repaint_at: None,
            frames: 0,
            pivot_seen: None,
            stroke_ended: None,
            print_watch: Default::default(),
            decimate: Default::default(),
            remesh: Default::default(),
            capture: None,
            transform: Default::default(),
            flash: Default::default(),
            recording: Default::default(),
            still: None,
            files,
            recovery,
            older: Default::default(),
            ui_demo: Default::default(),
            blender: blender::State::new(args.screenshot.is_some()),
            args,
        };
        running.scene_changed();
        running.sync_view();
        running.files_start(opened);
        if let Some((text, ok)) = open_status {
            log::warn!("{text}");
            running.status(text, ok);
        }
        if running.args.demo_scene {
            running.demo_scene();
        }
        if running.args.demo_select {
            running.demo_select();
        }
        if running.args.demo_gizmo {
            running.transform_demo();
        }
        if let Some(what) = running.args.demo_spheres.clone() {
            running.spheres_demo(&what);
        }
        if running.args.demo_mirror {
            running.mirror_demo();
        }
        if running.args.demo_symmetry {
            running.symmetry_demo();
        }
        if let Some(what) = running.args.demo_feel.clone() {
            running.feel_demo(&what);
        }
        if running.args.demo {
            running.demo_strokes();
        }
        if running.args.demo_dyntopo {
            running.dyntopo_demo();
        }
        if let Some(what) = running.args.demo_pose.clone() {
            running.pose_demo(&what);
        }
        // Before the tools, so that one of them can be shown at work on the damaged object.
        if let Some(what) = running.args.demo_print.clone() {
            running.print_demo(&what);
        }
        if let Some(what) = running.args.demo_tool.clone() {
            running.tools_demo(&what);
        }
        if let Some(resolution) = running.args.remesh {
            running.doc.opts.remesh_resolution = resolution as f32;
            running.act(Action::Remesh);
            running.remesh_wait();
        }
        if let Some(quads) = running.args.quad_remesh {
            running.doc.opts.quad_target = quads as f32;
            running.act(Action::QuadRemesh);
        }
        if let Some(triangles) = running.args.decimate {
            // The answer to the question about symmetry, were it asked: no one is here to give it.
            running.doc.opts.decimate_target = triangles as f32;
            running.act(Action::DecimateAnyway);
            running.decimate_wait();
            println!("{}", running.ui.status.0);
        }
        if running.args.demo_decimate {
            running.decimate_demo();
        }
        if running.args.demo_hide {
            running.demo_hide();
        }
        running.doc.opts.flat_shading = running.args.flat;
        if let Some(spec) = running.args.look.clone() {
            let size = crate::look::model_size(&running.doc.scene, &running.doc.mesh);
            if let Some(path) = crate::look::apply_spec(&mut running.doc.opts, &spec, size) {
                running.load_hdri(path);
            }
        }
        if let Some(picture) = running.args.demo_matcap.clone() {
            running.import_matcap(&picture);
        }
        if let Some(what) = running.args.demo_files.clone() {
            running.files_demo(&what);
        }
        if let Some(what) = running.args.demo_ui.clone() {
            running.ui_demo(&what);
        }
        if let Some(what) = running.args.demo_blender.clone() {
            running.blender_demo(&what);
        }
        if let Some(what) = running.args.demo_levels.clone() {
            running.levels_demo(&what);
        }
        if let Some(what) = running.args.demo_layers.clone() {
            running.layers_demo(&what);
        }
        if let Some(what) = running.args.demo_groups.clone() {
            running.groups_demo(&what);
        }
        if let Some(what) = running.args.demo_deform.clone() {
            running.deform_demo(&what);
        }
        if let Some(what) = running.args.demo_paint.clone() {
            running.paint_demo(&what);
        }
        if let Some((yaw, pitch)) = running.args.view {
            // Every object in the picture, from the side asked for.
            let aspect = running.view.size.x / running.view.size.y.max(1.0);
            let camera = &mut running.doc.camera;
            (camera.yaw, camera.pitch) = (yaw, pitch.clamp(-89.0, 89.0));
            camera.frame(running.doc.scene.bounds(&running.doc.mesh), aspect, false);
            running.sync_view();
        }
        // Whatever the setup above selected or undid must not pulse in the
        // first frames (a screenshot is taken in them).
        running.flash.startup(running.args.demo_flash.then(|| running.doc.scene.active_id()));
        running.recording_startup();
        // Last, over whatever the start-up above put on screen: the question
        // about a copy of the work that an earlier run left behind.
        running.recovery_start();
        Ok(running)
    }

    fn time(&self) -> f64 {
        self.start.elapsed().as_secs_f64()
    }

    /// True while something needs a frame every refresh without new input.
    fn busy(&self) -> bool {
        self.doc.sculptor.is_active()
            || self.doc.camera.is_animating()
            || self.args.screenshot.is_some()
            || self.recovery_script_running()
    }

    /// Whether the pointer is over a panel (or a panel widget is being
    /// dragged). Tested against the pointer's position right now, not the
    /// interface's own copy of it, which is only refreshed once per frame: a
    /// pen can land on a button without any hover first, and that tap must
    /// never be mistaken for a stroke on the model behind the panel.
    fn over_ui(&self) -> bool {
        let ppp = self.egui_ctx.pixels_per_point();
        let pos = egui::pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp);
        self.egui_ctx.egui_is_using_pointer()
            || self.egui_ctx.layer_id_at(pos).is_some_and(|l| l.order != egui::Order::Background)
    }

    /// Whether a press where the pointer is now is the echo of a tap on an
    /// interface control that is no longer under it (see app/echo.rs).
    fn echoes_ui_tap(&self) -> bool {
        let per_point = self.egui_ctx.pixels_per_point();
        self.input.ui_tap.is_some_and(|tap| tap.echoed_by(Instant::now(), self.input.pointer, per_point))
    }

    fn toast(&mut self, text: impl Into<String>) {
        self.ui.toast = Some((text.into(), Color32::WHITE, self.time()));
    }

    /// Writes the status line: the one at the top of the Scene panel, which
    /// is drawn over the view instead while that panel is folded away
    /// (`ui::status`). Not `ok` is an error: red, and it stays up there
    /// until something else is written or it is tapped away.
    fn status(&mut self, text: impl Into<String>, ok: bool) {
        self.ui.status = (text.into(), if ok { theme::OK } else { theme::ERROR });
        self.ui.status_seen = ui::status::Seen::at(self.time());
    }

    /// The orbit/zoom anchor: the last sculpted point, if the option is on and
    /// the point is in view. Turning about a point you cannot see is
    /// disorienting, so otherwise the camera uses its own pivot.
    fn anchor(&self) -> Option<Vec3> {
        if !self.doc.opts.stroke_pivot {
            return None;
        }
        // The brush works in the active object's space.
        let p = self.doc.scene.stroke_matrix().transform_point3(self.doc.sculptor.last_hit?);
        let px = self.view.project(p)?;
        (self.view.contains(px) && self.view.depth_of(p) > 0.0).then_some(p)
    }

    fn on_event(&mut self, event_loop: &ActiveEventLoop, event: WindowEvent) {
        // With the testing aid on, the interface is not told the pointer left
        // either, so scripted input reaches the panels too.
        if !(self.args.keep_pointer && matches!(event, WindowEvent::CursorLeft { .. })) {
            let response = self.egui_state.on_window_event(&self.window, &event);
            if response.repaint {
                self.redraw = true;
            }
        }
        match event {
            WindowEvent::CloseRequested => self.close_requested(event_loop),
            WindowEvent::Resized(size) => {
                self.gpu.resize(size.width, size.height);
                self.redraw = true;
            }
            WindowEvent::ModifiersChanged(m) => {
                self.input.mods = m.state();
                self.redraw = true;
            }
            WindowEvent::CursorMoved { position, .. } => {
                if !self.input.pen_down {
                    self.pointer_moved(Vec2::new(position.x as f32, position.y as f32), None);
                }
            }
            // Alt-tab or a click elsewhere: no release will come for what is held.
            WindowEvent::Focused(false) => self.focus_lost(),
            // A file from Explorer: opened or imported by its kind (app/drop.rs).
            WindowEvent::HoveredFile(path) => self.file_hovered(&path),
            WindowEvent::DroppedFile(path) => self.act(Action::DropFile(path)),
            WindowEvent::CursorEntered { .. } => self.input.inside = true,
            WindowEvent::CursorLeft { .. } => {
                self.input.inside = self.args.keep_pointer;
                self.redraw = true;
            }
            WindowEvent::MouseInput { state, button, .. } => {
                if self.input.pen_down {
                    return;
                }
                match state {
                    ElementState::Pressed => self.pointer_pressed(button, None),
                    ElementState::Released => self.pointer_released(button),
                }
            }
            // A pen arrives as touch events carrying pressure (app/pen.rs).
            WindowEvent::Touch(touch) => self.on_touch(&touch),
            WindowEvent::MouseWheel { delta, .. } => {
                let lines = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.wheel(lines);
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    let typing = self.egui_ctx.egui_wants_keyboard_input();
                    self.input.note_key(code, event.state == ElementState::Pressed);
                    match event.state {
                        // Releases always go through, so a held key can never stick.
                        ElementState::Released => self.key_released(code),
                        ElementState::Pressed if !typing && !event.repeat => {
                            self.key_pressed(event_loop, code)
                        }
                        // [ and ] held down keep stepping the brush (app/brush_keys.rs).
                        ElementState::Pressed if !typing => {
                            self.brush_step_key(code);
                        }
                        _ => {}
                    }
                }
                self.redraw = true;
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn pointer_moved(&mut self, pos: Vec2, pressure: Option<f32>) {
        let delta = pos - self.input.pointer;
        self.input.pointer = pos;
        self.redraw = true;
        if self.spheres_moved(pos) || self.transform_moved(pos) || self.tools_moved(pos, pressure) {
            return;
        }
        if let Some(g) = self.input.gauge {
            let travel = pos.x - g.anchor.x;
            let s = &mut self.doc.sculptor.settings;
            match g.kind {
                GaugeKind::Size => {
                    s.set_size(g.start + travel * s.size_per_drag_px(), self.view.size.y);
                }
                GaugeKind::Strength => {
                    s.current_mut().strength = (g.start + travel * 0.00125).clamp(0.01, 1.0);
                }
                GaugeKind::Focal => {
                    s.current_mut().focal_shift = (g.start - travel * 0.0025).clamp(-1.0, 1.0);
                }
            }
            return;
        }
        match self.input.drag {
            Drag::Orbit => self.turn_by(delta),
            Drag::Pan => {
                let wpp = self.view.world_per_pixel(self.view_depth);
                self.doc.camera.pan(delta, wpp);
            }
            Drag::Zoom => {
                let factor = (1.0 + delta.y * camera::DRAG_ZOOM_PER_PX).max(0.1);
                let anchor = self.anchor();
                self.doc.camera.zoom(factor, self.view_depth, self.scene_size, anchor);
            }
            Drag::Sculpt => {
                self.input.samples.push(PointerSample { pos, pressure, time: self.time() });
            }
            Drag::None | Drag::Ui | Drag::Spheres | Drag::Gizmo => {}
        }
    }

    fn pointer_pressed(&mut self, button: MouseButton, pressure: Option<f32>) {
        self.redraw = true;
        if self.input.drag != Drag::None {
            return;
        }
        self.input.button = Some(button);
        let on_interface = self.over_ui();
        if on_interface || self.ui.radial.is_some() || self.input.gauge.is_some() {
            self.input.drag = Drag::Ui;
            // A held-key gauge is a drag on the view, not a tap on a control:
            // the stroke that follows it may well begin where it ended.
            self.input.on_control = (on_interface && self.input.gauge.is_none()).then_some(self.input.pointer);
            return;
        }
        // The control tapped a moment ago has gone from under the pen (a
        // panel folded, a card put back): its second tap is not a stroke.
        if button == MouseButton::Left && self.echoes_ui_tap() {
            self.input.drag = Drag::Ui;
            return;
        }
        let mods = self.input.mods;
        let pos = self.input.pointer;
        self.input.drag = match button {
            MouseButton::Middle => Drag::Pan,
            MouseButton::Left if mods.alt_key() && mods.control_key() => Drag::Zoom,
            MouseButton::Left if mods.alt_key() && mods.shift_key() => Drag::Pan,
            MouseButton::Left if mods.alt_key() => Drag::Orbit,
            // The clean view is for looking: no tool or brush takes the press.
            // So is a Decimate preview: what is shown is not the model a brush would work on.
            MouseButton::Left | MouseButton::Right if self.doc.opts.clean_view || self.decimate.previewing() => {
                if button == MouseButton::Left { Drag::Orbit } else { Drag::Ui }
            }
            // A click for the Face Groups card: Ctrl + Shift + click on a group, or one the card armed.
            MouseButton::Left if self.groups_takes_press() => self.groups_pressed(),
            MouseButton::Left | MouseButton::Right if self.tools_active() => self.tools_pressed(button, pressure),
            MouseButton::Left | MouseButton::Right if self.spheres_active() => self.spheres_pressed(button),
            MouseButton::Left | MouseButton::Right if self.transform_active() => self.transform_pressed(button),
            MouseButton::Left | MouseButton::Right => {
                let invert = mods.control_key() || button == MouseButton::Right;
                let sample = PointerSample { pos, pressure, time: self.time() };
                self.sync_view();
                // Only the active object takes strokes, and only where no
                // other object is in front of it.
                let placed = self.pick_placed(pos);
                let picked = placed.map(|(id, _)| id);
                let on_active = self.press_on_active(placed, pos);
                // A layer that records but does not show takes no stroke, and the status line says why.
                // Nor does Erase Layer begin one with no layer to rub out.
                let refused = on_active && (self.layer_refuses_stroke() || self.erase_refuses_stroke(mods.shift_key()));
                // The Paint brush's Sample mode is the eyedropper: a click, no stroke. And a Paint stroke needs a
                // layer to paint on, which is made if there is none (app/paint.rs).
                let refused = refused || (on_active && self.paint_samples_press(pos, mods.shift_key()));
                let refused = refused || (on_active && self.paint_refuses_stroke(mods.shift_key()));
                if on_active && !refused {
                    self.stroke_baseline(mods.shift_key());
                }
                let doc = &mut self.doc;
                if refused {
                    Drag::Ui
                } else if on_active && doc.begin_stroke(&self.local, sample, invert, mods.shift_key()) {
                    // Drawn by hand: it may fall behind the pen rather than hold the frames up (sculpt/pace.rs).
                    doc.sculptor.pace = crate::sculpt::Pace::LIVE;
                    self.input.samples.clear();
                    Drag::Sculpt
                } else if button == MouseButton::Left {
                    // Off the active object: a second click picks the object
                    // under it or, on empty space, frames the model. A drag
                    // orbits.
                    let now = Instant::now();
                    let double = self.input.last_click.is_some_and(|(t, p)| {
                        now - t < DOUBLE_CLICK && p.distance(pos) < DOUBLE_CLICK_PX
                    });
                    self.input.last_click = Some((now, pos));
                    if double {
                        self.input.last_click = None;
                        match picked {
                            Some(id) if !on_active => self.act(Action::SelectObject(id)),
                            _ => self.frame_view(),
                        }
                    }
                    Drag::Orbit
                } else {
                    Drag::Ui
                }
            }
            _ => Drag::Ui,
        };
        if matches!(self.input.drag, Drag::Orbit | Drag::Pan | Drag::Zoom) {
            self.navigation_began();
        }
    }

    fn pointer_released(&mut self, button: MouseButton) {
        if self.input.button != Some(button) {
            return;
        }
        self.input.button = None;
        if let Some(pressed_at) = self.input.on_control.take() {
            let per_point = self.egui_ctx.pixels_per_point();
            self.input.ui_tap = echo::Tap::ending(pressed_at, self.input.pointer, per_point);
        }
        if self.input.drag == Drag::Sculpt {
            self.finish_stroke();
        }
        self.spheres_released();
        self.transform_released();
        self.tools_released();
        // An orbit that went somewhere is not the first click of a double-click.
        let pointer = self.input.pointer;
        if self.input.drag == Drag::Orbit && self.input.last_click.is_some_and(|(_, at)| at.distance(pointer) > DOUBLE_CLICK_PX) {
            self.input.last_click = None;
        }
        self.input.drag = Drag::None;
        self.redraw = true;
    }

    fn finish_stroke(&mut self) {
        // The stroke's undo step is called by its brush ("Undo Clay Strips").
        let name = self.stroke_name();
        self.doc.history.name_next(Some(name));
        focus::end_stroke(&mut self.input, &mut self.doc, &self.local);
        self.doc.history.name_next(None);
        self.stroke_ended = Some(Instant::now());
        self.stroke_check();
    }

    fn wheel(&mut self, lines: f32) {
        if lines.abs() < 0.01 || self.over_ui() {
            return;
        }
        self.redraw = true;
        if !self.doc.opts.clean_view && (self.spheres_wheel(lines) || self.tools_wheel(lines)) {
            return;
        }
        if self.input.hover.is_some() && !self.doc.sculptor.is_active() {
            // Over the model the wheel sizes the brush; off it, it zooms.
            let s = &mut self.doc.sculptor.settings;
            s.set_size(s.size() * (1.0 + 0.1 * lines.signum()), self.view.size.y);
        } else {
            let factor = 1.0 - lines.signum() * camera::WHEEL_ZOOM_STEP;
            let anchor = self.anchor();
            self.doc.camera.zoom(factor, self.view_depth, self.scene_size, anchor);
        }
    }

    fn key_pressed(&mut self, event_loop: &ActiveEventLoop, code: KeyCode) {
        let mods = self.input.mods;
        let ctrl = mods.control_key();
        let shift = mods.shift_key();
        let idle = self.input.drag == Drag::None;
        // A question on screen (the Delete key's) takes every key until answered.
        if self.confirm_key(code) {
            return;
        }
        // Tab, O and Esc are the turntable's; the clean view takes the rest.
        if self.recording_key(code) {
            return;
        }
        // Esc takes back a stroke or a gizmo drag under way (app/escape.rs).
        if code == KeyCode::Escape && self.escape_drag() {
            return;
        }
        if self.spheres_key_pressed(code) || self.tools_key_pressed(code) || self.decimate_key(code) || self.deform_key(code) {
            return;
        }
        if self.brush_step_key(code) {
            return;
        }
        if let Some(&b) = Brush::ALL.iter().find(|&&b| brush_key(b) == Some(code)) {
            if idle && !ctrl {
                // The number keys are the base brushes' (the first ten of them: `Brush::hotkey`).
                let sculptor = &mut self.doc.sculptor;
                sculptor.library.select(crate::sculpt::library::Library::base(b), &mut sculptor.settings);
                // With the Transform tool up, the brush is picked to sculpt with.
                self.back_to_sculpting();
                self.toast(b.name());
            }
            return;
        }
        let anchor = self.input.pointer;
        let gauge = move |kind, start| Gauge { kind, anchor, start };
        match code {
            KeyCode::KeyZ if ctrl && shift => self.act(Action::Redo),
            KeyCode::KeyZ if shift => self.act(Action::Redo),
            KeyCode::KeyZ => self.act(Action::Undo),
            KeyCode::KeyY if ctrl => self.act(Action::Redo),
            KeyCode::KeyS if ctrl && shift => self.act(Action::SaveAs),
            KeyCode::KeyS if ctrl => self.act(Action::Save),
            KeyCode::KeyO if ctrl => self.act(Action::Open),
            // Not in the middle of a drag: it would take the scene from under it.
            KeyCode::KeyN if ctrl => {
                if idle {
                    self.act(Action::NewScene);
                }
            }
            KeyCode::Equal if ctrl => self.doc.opts.ui_scale = (self.doc.opts.ui_scale + 0.1).min(3.0),
            KeyCode::Minus if ctrl => self.doc.opts.ui_scale = (self.doc.opts.ui_scale - 0.1).max(0.75),
            KeyCode::KeyS if idle => {
                self.input.gauge = Some(gauge(GaugeKind::Size, self.doc.sculptor.settings.size()));
            }
            KeyCode::KeyF if idle => {
                // F F frames the model; a lone F held and dragged is strength.
                let now = Instant::now();
                if self.input.last_f.is_some_and(|t| now - t < DOUBLE_CLICK) {
                    self.input.last_f = None;
                    self.frame_view();
                } else {
                    self.input.last_f = Some(now);
                    self.input.gauge =
                        Some(gauge(GaugeKind::Strength, self.doc.sculptor.settings.current().strength));
                }
            }
            // Subdivision levels (app/levels.rs). Ctrl + D before plain D, which is the focal shift.
            KeyCode::KeyD if ctrl => {
                if idle {
                    self.act(Action::Subdivide);
                }
            }
            KeyCode::PageUp if idle => self.act(Action::LevelBy(1)),
            KeyCode::PageDown if idle => self.act(Action::LevelBy(-1)),
            KeyCode::KeyD if idle => {
                self.input.gauge =
                    Some(gauge(GaugeKind::Focal, self.doc.sculptor.settings.current().focal_shift));
            }
            // P: the Paint brush, and again the brush before it. I: the eyedropper, at the pointer (app/paint.rs).
            KeyCode::KeyP if idle && !ctrl => self.paint_key(),
            KeyCode::KeyI if idle && !ctrl => self.act(Action::Paint(crate::doc::PaintAction::PickAt(self.input.pointer))),
            KeyCode::KeyM if idle => {
                let s = &mut self.doc.sculptor.settings;
                s.mask_mode = !s.mask_mode;
                let on = s.mask_mode;
                self.toast(if on { "Mask" } else { "Mask Off" });
            }
            KeyCode::KeyX if idle => {
                // The selected object's own Mirror X, as in the Unity app.
                self.doc.edit_symmetry(|symmetry| symmetry.mirror[0] = !symmetry.mirror[0]);
                let on = self.doc.scene.active_symmetry().mirror[0];
                self.toast(if on { "Mirror X" } else { "Mirror Off" });
            }
            KeyCode::KeyR if idle => self.act(Action::Remesh),
            // G, not T: T is the Tube tool's key, as in the Unity app.
            KeyCode::KeyG if idle => {
                self.set_tool(if self.transform_active() { Tool::Sculpt } else { Tool::Transform });
            }
            KeyCode::Delete if idle => self.delete_key(),
            KeyCode::KeyH if idle && !ctrl => self.hide_key(),
            KeyCode::Slash | KeyCode::NumpadDivide if idle && !ctrl => self.local_view_key(),
            KeyCode::Space if idle && self.ui.radial.is_none() => {
                let ppp = self.egui_ctx.pixels_per_point();
                self.ui.radial = Some(egui::pos2(self.input.pointer.x / ppp, self.input.pointer.y / ppp));
            }
            KeyCode::Escape => {
                // The wheel and the clean view first; then the Transform tool is put away.
                if self.ui.radial.is_none() && !self.doc.opts.clean_view {
                    self.escape_tool();
                }
                self.ui.radial = None;
                self.doc.opts.clean_view = false;
            }
            // Orthographic or perspective, as Blender's Numpad 5 (app/turn.rs).
            KeyCode::Numpad5 if idle => self.act(Action::ToggleOrtho),
            KeyCode::F11 => {
                let full = self.window.fullscreen().is_some();
                self.window.set_fullscreen((!full).then_some(Fullscreen::Borderless(None)));
            }
            KeyCode::KeyQ if ctrl => self.close_requested(event_loop),
            // A key nothing above uses may be a hosted tool's (L is Measure).
            _ => {
                self.tools_hotkey(code);
            }
        }
    }

    fn key_released(&mut self, code: KeyCode) {
        self.spheres_key_released(code);
        self.tools_key_released(code);
        match code {
            KeyCode::KeyS | KeyCode::KeyF | KeyCode::KeyD => {
                // F held and dragged set the strength: it is not the first F of F F.
                let moved = self.input.gauge.is_some_and(|g| g.anchor.distance(self.input.pointer) > DOUBLE_CLICK_PX);
                if code == KeyCode::KeyF && moved {
                    self.input.last_f = None;
                }
                self.input.gauge = None;
            }
            KeyCode::Space => self.ui.radial = None,
            _ => {}
        }
    }

    fn frame_view(&mut self) {
        if self.spheres_frame_view() {
            return;
        }
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        let scene = &self.doc.scene;
        // The whole selection, not only the object brushes work on.
        let bounds = scene.selection_bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, aspect, true);
        // Shows which objects the view went to.
        for id in self.doc.scene.selected().to_vec() {
            self.flash.start(id, flash::Kind::Select);
        }
        self.redraw = true;
    }

    /// The visible object under a pixel, if any.
    fn pick(&self, px: Vec2) -> Option<u64> {
        let (origin, dir) = self.view.ray(px);
        self.doc.scene.pick(origin, dir, &self.doc.mesh).map(|p| p.id)
    }

    /// Call after anything that may have changed the object list or which
    /// object is active.
    fn scene_changed(&mut self) {
        let doc = &mut self.doc;
        let mesh = doc.scene.active_mesh();
        if doc.history.target != mesh {
            doc.history.target = mesh;
            doc.sculptor.last_hit = None;
        }
        // Meshes of deleted objects are kept only while undo can bring them
        // back, and count against what undo may hold.
        doc.history.settle(&mut doc.scene);
        self.redraw = true;
    }

    /// Runs an edit to the object list as one undo step. `edit` returns
    /// whether it changed anything.
    fn edit_scene(&mut self, edit: impl FnOnce(&mut Scene, &mut Mesh) -> bool) -> bool {
        let doc = &mut self.doc;
        let before = doc.scene.state();
        let changed = edit(&mut doc.scene, &mut doc.mesh);
        if changed {
            doc.history.push_scene(before);
        }
        self.scene_changed();
        changed
    }

    /// Adds a mesh to the scene as a new object at the centre and makes it
    /// the active one (so it becomes `doc.mesh`). One undo step.
    #[allow(dead_code)]
    pub(crate) fn add_object(&mut self, name: &str, mesh: Mesh) -> u64 {
        let mut id = 0;
        self.edit_scene(|scene, slot| {
            id = scene.add(name, mesh, slot);
            true
        });
        id
    }

    /// Replaces the mesh, keeping the old one on the undo stack.
    fn replace_mesh(&mut self, mut mesh: Mesh, undoable: bool) {
        // The same shape built again keeps its face groups (app/groups.rs). A mesh that is
        // not an undo step is a new model put in for a check, and starts with none.
        if undoable {
            groups::follow(&self.doc.mesh, &mut mesh, crate::mesh::groups::Fit::Whole);
        }
        let old = std::mem::replace(&mut self.doc.mesh, mesh);
        if undoable {
            self.doc.history.push_replace(old);
        } else {
            self.doc.history.clear();
        }
        self.doc.mesh_replaced = true;
        self.doc.sculptor.last_hit = None;
    }

    /// Builds the mesh again without what dynamic topology's merges left
    /// behind (mesh/dyntopo.rs), as an undo step of its own, before a tool
    /// that reads every vertex and triangle and changes the mesh where it is
    /// (Make Symmetric, Symmetry Cleanup, Groups by Angle). Nothing when
    /// there is nothing to tidy.
    fn tidy_merged(&mut self) {
        if let Some(tidy) = crate::scene::tidied(&self.doc.mesh) {
            self.replace_mesh(tidy, true);
        }
    }

    /// Carries out a command. The undo steps it records are called by what
    /// it is (app/undo_names.rs).
    fn act(&mut self, action: Action) {
        let name = undo_names::of(&action);
        self.doc.history.name_next(name);
        self.act_now(action);
        self.doc.history.name_next(None);
    }

    fn act_now(&mut self, action: Action) {
        // A gizmo drag, like a stroke, holds what its undo step goes back to.
        if self.doc.sculptor.is_active() || self.transform_dragging() {
            return;
        }
        self.redraw = true;
        // A Strength slider of the Layers card that is still held is let go first.
        self.layers_before(&action);
        // And so is an Opacity slider of the Paint Layers card (app/paint.rs).
        self.paint_before(&action);
        // And a deformation on the model is applied (app/deform.rs).
        self.deform_before(&action);
        // A Decimate preview is closed, unless the command is only about the view (app/decimate.rs).
        if self.decimate_before(&action) {
            return;
        }
        // While the SSphere tool is up, Z steps its rig first.
        if matches!(action, Action::Undo | Action::Redo)
            && (self.spheres_undo(action == Action::Redo) || self.tools_undo(action == Action::Redo))
        {
            return;
        }
        match action {
            // The History list: so many presses of Undo or Redo at once.
            Action::UndoSteps(n) | Action::RedoSteps(n) => {
                let one = if matches!(action, Action::UndoSteps(_)) { Action::Undo } else { Action::Redo };
                for _ in 0..n {
                    self.act(one.clone());
                }
            }
            Action::Undo | Action::Redo => {
                let undo = action == Action::Undo;
                // What the step is called, for the toast: asked before it is taken.
                let name = self.doc.history.next_name(undo).map(str::to_owned);
                // Asked before the step: the step moves what the answer goes by.
                let was_saved = !self.unsaved();
                let doc = &mut self.doc;
                let (mut done, mut first) = (false, None);
                loop {
                    let next = if undo { doc.history.next_undo() } else { doc.history.next_redo() };
                    if let Some(Next::Mesh(id)) = next {
                        first.get_or_insert(id);
                    }
                    let stepped = match next {
                        Some(Next::Scene) => doc.history.step_scene(undo, &mut doc.scene, &mut doc.mesh),
                        Some(Next::Mesh(id)) => {
                            // The step may belong to an object that is not the
                            // active one any more: go back to it first.
                            doc.scene.focus_mesh(id, &mut doc.mesh);
                            let step = if doc.scene.active_mesh() != id {
                                None
                            } else if undo {
                                doc.history.undo(&mut doc.mesh)
                            } else {
                                doc.history.redo(&mut doc.mesh)
                            };
                            doc.mesh_replaced |= step == Some(true);
                            step.is_some()
                        }
                        None => false,
                    };
                    done |= stepped;
                    // One edit of several meshes is taken whole (`History::join_last`),
                    // and leaves the object it began with active.
                    if !stepped || !doc.history.joined(undo) {
                        break;
                    }
                }
                if let Some(first) = first.filter(|_| !undo) {
                    doc.scene.focus_mesh(first, &mut doc.mesh);
                }
                if done {
                    self.scene_changed();
                    // Across an Open, the file comes with the scene.
                    self.file_stepped(undo, was_saved);
                    let verb = if undo { "Undo" } else { "Redo" };
                    self.toast(name.map_or(verb.to_owned(), |name| format!("{verb} {name}")));
                    // A mesh step has made its object the active one.
                    self.flash.start(self.doc.scene.active_id(), flash::Kind::Undo);
                }
            }
            Action::New(kind) => self.new_shape(kind),
            Action::SelectObject(id) => {
                let doc = &mut self.doc;
                let before = doc.scene.active_id();
                doc.scene.select(id, &mut doc.mesh);
                self.scene_changed();
                // Only a new target pulses, not a click on the active object.
                if self.doc.scene.active_id() != before {
                    self.flash.start(id, flash::Kind::Select);
                }
            }
            Action::SelectAlso(id) => self.select_also(id),
            Action::Join { remesh } => self.join_selected(remesh),
            Action::Boolean { op, resolution, delete } => self.boolean_selected(op, resolution, delete),
            Action::ResetShape => self.reset_shape(),
            Action::ToggleVisible(id) => self.toggle_visible(id),
            Action::RenameObject(id, name) => {
                self.edit_scene(|scene, _| match scene.get_mut(id) {
                    Some(o) if o.name != name => {
                        o.name = name;
                        true
                    }
                    _ => false,
                });
            }
            Action::DeleteObject(id) => {
                let name = self.doc.scene.get(id).map(|o| o.name.clone()).unwrap_or_default();
                if self.edit_scene(|scene, slot| scene.remove(id, slot)) {
                    self.older_deleted(id, &name);
                    self.status(format!("Deleted \"{name}\". Undo (Z) restores it."), true);
                } else if self.doc.scene.objects.len() < 2 {
                    self.status("The last object cannot be deleted.", false);
                }
            }
            Action::MoveObject(id, to) => {
                self.edit_scene(|scene, _| scene.move_to(id, to));
            }
            Action::CloneObject => {
                self.edit_scene(|scene, slot| {
                    scene.clone_active(slot);
                    true
                });
                self.status("Copy added on top of the original. Move it with Position.", true);
            }
            Action::InstanceObject => {
                self.edit_scene(|scene, _| {
                    scene.instance_active();
                    true
                });
                self.status(
                    "Instance added on top of the original. Move it with Position. Sculpting either changes both.",
                    true,
                );
            }
            Action::MakeUnique => {
                if self.edit_scene(|scene, slot| scene.make_unique(slot)) {
                    let name = self.doc.scene.active().name.clone();
                    self.status(format!("\"{name}\" is now an independent object."), true);
                }
            }
            // In the background (app/remesh_job.rs).
            Action::Remesh => self.remesh_start(),
            Action::QuadRemesh => {
                let target = self.doc.opts.quad_target as usize;
                // A quad remesh keeps the holes a model has, and can leave new ones.
                let was_closed = crate::printable::census(&self.doc.mesh).is_closed();
                match crate::quadremesh::quad_remesh_mesh(&self.doc.mesh, target) {
                    Ok((mesh, report)) => {
                        self.replace_mesh(mesh, true);
                        // The quads are the point: show them.
                        self.doc.opts.wireframe = true;
                        self.toast("Quad Remesh");
                        let closed = report.census().is_closed();
                        let text = format!("Quad remesh: {}.{}", report.summary(), report.advice(was_closed));
                        self.status(text, closed);
                    }
                    Err(e) => self.status(e, false),
                }
            }
            Action::CutAndMirror { axis, from_positive } => self.cut_and_mirror(axis, from_positive),
            // The rest of the Symmetry card's repairs are in app/symmetry_repair.rs.
            Action::CheckSymmetry { axis } => self.check_symmetry(axis),
            Action::MakeSymmetric { axis, from_positive } => self.make_symmetric(axis, from_positive),
            Action::SymmetryCleanup { axis } => self.symmetry_cleanup(axis),
            // Subdivision levels are in app/levels.rs.
            Action::Subdivide => self.subdivide(),
            Action::Level(level) => self.level_to(level),
            Action::LevelBy(by) => self.level_by(by),
            Action::DeleteLevels { lower } => self.delete_levels(lower),
            Action::Layer(action) => self.layer_act(action),
            Action::Groups(ask) => self.groups_act(ask),
            Action::Paint(action) => self.paint_act(action),
            Action::Decimate => self.decimate_asking(),
            Action::DecimateAnyway => self.decimate_start(),
            Action::DecimatePreview => self.decimate_preview_start(),
            Action::DecimateApply => self.decimate_apply(),
            Action::DecimateCancel => self.decimate_cancel(),
            Action::Mask(op) => {
                let delta = match op {
                    MaskOp::Invert => self.doc.mesh.remap_mask(|_, _, m| 1.0 - m),
                    MaskOp::Clear => self.doc.mesh.remap_mask(|_, _, _| 0.0),
                    MaskOp::Blur => self.doc.mesh.remap_mask(|mesh, v, m| 0.5 * (m + mesh.mask_ring_mean(v))),
                    MaskOp::Sharpen => self.doc.mesh.remap_mask(|mesh, v, m| {
                        ((2.0 * m - mesh.mask_ring_mean(v)).clamp(0.0, 1.0) - 0.5) * 1.25 + 0.5
                    }),
                    MaskOp::Grow => self.doc.mesh.remap_mask(|mesh, v, _| mesh.mask_ring_extreme(v, true)),
                    MaskOp::Shrink => self.doc.mesh.remap_mask(|mesh, v, _| mesh.mask_ring_extreme(v, false)),
                };
                if let Some(delta) = delta {
                    self.doc.history.push_stroke(delta);
                }
            }
            Action::MaskShape(shape, amount) => self.mask_shape(shape, amount),
            Action::HideFaces { triangles, hide } => {
                self.hide_faces(&triangles, hide);
            }
            Action::ShowAllFaces => {
                self.show_all_faces();
            }
            Action::InvertVisible => {
                self.invert_visible_faces();
            }
            Action::Matcap(i) => {
                if let Some(image) = self.ui.assets.matcap_image(i) {
                    self.renderer.set_matcap(&self.gpu, &image);
                    self.doc.opts.matcap = i;
                    self.doc.opts.matcap_on = true;
                }
            }
            Action::ImportMatcap => {
                let picked = rfd::FileDialog::new()
                    .set_title("Import Matcap")
                    .add_filter("Matcap pictures", &["png", "jpg", "jpeg"])
                    .pick_file();
                if let Some(path) = picked {
                    self.import_matcap(&path);
                }
            }
            Action::RescanMatcaps => {
                // The list may have shifted under the selected matcap, or
                // lost it; show whichever one is selected now, on or off as
                // it was.
                let on = self.doc.opts.matcap_on;
                let message = self.ui.assets.rescan_matcaps(&self.egui_ctx, &mut self.doc.opts.matcap);
                self.act(Action::Matcap(self.doc.opts.matcap));
                self.doc.opts.matcap_on = on;
                self.status(message, true);
            }
            Action::ImportAlpha => self.pick_alpha(),
            Action::Brush(ask) => self.brush_ask(ask),
            Action::Material(ask) => self.material_ask(ask),
            Action::RescanAlphas => self.rescan_alphas(),
            Action::LoadHdri => {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose an HDRI")
                    .add_filter("Environment pictures", &["hdr", "exr", "png", "jpg", "jpeg"])
                    .pick_file();
                if let Some(path) = picked {
                    self.load_hdri(path);
                }
            }
            Action::LoadLut => {
                let picked = rfd::FileDialog::new()
                    .set_title("Choose a colour table")
                    .add_filter("3D LUT", &["cube"])
                    .pick_file();
                if let Some(path) = picked {
                    self.load_lut(path);
                }
            }
            Action::ClearLut => {
                self.renderer.set_lut(&self.gpu, None);
                let lut = &mut self.doc.opts.look.lut;
                (lut.loaded, lut.failed, lut.path) = (false, false, None);
                lut.status = "No colour table loaded.".into();
            }
            Action::ClearHdri => {
                self.renderer.set_hdri(&self.gpu, None);
                let hdri = &mut self.doc.opts.look.hdri;
                hdri.loaded = false;
                hdri.enabled = false;
                hdri.failed = false;
                hdri.path = None;
                hdri.status = "No HDRI loaded.".into();
            }
            Action::Frame => self.frame_view(),
            Action::Snap(yaw, pitch) => self.snap_view(yaw, pitch),
            Action::ToggleOrtho => self.toggle_ortho(),
            // The view gizmo's hand, magnifier and Home (app/view_moves.rs).
            Action::PanView(by) => self.pan_from_gizmo(Vec2::from_array(by)),
            Action::ZoomView(dy) => self.zoom_from_gizmo(dy),
            Action::HomeView => self.home_view(),
            Action::DropFile(path) => self.file_dropped(path),
            Action::Turn { by, begins } => self.turn_from_gizmo(Vec2::from_array(by), begins),
            Action::Tool(tool) => self.set_tool(tool),
            Action::ResetTransform => self.reset_transform(),
            Action::SetOriginToMesh => self.set_origin_to_mesh(),
            Action::MakeSkin => self.spheres_make_skin(),
            Action::Mirror(command) => self.mirror_command(command),
            Action::NewRig => self.spheres_new_rig(),
            Action::Deform(ask) => self.deform_act(ask),
            // A new scene is in app/new_scene.rs.
            Action::NewScene => self.new_scene_action(),
            Action::NewSceneAs { save } => self.new_scene_as(save),
            // Saving, opening and closing are in app/files.rs.
            Action::Open => self.open_action(),
            Action::Import => self.import_action(),
            Action::Save | Action::SaveAs => {
                self.save_action(action == Action::SaveAs);
            }
            Action::OpenRecent(path) => self.open_recent(path),
            Action::OpenAs(path, how) => self.open_as(path, how),
            Action::Exit => self.exit_action(),
            Action::Quit { save } => self.quit(save),
            Action::Recover(answer) => self.recover(answer),
            // Earlier saves and kept recovery copies are in app/older.rs.
            Action::OlderVersions => self.older_versions_action(),
            Action::OpenOlder { file, add } => self.open_older(file, add),
            Action::ShowFolder(folder) => self.show_folder(&folder),
            // Export OBJ and PLY are in app/exports.rs.
            Action::ExportObj => self.export_obj_action(),
            Action::ExportPly => self.export_ply_action(),
            Action::ExportFbx => self.export_fbx_action(),
            // The Blender link is in app/blender.rs.
            Action::SendToBlender => self.send_to_blender(),
            // Checking and exporting for printing are in app/print_check.rs.
            Action::ExportStl => self.export_stl_asking(),
            Action::ExportStlAnyway => self.export_stl_now(),
            Action::CheckForPrinting => self.check_for_printing(),
        }
    }

    /// An export's result as success or failure, and the sentence to add to its status line
    /// when damaged points were written mended (empty when none were).
    fn split_mended(result: io::Result<usize>) -> (io::Result<()>, String) {
        let mended = result.as_ref().map_or(0, |&count| count);
        let note = io::note_saved(mended).map_or(String::new(), |note| format!(". {note}"));
        (result.map(|_| ()), note)
    }

    /// Rebuilds the per-frame view from the camera (and measures the distance
    /// to the nearest visible surface, which zoom and the near plane use).
    /// Import Matcap, once a picture has been picked: copies it into the
    /// matcap folder, shades with it, and says so. A picture larger than the
    /// graphics device takes is shown smaller (`render::mips::fitted`), and
    /// the line says that too.
    fn import_matcap(&mut self, path: &std::path::Path) {
        match self.ui.assets.import_matcap(&self.egui_ctx, path, &mut self.doc.opts.matcap) {
            Ok(mut message) => {
                self.act(Action::Matcap(self.doc.opts.matcap));
                if let Ok((width, height)) = image::image_dimensions(path) {
                    let (w, h) = self.gpu.fitted_size(width, height);
                    if (w, h) != (width, height) {
                        let shown = format!("larger than this graphics card takes, so it is shown at {w} x {h}");
                        message += &format!(". It is {width} x {height}, {shown}.");
                    }
                }
                self.status(message, true);
            }
            Err(e) => self.status(e, false),
        }
    }

    /// Loads `path` as the environment picture and switches HDRI lighting
    /// on. A file that cannot be read leaves the previous picture in place.
    fn load_hdri(&mut self, path: PathBuf) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match crate::look::load_hdri(&path) {
            Ok(image) => {
                let hdri = &mut self.doc.opts.look.hdri;
                hdri.status = format!("{name} ({}x{})", image.width, image.height);
                hdri.path = Some(path);
                hdri.loaded = true;
                hdri.enabled = true;
                hdri.failed = false;
                self.renderer.set_hdri(&self.gpu, Some(image));
            }
            Err(e) => {
                let hdri = &mut self.doc.opts.look.hdri;
                hdri.status = e;
                hdri.failed = true;
            }
        }
        self.redraw = true;
    }

    /// Loads `path` as the colour table the picture is graded by. A file that
    /// cannot be read leaves the previous table in place.
    fn load_lut(&mut self, path: PathBuf) {
        let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
        match crate::render::lut::Lut::load(&path) {
            Ok(table) => {
                self.renderer.set_lut(&self.gpu, Some(&table));
                let lut = &mut self.doc.opts.look.lut;
                lut.status = format!("{name} ({0} x {0} x {0})", table.size);
                (lut.path, lut.loaded, lut.failed) = (Some(path), true, false);
            }
            Err(e) => {
                let lut = &mut self.doc.opts.look.lut;
                lut.status = e;
                lut.failed = true;
            }
        }
        self.redraw = true;
    }

    /// The material, lighting and environment inputs of this frame.
    fn look_uniform(&self) -> crate::look::LookUniform {
        let o = &self.doc.opts;
        let v = &self.view;
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        let (scene_min, scene_max) =
            if bounds.is_empty() { (Vec3::splat(-0.5), Vec3::splat(0.5)) } else { (bounds.min, bounds.max) };
        o.look.uniform(&crate::look::Frame {
            cam_right: v.right,
            cam_up: v.up,
            cam_forward: v.forward,
            ortho_half_height: v.ortho_half_height,
            eye_offset: v.depth_of(self.doc.camera.pivot) - self.doc.camera.distance,
            scene_min,
            scene_max,
            model_size: crate::look::model_size(&self.doc.scene, &self.doc.mesh),
            time: self.time() as f32,
            gradient_background: o.gradient_background,
            smoothness: o.smoothness,
            shadow_size: o.quality.shadow_size(),
        })
    }

    fn sync_view(&mut self) {
        let size = self.window.inner_size();
        let size = Vec2::new(size.width.max(1) as f32, size.height.max(1) as f32);
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.scene_size = if bounds.is_empty() { 1.0 } else { bounds.size().max_element().max(1e-4) };
        let probe = self.doc.camera.view(Vec2::ZERO, size, self.view_depth, self.scene_size);
        let mut depth = self.doc.camera.distance;
        for uv in [(0.5, 0.5), (0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)] {
            let (o, d) = probe.ray(Vec2::new(uv.0, uv.1) * size);
            if let Some(hit) = self.doc.scene.pick(o, d, &self.doc.mesh) {
                let forward = probe.depth_of(hit.pos);
                if forward > 0.0 {
                    depth = depth.min(forward);
                }
            }
        }
        self.view_depth = depth.max(1e-5);
        self.view = self.doc.camera.view(Vec2::ZERO, size, self.view_depth, self.scene_size);
        self.local = self.view.local_to(self.doc.scene.brush_matrix());
    }

    fn cursor(&mut self) -> Cursor {
        let ppp = self.egui_ctx.pixels_per_point();
        let to_points = |p: Vec2| egui::pos2(p.x / ppp, p.y / ppp);
        let s = &self.doc.sculptor.settings;
        let mods = self.input.mods;
        let brush = self.doc.sculptor.effective_brush(mods.shift_key());
        // With Shift held the ring is the brush Shift smooths with, which
        // need not be the Smooth brush last in hand.
        let shift = mods.shift_key() && !s.mask_mode;
        let (bs, _) = s.in_use(brush, shift);
        let mut negative = !bs.positive && brush.has_polarity();
        if (mods.control_key() || self.input.button == Some(MouseButton::Right))
            && (brush.has_polarity() || s.mask_mode)
        {
            negative = !negative;
        }
        let mut tint = if s.mask_mode {
            if negative { ui::overlay_colors::SUBTRACT } else { theme::MASK }
        } else if brush == Brush::Smooth {
            ui::overlay_colors::SMOOTH
        } else if negative {
            ui::overlay_colors::SUBTRACT
        } else {
            ui::overlay_colors::ADD
        };

        let mut center = self.input.pointer;
        let mut radius = s.radius_px;
        let mut tether = None;
        if let Some((rope, world_radius, sign)) = self.doc.sculptor.stroke_cursor() {
            // During a stroke the world size is frozen; show its true
            // projection where the brush actually is.
            if rope.distance(center) > 1.0 {
                tether = Some(to_points(center));
            }
            center = rope;
            let depth = self.input.hover.map_or(self.view_depth, |h| self.local.depth_of(h));
            radius = world_radius / self.local.world_per_pixel(depth.max(1e-6));
            if sign < 0.0 && brush.has_polarity() {
                tint = ui::overlay_colors::SUBTRACT;
            }
        }
        if let Some(g) = self.input.gauge {
            center = g.anchor;
        }
        // A blink at the end of each stroke confirms it landed.
        let mut alpha = 1.0;
        if let Some(t) = self.stroke_ended {
            let age = t.elapsed().as_secs_f32();
            if age < 0.1 {
                alpha = age / 0.1;
                self.redraw = true;
            } else {
                self.stroke_ended = None;
            }
        }

        let navigating = matches!(self.input.drag, Drag::Orbit | Drag::Pan | Drag::Zoom);
        // The brush ring belongs to sculpting; the SSphere tool shows a ghost
        // sphere in the scene instead.
        let visible = !self.doc.opts.clean_view
            && !self.spheres_active()
            && !self.transform_active()
            && !self.tools_active()
            && self.input.inside
            && self.view.contains(self.input.pointer)
            && !navigating
            && self.ui.radial.is_none()
            && (self.input.drag == Drag::Sculpt || self.input.gauge.is_some() || !self.over_ui());

        // Pivot dot: shown while orbiting about the stroke pivot, then fades.
        let mut pivot = None;
        if let Some(p) = self.anchor().filter(|_| !self.doc.opts.clean_view) {
            if navigating {
                self.pivot_seen = Some(Instant::now());
            }
            if let Some(t) = self.pivot_seen {
                let fade = 1.0 - t.elapsed().as_secs_f32() / 0.35;
                if fade > 0.0 {
                    pivot = self.view.project(p).map(|px| (to_points(px), fade.min(1.0)));
                    self.redraw = true;
                } else {
                    self.pivot_seen = None;
                }
            }
        }

        let inner = if s.mask_mode {
            s.mask_hardness + (1.0 - s.mask_hardness) * 0.5
        } else {
            s.half_weight_radius_in_use(brush, shift)
        };
        let gauge = self.input.gauge.map(|g| g.kind);
        // The number the held key is setting, beside the ring.
        let held = s.current();
        let readout = gauge.map(|kind| match kind {
            GaugeKind::Size => format!("Size {}", ui::brush_size_shown(s)),
            GaugeKind::Strength => {
                let name = if brush == Brush::Paint && !s.mask_mode { "Flow" } else { "Strength" };
                format!("{name} {:.0}%", held.strength * 100.0)
            }
            GaugeKind::Focal => format!("Focal Shift {:.0}", held.focal_shift * 100.0),
        });
        Cursor {
            visible,
            center: to_points(center),
            radius: radius / ppp,
            tint: tint.gamma_multiply(alpha),
            dashed: brush == Brush::Smooth && !s.mask_mode,
            inner,
            inner_bold: gauge == Some(GaugeKind::Focal),
            strength: (gauge == Some(GaugeKind::Strength)).then_some(bs.strength),
            tether,
            pivot,
            alpha: self.alpha_under_cursor(brush),
            readout,
        }
    }

    fn globals(&self) -> Globals {
        let o = &self.doc.opts;
        let v = &self.view;
        // The selected object's guides: the planes of its own symmetry.
        let shown = self.doc.scene.guides();
        let guides = |a: usize| if o.symmetry_guides && !o.clean_view && shown.mirror[a] { 1.0 } else { 0.0 };
        let cavity = |x: f32| if o.cavity { x } else { 0.0 };
        let bg = srgb_to_linear(o.background);
        let top = srgb_to_linear(o.background_top);
        let ppp = self.window.scale_factor() as f32;
        // Studio three-point rig, fixed to the camera so the model's lit side
        // is always the one you are looking at. xyz in view space.
        let light = |d: [f32; 3], c: [f32; 3], i: f32| {
            let c = srgb_to_linear(c);
            ([d[0], d[1], d[2], 0.0], [c[0] * i, c[1] * i, c[2] * i, 0.0])
        };
        let key = light([-0.492, 0.643, 0.587], [1.0, 0.969, 0.922], 1.56);
        let fill = light([0.754, 0.174, 0.633], [0.859, 0.910, 1.0], 0.40);
        let rim = light([0.410, 0.574, -0.709], [1.0, 1.0, 1.0], 1.08);
        let lin = |c: [f32; 3]| {
            let c = srgb_to_linear(c);
            [c[0], c[1], c[2], 0.0]
        };
        let base = srgb_to_linear(o.base_color);
        Globals {
            view_proj: v.view_proj.to_cols_array_2d(),
            view: v.view.to_cols_array_2d(),
            cam_pos: v.pos.extend(1.0).to_array(),
            cam_right: v.right.extend(0.0).to_array(),
            cam_up: v.up.extend(0.0).to_array(),
            cam_forward: v.forward.extend(0.0).to_array(),
            base_color: [base[0], base[1], base[2], o.smoothness],
            bg_bottom: [bg[0], bg[1], bg[2], if o.gradient_background { 1.0 } else { 0.0 }],
            bg_top: [top[0], top[1], top[2], 1.0],
            shading: [
                if o.matcap_on { 1.0 } else { 0.0 },
                if o.flat_shading { 1.0 } else { 0.0 },
                cavity(o.cavity_ridge),
                cavity(o.cavity_valley),
            ],
            symmetry: [guides(0), guides(1), guides(2), ppp.round().clamp(1.0, 3.0)],
            viewport: [
                v.size.x,
                v.size.y,
                if v.ortho_half_height.is_some() { 0.0 } else { v.tan_half_fov },
                v.size.x / v.size.y,
            ],
            light_dir: [key.0, fill.0, rim.0],
            light_color: [key.1, fill.1, rim.1],
            ambient_sky: lin([0.42, 0.44, 0.48]),
            ambient_equator: lin([0.30, 0.30, 0.31]),
            ambient_ground: lin([0.16, 0.15, 0.14]),
            radial_axis: [0.0; 4],
            radial_spoke: [0.0; 4],
            guide_space: shown.space.to_cols_array_2d(),
        }
    }

    /// The radial symmetry guide for the viewport's own picture, when the
    /// guides are showing. It is not part of `globals`: the pictures drawn on
    /// the side (a file's tile, a recording) start from those and show no
    /// guides.
    fn radial_guide(&self) -> ([f32; 4], [f32; 4]) {
        let o = &self.doc.opts;
        let shown = o.symmetry_guides && !o.clean_view;
        self.doc.scene.guides().radial.filter(|_| shown).unwrap_or_default()
    }

    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        self.redraw = false;
        // What the edits since the last frame have left undo to do.
        self.undo_upkeep();
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 {
            return;
        }

        if self.doc.camera.update(dt) {
            self.redraw = true;
        }
        self.recording_step(dt);
        self.sync_view();
        // Says so when something has just replaced a mesh that had subdivision levels or sculpt layers.
        self.levels_watch();
        self.layers_watch();
        self.paint_watch();
        self.groups_watch();
        // The selected object's symmetry is the sculptor's: what the Pose
        // brush's preview goes by between strokes (a stroke asks again).
        if !self.doc.sculptor.is_active() {
            self.doc.scene.hand_symmetry_to(&mut self.doc.sculptor.settings);
        }

        // Sculpt: every pointer sample since the last frame, in order.
        self.dyntopo_demo_frame();
        self.hand_demo_frame();
        let sculpt_start = Instant::now();
        if self.doc.sculptor.is_active() {
            let doc = &mut self.doc;
            for s in self.input.samples.drain(..) {
                doc.sculptor.sample(&mut doc.mesh, &self.local, s);
            }
            doc.sculptor.frame(&mut doc.mesh, &self.local, dt);
        }
        self.input.samples.clear();
        let sculpt_ms = sculpt_start.elapsed().as_secs_f32() * 1000.0;

        self.input.hover = None;
        if self.input.inside
            && !self.doc.opts.clean_view
            && self.doc.tool == Tool::Sculpt
            && self.view.contains(self.input.pointer)
            && !self.over_ui()
            && self.hover_on_active()
        {
            let (o, d) = self.local.ray(self.input.pointer);
            self.input.hover = self.hover_at(o, d);
        }
        // The brush's size on screen and its size on the model, kept in step.
        let depth = self.input.hover.map_or(self.view_depth, |h| self.local.depth_of(h));
        self.doc.sculptor.sync_size(&self.doc.mesh, self.local.world_per_pixel(depth.max(1e-6)));

        // A gizmo drag around a mask reshapes the mesh once a frame.
        self.transform_apply();
        // So does a deformer of the Deform card whose settings have changed (app/deform.rs).
        self.deform_frame();

        // A Decimate or a Remesh that has finished in the background takes the model's place.
        self.decimate_frame();
        self.remesh_frame();

        // The hosted tools' frame: before the interface, which draws their overlay.
        self.tools_frame();

        // Interface.
        self.ui_demo_frame(event_loop);
        // The file's name in the title, starred while there is unsaved work (app/title.rs).
        self.title_frame();
        self.egui_ctx.set_zoom_factor(self.doc.opts.ui_scale);
        let cursor = self.cursor();
        self.pose_frame();
        if cursor.visible || self.recording_hides_pointer() {
            self.egui_ctx.set_cursor_icon(egui::CursorIcon::None);
        }
        let raw_input = self.egui_state.take_egui_input(&self.window);
        let time = self.time();
        let mut actions = Vec::new();
        let (doc, ui_state) = (&mut self.doc, &mut self.ui);
        let output = self.egui_ctx.run_ui(raw_input, |ui| {
            actions = ui::build(ui, doc, ui_state, &cursor, time);
        });
        self.egui_state.handle_platform_output(&self.window, output.platform_output);
        // Texture updates go to the GPU now, before anything below can skip the
        // frame: egui sends each one once (the font atlas arrives this way), so
        // one dropped with the frame is lost for good, and epaint asserts on it
        // in debug builds. Textures to free wait until the frame has drawn.
        let mut textures = output.textures_delta;
        for (id, deltas) in &textures.set {
            for delta in deltas {
                self.egui_renderer.update_texture(&self.gpu.device, &self.gpu.queue, *id, delta);
            }
        }
        let freed = std::mem::take(&mut textures.free);
        textures.clear();
        if let Some(delay) = output.viewport_output.get(&egui::ViewportId::ROOT).map(|v| v.repaint_delay) {
            if delay.is_zero() {
                self.redraw = true;
            } else if delay < Duration::from_secs(3600) {
                let at = now + delay;
                self.repaint_at = Some(self.repaint_at.map_or(at, |t| t.min(at)));
            }
        }
        for action in actions {
            self.act(action);
        }
        self.tools_after_ui();
        // Now and then, writes the settings if they have changed.
        self.files_frame();
        // The steps of a scripted check of recovery, if this run is one.
        self.recovery_frame();

        // The palette packed for the renderer, custom shaders read again
        // if their files changed (app/materials.rs).
        self.materials_frame();
        // Send the mesh changes: a whole mesh if it is new or was replaced,
        // otherwise only the vertex ranges the brush touched.
        let doc = &mut self.doc;
        // Curvature, for the Look's finish or a painted material that reads it.
        let painted_curvature =
            !doc.opts.matcap_on && doc.opts.paint.show_materials && self.renderer.painted.upload().any_curvature();
        self.renderer.want_curvature = doc.opts.look.needs_curvature(doc.opts.matcap_on) || painted_curvature;
        self.renderer.wire.enabled = doc.opts.wireframe;
        self.renderer.groups.enabled = doc.opts.groups.colours;
        if doc.opts.look.animated() {
            // A glow that moves needs every frame.
            self.redraw = true;
        }
        let rebuilt = std::mem::take(&mut doc.mesh_replaced);
        // A Decimate preview is drawn in the model's place; the model itself is not touched (app/decimate.rs).
        match self.decimate.shown_mut() {
            Some(preview) => self.renderer.sync_mesh(&self.gpu, doc.scene.active_mesh(), preview, false),
            None => self.renderer.sync_mesh(&self.gpu, doc.scene.active_mesh(), &mut doc.mesh, rebuilt),
        }
        let shown: Vec<u64> = doc.scene.objects.iter().map(|o| o.mesh).collect();
        for (id, mesh) in doc.scene.parked_mut() {
            if shown.contains(&id) {
                self.renderer.sync_mesh(&self.gpu, id, mesh, false);
            }
        }
        self.renderer.retain_meshes(|id| shown.contains(&id));
        if rebuilt {
            self.sync_view();
        }

        let screenshot = self.args.screenshot.clone().filter(|_| self.frames >= 12);
        let size = (size.width, size.height);
        let capture = self.capture.take();
        let offscreen = (screenshot.is_some() || capture.is_some()).then(|| {
            self.gpu.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("screenshot"),
                size: wgpu::Extent3d { width: size.0, height: size.1, depth_or_array_layers: 1 },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: self.gpu.format,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
                view_formats: &[],
            })
        });
        let surface_texture = if offscreen.is_some() {
            None
        } else {
            match self.gpu.surface.get_current_texture() {
                wgpu::CurrentSurfaceTexture::Success(t) | wgpu::CurrentSurfaceTexture::Suboptimal(t) => Some(t),
                wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                    self.gpu.resize(size.0, size.1);
                    self.redraw = true;
                    return;
                }
                _ => return,
            }
        };
        let target = match (&offscreen, &surface_texture) {
            (Some(t), _) => t.create_view(&Default::default()),
            (None, Some(s)) => s.texture.create_view(&wgpu::TextureViewDescriptor {
                format: Some(self.gpu.format),
                ..Default::default()
            }),
            (None, None) => return,
        };

        self.spheres_frame();
        self.transform_frame();
        self.recording_before_draw();
        let mut encoder = self.gpu.device.create_command_encoder(&Default::default());
        let mut globals = self.globals();
        (globals.radial_axis, globals.radial_spoke) = self.radial_guide();
        self.doc.opts.look.track_camera(self.view.forward, self.view.up);
        let look = self.recording_look(self.look_uniform());
        let scene = &self.doc.scene;
        let items: Vec<DrawItem> = scene
            .objects
            .iter()
            .filter(|o| o.visible && !self.spheres_stand_in_for(o.id) && !self.tools_hide(o.id))
            .map(|o| {
                let active = if o.id == scene.active_id() { 1.0 } else { 0.0 };
                let mut model = o.matrix().to_cols_array_2d();
                model[3][3] = active;
                DrawItem { mesh: o.mesh, model }
            })
            .collect();
        let items = self.tools_copies(items);
        let items = self.mirror_items(items);
        self.recording.items.clone_from(&items);
        // The pulse over a picked, framed or undone object (see render/flash.rs).
        // Not in the Tab "clean" view, nor in the bridge's screenshots.
        let pulses: Vec<flash::Item> = self
            .flash
            .frame(dt, !self.doc.opts.clean_view && capture.is_none())
            .into_iter()
            .flat_map(|(id, color)| self.pulses_of(id, color))
            .collect();
        self.redraw |= self.flash.animating();
        self.renderer.flash.show(&self.gpu, &globals.view_proj, &pulses);
        // A still picture refines (render/refine.rs), but not between the
        // dabs of a stroke: a pause there is not the end of it.
        let quality = &self.doc.opts.quality;
        self.renderer.refine_to = if self.doc.sculptor.is_active() { 1 } else { quality.refine_samples() };
        self.renderer.steady = quality.steady;
        self.renderer.draw(&self.gpu, &mut encoder, &target, size, &globals, &look, &self.doc.opts, &items);
        self.redraw |= self.renderer.refining();

        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [size.0, size.1],
            pixels_per_point: output.pixels_per_point,
        };
        let primitives = self.egui_ctx.tessellate(output.shapes, output.pixels_per_point);
        let mut commands = self.egui_renderer.update_buffers(
            &self.gpu.device,
            &self.gpu.queue,
            &mut encoder,
            &primitives,
            &screen,
        );
        {
            let mut pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("interface"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &target,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations { load: wgpu::LoadOp::Load, store: wgpu::StoreOp::Store },
                    })],
                    ..Default::default()
                })
                .forget_lifetime();
            self.egui_renderer.render(&mut pass, &primitives, &screen);
        }
        for id in &freed {
            self.egui_renderer.free_texture(id);
        }
        commands.push(encoder.finish());
        self.gpu.queue.submit(commands);

        if let (Some(texture), Some(capture)) = (&offscreen, capture) {
            capture.finish(crate::render::read_texture(&self.gpu, texture), &mut self.doc.opts);
            // This frame went to the bridge, not the window: draw it again.
            self.redraw = true;
            return;
        }
        if let (Some(texture), Some(path)) = (&offscreen, &screenshot) {
            let image = crate::render::read_texture(&self.gpu, texture);
            if let Err(e) = image.save(path) {
                log::error!("could not save screenshot: {e}");
            }
            event_loop.exit();
            return;
        }
        if let Some(surface_texture) = surface_texture {
            self.window.pre_present_notify();
            self.gpu.queue.present(surface_texture);
        }
        // Pictures for a 360 loop or a timelapse, now the window's is out.
        self.recording_capture(event_loop);
        // The Materials card's thumbnails, each its own batch of work.
        self.materials_thumbs();

        self.frames += 1;
        let frame_ms = now.elapsed().as_secs_f32() * 1000.0;
        let stats = &mut self.doc.stats;
        stats.frame_ms += (frame_ms - stats.frame_ms) * 0.1;
        stats.sculpt_ms += (sculpt_ms - stats.sculpt_ms) * 0.1;
        stats.upload_kb = self.renderer.last_upload_bytes as f32 / 1024.0;
    }

    /// One of each shape in two rows, then an instance of the first; used
    /// with `--screenshot` to check the object list and multi-object drawing.
    fn demo_scene(&mut self) {
        for (i, kind) in Primitive::ALL.into_iter().enumerate() {
            self.act(Action::New(kind));
            let place = Vec3::new((i % 3) as f32 * 1.3 - 1.3, 0.65 - (i / 3) as f32 * 1.3, 0.0);
            self.doc.scene.active_mut().position = place;
        }
        let first = self.doc.scene.objects[0].id;
        self.act(Action::ToggleVisible(first));
        self.act(Action::SelectObject(self.doc.scene.objects[1].id));
        for depth in [1.4, 2.8] {
            self.act(Action::InstanceObject);
            self.doc.scene.active_mut().position.z -= depth;
        }
        let bounds = self.doc.scene.bounds(&self.doc.mesh);
        self.doc.camera.frame(bounds, self.view.size.x / self.view.size.y.max(1.0), false);
        self.sync_view();
    }

    /// A few scripted strokes, used with `--screenshot` to check the brushes
    /// by eye without a person at the tablet.
    fn demo_strokes(&mut self) {
        self.sync_view();
        let size = self.view.size;
        let c = size * 0.5;
        let k = size.y / 900.0;
        let strokes: [(Brush, f32, f32, Vec2, Vec2, f32); 6] = [
            (Brush::Clay, 0.5, 34.0, Vec2::new(-170.0, -120.0), Vec2::new(-60.0, -60.0), 20.0),
            (Brush::Standard, 0.4, 30.0, Vec2::new(-170.0, -30.0), Vec2::new(-60.0, 10.0), -14.0),
            (Brush::Crease, 0.8, 26.0, Vec2::new(-175.0, 60.0), Vec2::new(-55.0, 80.0), 10.0),
            (Brush::Inflate, 0.4, 26.0, Vec2::new(-150.0, 150.0), Vec2::new(-70.0, 160.0), 0.0),
            (Brush::Layer, 0.8, 24.0, Vec2::new(-40.0, -170.0), Vec2::new(-25.0, -110.0), 0.0),
            (Brush::SnakeHook, 0.5, 40.0, Vec2::new(-150.0, -175.0), Vec2::new(-210.0, -260.0), 0.0),
        ];
        self.demo_symmetry(Some([true, false, false]));
        for (brush, strength, radius, from, to, bend) in strokes {
            let s = &mut self.doc.sculptor.settings;
            s.brush = brush;
            s.radius_px = radius * k;
            s.per_brush[brush.index()].strength = strength;
            let n = 48;
            let point = |i: usize| {
                let t = i as f32 / n as f32;
                c + (from.lerp(to, t) + Vec2::new(0.0, (t * std::f32::consts::PI).sin() * bend)) * k
            };
            let sample = |i: usize| PointerSample { pos: point(i), pressure: None, time: i as f64 / 240.0 };
            let doc = &mut self.doc;
            if !doc.begin_stroke(&self.local, sample(0), false, false) {
                continue;
            }
            for i in 1..=n {
                doc.sculptor.sample(&mut doc.mesh, &self.local, sample(i));
                if i % 4 == 0 {
                    doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
                }
            }
            if let Some(delta) = doc.sculptor.end(&mut doc.mesh, &self.local) {
                doc.history.push_stroke(delta);
            }
        }
        self.doc.sculptor.settings.brush = Brush::Clay;
        self.doc.sculptor.settings.radius_px = 60.0;
        self.doc.sculptor.settings.demo_setup(true);
        // Strokes repeated round an axis are looked at down that axis, where
        // every repeat shows.
        let radial = self.doc.scene.active_symmetry().radial;
        let down_the_axis = match radial.axis {
            crate::sculpt::RadialAxis::X => Some((-90.0, 0.0)),
            crate::sculpt::RadialAxis::Y => Some((0.0, 90.0)),
            crate::sculpt::RadialAxis::Z => Some((0.0, 0.0)),
            crate::sculpt::RadialAxis::Custom => None,
        };
        if let Some((yaw, pitch)) = down_the_axis.filter(|_| radial.on) {
            (self.doc.camera.yaw, self.doc.camera.pitch) = (yaw, pitch);
            self.sync_view();
        }
    }
}

/// The number key that picks a brush, for the brushes that have one.
fn brush_key(brush: Brush) -> Option<KeyCode> {
    Some(match brush.hotkey()? {
        '1' => KeyCode::Digit1,
        '2' => KeyCode::Digit2,
        '3' => KeyCode::Digit3,
        '4' => KeyCode::Digit4,
        '5' => KeyCode::Digit5,
        '6' => KeyCode::Digit6,
        '7' => KeyCode::Digit7,
        '8' => KeyCode::Digit8,
        '9' => KeyCode::Digit9,
        '0' => KeyCode::Digit0,
        _ => return None,
    })
}

/// Whether `path` is a whole scene: this app's `.sclp`, or a `.sculpt` from
/// the Unity app, which opens the same way (see `unity_import.rs`).
fn is_native(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("sclp"))
        || crate::unity_import::is_unity(path)
}

/// A file's name without its extension, for naming the object it becomes.
fn stem(path: &std::path::Path) -> String {
    path.file_stem().map_or("Model".into(), |s| s.to_string_lossy().into_owned())
}

/// Reads a model file and returns its mesh with the position its object
/// should be given (see [`io::place_import`]). `first_half_size` is the half
/// size of the scene's first object, which an OBJ is fitted to.
fn import_mesh(path: &std::path::Path, first_half_size: Option<Vec3>) -> io::Result<(Mesh, Vec3)> {
    let mut raw = io::import(path)?;
    let at = io::place_import(&mut raw, first_half_size);
    Ok((io::imported_mesh(raw), at))
}

/// Puts a loaded file's objects in place of the scene's. Gives back what
/// else the file kept (its look, view and tool state) for the app to apply.
fn install(scene: &mut Scene, slot: &mut Mesh, saved: io::SavedScene) -> io::Details {
    let objects = saved
        .objects
        .into_iter()
        .map(|o| Object {
            id: 0,
            name: o.name,
            visible: o.visible,
            mesh: o.mesh as u64,
            position: o.position,
            rotation: o.rotation,
            scale: o.scale,
            mirror: None,
        })
        .collect();
    scene.load(saved.meshes, objects, saved.active, slot);
    saved.details
}

/// Windows shows a "press and hold for right-click" ring under a resting pen
/// and treats quick strokes as flick gestures. Both get in the way of
/// sculpting, so they are switched off for this window.
#[cfg(windows)]
fn disable_pen_gestures(window: &Window) {
    use winit::raw_window_handle::{HasWindowHandle, RawWindowHandle};

    #[link(name = "user32")]
    unsafe extern "system" {
        fn SetPropW(hwnd: isize, name: *const u16, data: isize) -> i32;
    }
    const DISABLE_PRESS_AND_HOLD: isize = 0x0000_0001;
    const DISABLE_PEN_TAP_FEEDBACK: isize = 0x0000_0008;
    const DISABLE_PEN_BARREL_FEEDBACK: isize = 0x0000_0010;
    const DISABLE_FLICKS: isize = 0x0001_0000;

    let Ok(handle) = window.window_handle() else { return };
    let RawWindowHandle::Win32(handle) = handle.as_raw() else { return };
    let name: Vec<u16> = "MicrosoftTabletPenServiceProperty\0".encode_utf16().collect();
    let flags = DISABLE_PRESS_AND_HOLD | DISABLE_PEN_TAP_FEEDBACK | DISABLE_PEN_BARREL_FEEDBACK | DISABLE_FLICKS;
    // SAFETY: `hwnd` is this live window's handle and `name` is a valid
    // NUL-terminated UTF-16 string that outlives the call.
    unsafe {
        SetPropW(handle.hwnd.get(), name.as_ptr(), flags);
    }
}

#[cfg(not(windows))]
fn disable_pen_gestures(_: &Window) {}

#[cfg(test)]
mod tests {
    use super::*;

    /// A review point about the recovery copies: with a frame asked for, the
    /// loop waited for it alone, and a minimised window never draws one.
    #[test]
    fn a_frame_that_never_comes_does_not_hold_up_the_look_at_whether_a_copy_is_due() {
        let now = Instant::now();
        let (soon, later) = (now + Duration::from_millis(200), now + Duration::from_secs(180));
        // A frame has been asked for, as it stays while the window is minimised.
        assert_eq!(wait_for(true, None, Some(later)), ControlFlow::WaitUntil(later), "the copy is still looked at");
        assert_eq!(wait_for(true, Some(soon), Some(later)), ControlFlow::WaitUntil(later));
        assert_eq!(wait_for(true, Some(soon), None), ControlFlow::Wait, "the frame asked for is that repaint");
        assert_eq!(wait_for(true, None, None), ControlFlow::Wait);
        // None asked for: whichever comes first.
        assert_eq!(wait_for(false, Some(soon), Some(later)), ControlFlow::WaitUntil(soon));
        assert_eq!(wait_for(false, Some(later), Some(soon)), ControlFlow::WaitUntil(soon));
        assert_eq!(wait_for(false, None, Some(later)), ControlFlow::WaitUntil(later));
        assert_eq!(wait_for(false, Some(soon), None), ControlFlow::WaitUntil(soon));
        assert_eq!(wait_for(false, None, None), ControlFlow::Wait, "nothing is wanted: until the next event");
    }
}
