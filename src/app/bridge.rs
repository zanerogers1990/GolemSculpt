//! The Claude bridge, app side: a loopback socket that lets a Claude session
//! see and drive the running app (state, screenshots, strokes, camera, files).
//!
//! Claude itself talks to `sculpt.exe --mcp` (see `crate::mcp`), which passes
//! each tool call here as one line of JSON and gets one line back. Requests
//! are handed to the main thread and run between frames, through the same
//! code paths the interface uses, so what Claude does is undoable like
//! anything else.
//!
//! Who may connect is `crate::bridge_guard`'s business: every run makes a new
//! key, leaves it in a file in the settings folder for Claude's server to
//! read, and runs nothing for a connection that has not shown it as its
//! first line. A run with no settings folder (a scripted one that was not
//! given `SCULPT_SETTINGS_DIR`) has nowhere to leave the key, so it has no
//! bridge. Each connection has a thread of its own, so one that says nothing
//! holds nobody else up. There is a limit on how many may be waiting to show
//! the key and another on how many have shown it, kept apart so that no crowd
//! of the first kind can keep Claude out (`bridge_guard::Door`).
//!
//! What the bridge will not do behind the user's back: close the app, or
//! open a scene in place of the one on screen, while there is unsaved work
//! or work that is in no file and would go with it (SSpheres, unmerged tubes
//! and the like: `app/unsaved.rs`). `quit` and `open_file` say so, naming
//! that work, and do nothing unless told to `discard`. Nor will it touch a
//! path that is not on this computer's own drives.
//!
//! Screen positions are fractions of the window: (0, 0) is the top-left
//! corner and (1, 1) the bottom-right, whatever size a screenshot was sent at.

use std::io::Cursor;
use std::net::{Ipv4Addr, TcpListener};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use glam::{Vec2, Vec3};
use sculpt_toolkit::Loss;
use serde_json::{Value, json};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};

use super::{Drag, Running, is_native};
use crate::bridge_guard::{self as guard, Key};
use crate::doc::{Action, MaskOp, Options};
use crate::io;
use crate::mesh::bvh::Aabb;
use crate::mesh::primitives::{self, Primitive};
use crate::scene::Space;
use crate::sculpt::alpha::{self, AlphaSettings};
use crate::sculpt::cavity;
use crate::sculpt::{BrushSettings, FlattenMode, PointerSample, PoseMode, PoseSettings, Radial, RadialAxis, SmoothMode};
use crate::ui::question::listed;
use crate::ui::theme;

mod brushes;

/// How long a connection waits for the main thread. A remesh of a heavy mesh
/// holds it for a while.
const REPLY_TIMEOUT: Duration = Duration::from_secs(120);
/// Spacing of the pointer samples a scripted stroke is fed as; about what a
/// pen reports.
const STROKE_STEP_PX: f32 = 3.0;
const MAX_STROKE_SAMPLES: usize = 20_000;
const DEFAULT_SHOT_SIZE: u32 = 1280;

pub struct Reply {
    ok: bool,
    text: String,
    /// A PNG to show alongside the text.
    image: Option<Vec<u8>>,
}

impl Reply {
    fn from(result: Result<String, String>) -> Reply {
        let ok = result.is_ok();
        Reply { ok, text: result.unwrap_or_else(|e| e), image: None }
    }
}

struct Request {
    tool: String,
    args: Value,
    reply: Sender<Reply>,
}

pub struct Bridge {
    requests: Receiver<Request>,
    /// This run's key in its file. Dropped with the bridge when the app
    /// closes, which removes the file.
    _key: guard::Posted,
}

/// A screenshot waiting for the next frame to be drawn.
pub struct Capture {
    reply: Sender<Reply>,
    max_size: u32,
    /// The panels were hidden for this shot; what to set the option back to.
    clean_view: Option<bool>,
}

/// Starts listening. `wake` rouses the event loop, which sleeps while nothing
/// is happening. `folder` is this run's settings folder, where the key goes.
/// Returns `None` if there is no such folder, or if the port is taken, which
/// is what happens when a second copy of the app is opened: the first keeps
/// the bridge, and its key file is left alone.
pub fn start(wake: EventLoopProxy<()>, folder: Option<&Path>) -> Option<Bridge> {
    let port = crate::mcp::port();
    let Some(folder) = folder else {
        log::info!("the Claude bridge is off: this run keeps no settings folder to leave its key in");
        return None;
    };
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(listener) => listener,
        Err(e) => {
            log::warn!("the Claude bridge is off (port {port}: {e})");
            return None;
        }
    };
    // The port is ours, so the file is ours to replace: only now is the key
    // written, and only once it is do connections get an answer.
    let key = Key::new().map_err(|e| e.to_string());
    let (posted, key) = match key.and_then(|key| Ok((guard::Posted::post(folder, port, &key)?, key))) {
        Ok(both) => both,
        Err(e) => {
            log::warn!("the Claude bridge is off ({e})");
            return None;
        }
    };
    let (tx, rx) = mpsc::channel();
    let door = guard::Door::new(guard::MAX_WAITING, guard::MAX_ADMITTED);
    let run = move |request| answer(request, &tx, &wake);
    std::thread::spawn(move || guard::listen(listener, key, door, guard::HELLO_WAIT, run));
    Some(Bridge { requests: rx, _key: posted })
}

/// Hands one request to the main thread and waits for what it says: the
/// line to send back. `None` once the app is closing.
fn answer(request: guard::Request, requests: &Sender<Request>, wake: &EventLoopProxy<()>) -> Option<String> {
    let (tx, rx) = mpsc::channel();
    let request = Request { tool: request.tool, args: request.args, reply: tx };
    if requests.send(request).is_err() || wake.send_event(()).is_err() {
        return None;
    }
    let reply =
        rx.recv_timeout(REPLY_TIMEOUT).unwrap_or_else(|_| Reply::from(Err("The app did not answer in time.".into())));
    let mut message = json!({ "ok": reply.ok, "text": reply.text });
    if let Some(png) = reply.image {
        message["image"] = base64(&png).into();
    }
    Some(message.to_string())
}

fn base64(bytes: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let n = (chunk[0] as u32) << 16
            | (*chunk.get(1).unwrap_or(&0) as u32) << 8
            | *chunk.get(2).unwrap_or(&0) as u32;
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(TABLE[(n >> (18 - 6 * i)) as usize & 63] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

impl Bridge {
    /// Runs every request that has arrived. Main thread only.
    pub fn pump(&self, app: &mut Running, event_loop: &ActiveEventLoop) {
        while let Ok(request) = self.requests.try_recv() {
            app.redraw = true;
            match request.tool.as_str() {
                // Answered by the next frame, once it has been drawn.
                "screenshot" => app.request_capture(request),
                "quit" => {
                    let outcome = app.quit_for_claude(&request.args);
                    let closing = outcome.is_ok();
                    let _ = request.reply.send(Reply::from(outcome));
                    if closing {
                        event_loop.exit();
                    }
                }
                tool => {
                    let result = app.call(tool, &request.args);
                    let _ = request.reply.send(Reply::from(result));
                }
            }
        }
    }
}

impl Capture {
    /// Sends the drawn frame back, scaled down to the size that was asked for.
    pub fn finish(self, image: image::RgbaImage, opts: &mut Options) {
        if let Some(clean_view) = self.clean_view {
            opts.clean_view = clean_view;
        }
        let (width, height) = image.dimensions();
        let scale = (self.max_size as f32 / width.max(height) as f32).min(1.0);
        let mut shot = image::DynamicImage::ImageRgba8(image).to_rgb8();
        if scale < 1.0 {
            let fit = |v: u32| ((v as f32 * scale).round() as u32).max(1);
            shot = image::imageops::resize(&shot, fit(width), fit(height), image::imageops::FilterType::Triangle);
        }
        let mut png = Vec::new();
        let reply = match shot.write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png) {
            Ok(()) => Reply {
                ok: true,
                text: format!(
                    "The {width}x{height} window, sent at {}x{}. Positions for stroke and pick are \
                     fractions of this picture: [0, 0] is its top-left corner, [1, 1] its bottom-right.",
                    shot.width(),
                    shot.height()
                ),
                image: Some(png),
            },
            Err(e) => Reply::from(Err(format!("Could not encode the screenshot: {e}"))),
        };
        let _ = self.reply.send(reply);
    }
}

/// Lower case with everything but letters and digits dropped, so "Snake Hook",
/// "snake_hook" and "snakehook" all match.
fn key(name: &str) -> String {
    name.chars().filter(char::is_ascii_alphanumeric).map(|c| c.to_ascii_lowercase()).collect()
}

fn num(args: &Value, name: &str) -> Option<f32> {
    let value = args.get(name)?;
    value.as_f64().or_else(|| value.as_str()?.trim().parse().ok()).map(|v| v as f32)
}

fn flag(args: &Value, name: &str) -> Option<bool> {
    args.get(name)?.as_bool()
}

fn text<'a>(args: &'a Value, name: &str) -> Option<&'a str> {
    args.get(name)?.as_str()
}

fn vec3(args: &Value, name: &str) -> Result<Option<Vec3>, String> {
    let Some(value) = args.get(name).filter(|v| !v.is_null()) else { return Ok(None) };
    let part = |i: usize| value.get(i).and_then(Value::as_f64).map(|v| v as f32);
    match (part(0), part(1), part(2)) {
        (Some(x), Some(y), Some(z)) => Ok(Some(Vec3::new(x, y, z))),
        _ => Err(format!("{name} must be three numbers: [x, y, z].")),
    }
}

/// Four decimals: plenty for a reader, and no float noise in the output.
fn round(v: f32) -> f64 {
    (v as f64 * 1e4).round() / 1e4
}

fn point(v: Vec3) -> [f64; 3] {
    [round(v.x), round(v.y), round(v.z)]
}

/// What `open_file` says instead of opening a sculpt over unsaved work.
const REPLACING_UNSAVED: &str = "The scene has unsaved work, and opening this file would put the scene aside. Nothing \
was opened. Ask the user what to do with the work first. Then either save it (save_file with a .sclp path) and open \
again, or, if the user says it can go, call open_file again with discard: true.";

/// What `quit` says instead of closing the app in the middle of a stroke.
const STROKE_UNDER_WAY: &str = "Someone is in the middle of a stroke or drag in the app, so Sculpt was not closed. \
Try again in a moment.";

/// What a `quit` from Claude comes to.
#[derive(Debug, PartialEq)]
enum Leave {
    Close,
    /// Save the scene to this file, then close.
    SaveFirst(PathBuf),
    /// Stay open, and say this.
    Stay(String),
}

/// What a `quit` is called with, and how things stand when it is.
struct Leaving<'a> {
    /// A stroke or drag is under way in the app.
    mid_stroke: bool,
    /// The scene differs from what its file holds.
    unsaved: bool,
    /// The work that is in no file and that closing would lose
    /// (`Running::scaffolds_at_risk`), a phrase for each kind.
    lost: &'a [String],
    /// Where Save would write the scene, if it has such a file.
    file: Option<&'a Path>,
    save: bool,
    discard: bool,
}

/// Decides a `quit`. Work is never dropped on the quiet. A stroke under way
/// is work that is in no undo step yet and that no flag speaks for, so the
/// app stays open whatever the call says. With unsaved work it stays open
/// unless the call says which is wanted, saving or discarding, and saying
/// both is saying neither. With work that is in no file (the interface's
/// own question names it before closing) saving is no answer, since a save
/// does not keep it: the app stays open and names that work, and closes
/// only on `discard`.
fn leave(call: Leaving) -> Leave {
    const ASK: &str = "The scene has unsaved work, so Sculpt was not closed. Ask the user what to do with it first.";
    let Leaving { mid_stroke, unsaved, lost, file, save, discard } = call;
    if mid_stroke {
        return Leave::Stay(STROKE_UNDER_WAY.into());
    }
    if !unsaved && lost.is_empty() {
        return Leave::Close;
    }
    if !lost.is_empty() {
        return match (save, discard) {
            (false, true) => Leave::Close,
            _ => Leave::Stay(lost_by_closing(unsaved, lost, save && discard)),
        };
    }
    match (save, discard, file) {
        (true, true, _) => Leave::Stay(format!("{ASK} Then call quit with save: true or with discard: true, not both.")),
        (false, true, _) => Leave::Close,
        (true, false, Some(file)) => Leave::SaveFirst(file.to_owned()),
        (true, false, None) => Leave::Stay(format!(
            "{ASK} It has no .sclp file of its own to save to: call save_file with a full path ending in .sclp, \
             then quit."
        )),
        (false, false, Some(file)) => Leave::Stay(format!(
            "{ASK} Then call quit with save: true to save it to {} and close, or with discard: true to close \
             without saving.",
            file.display()
        )),
        (false, false, None) => Leave::Stay(format!(
            "{ASK} Then call save_file with a full path ending in .sclp and quit again, or call quit with \
             discard: true to close without saving."
        )),
    }
}

/// What `quit` says instead of closing over work that is in no file: what
/// that work is, that saving does not keep it, and what to call next. `both`
/// is whether the call said `save` and `discard` at once.
fn lost_by_closing(unsaved: bool, lost: &[String], both: bool) -> String {
    let named = listed(lost);
    let stays = format!(
        "Closing Sculpt would lose work that is in no file, which saving does not keep: {named}. Sculpt was not \
         closed. Ask the user what to do with it first: it can be finished in the app, as each says."
    );
    let then = match (unsaved, both) {
        (_, true) => "Then call quit with discard: true alone to close and lose it, not with save as well.",
        (false, false) => "If the user says it can go, call quit again with discard: true.",
        (true, false) => {
            "The scene has unsaved changes as well. To keep those, save the scene first (save_file with a path \
             ending in .sclp). Then, if the user says the rest can go, call quit with discard: true."
        }
    };
    format!("{stays} {then}")
}

/// What `open_file` says instead of opening a sculpt over work that is in no
/// file and stands on the scene's objects.
fn lost_by_opening(unsaved: bool, lost: &[String]) -> String {
    let named = listed(lost);
    let also =
        if unsaved { " The scene has unsaved changes as well, which save_file with a .sclp path keeps." } else { "" };
    format!(
        "Opening this file would put the scene aside and lose work that is in no file, which saving does not \
         keep: {named}. Nothing was opened. Ask the user what to do with it first: it can be finished in the app, \
         as each says.{also} If the user says it can go, call open_file again with discard: true."
    )
}

/// Decides an `open_file`: whether the file may be opened as things stand,
/// or what to say instead. `takes_the_scene_s_place` is whether it is a
/// sculpt, which is opened in place of the scene, and not a model, which is
/// added to it. `lost` is the work in no file that replacing the scene would
/// cost (`Loss::SceneReplaced`). The interface asks before a sculpt puts
/// unsaved work aside, or takes such work with the scene; here there is
/// nobody to ask but Claude, which is told to ask the user and to say
/// `discard` once the user has agreed.
fn may_open(takes_the_scene_s_place: bool, unsaved: bool, lost: &[String], discard: bool) -> Result<(), String> {
    if !takes_the_scene_s_place || discard {
        return Ok(());
    }
    match (unsaved, lost.is_empty()) {
        (false, true) => Ok(()),
        (true, true) => Err(REPLACING_UNSAVED.into()),
        (_, false) => Err(lost_by_opening(unsaved, lost)),
    }
}

/// What `save_file` says of a sculpt it saved, and `quit` of the one it
/// saved on the way out (`closing`). `note` is what the interface's own Save
/// adds when damaged points were written mended (`Running::saved_note`):
/// nothing, or a sentence that begins with its own full stop.
fn saved_words(path: &Path, note: &str, closing: bool) -> String {
    let saved = format!("Saved {}{note}", path.display());
    if !closing {
        return saved;
    }
    let stop = if note.is_empty() { "." } else { "" };
    format!("{saved}{stop} Sculpt is closing.")
}

impl Running {
    /// Whether a stroke or a drag is under way in the app. Nothing of
    /// Claude's is run across one (`get_state` apart, which changes nothing).
    fn mid_stroke(&self) -> bool {
        self.doc.sculptor.is_active() || self.input.drag != Drag::None
    }

    fn request_capture(&mut self, request: Request) {
        let size = self.window.inner_size();
        if size.width == 0 || size.height == 0 || self.window.is_minimized() == Some(true) {
            let message = "The Sculpt window is minimised, so there is nothing to see. Restore it first.";
            let _ = request.reply.send(Reply::from(Err(message.into())));
            return;
        }
        // Only the newest request is answered.
        if let Some(clean_view) = self.capture.take().and_then(|old| old.clean_view) {
            self.doc.opts.clean_view = clean_view;
        }
        let panels = flag(&request.args, "panels").unwrap_or(true);
        let clean_view = (!panels).then(|| std::mem::replace(&mut self.doc.opts.clean_view, true));
        let max_size = num(&request.args, "max_size").map_or(DEFAULT_SHOT_SIZE, |v| v.clamp(64.0, 8192.0) as u32);
        self.capture = Some(Capture { reply: request.reply, max_size, clean_view });
    }

    /// Runs one of the bridge's tools. (Also called by the scripted checks,
    /// which try a tool without a socket.)
    pub(super) fn call(&mut self, tool: &str, args: &Value) -> Result<String, String> {
        if tool == "get_state" {
            return Ok(self.state().to_string());
        }
        if self.mid_stroke() {
            return Err("Someone is in the middle of a stroke or drag in the app. Try again in a moment.".into());
        }
        match tool {
            "set_brush" => self.set_brush(args),
            "stroke" => self.stroke(args),
            "set_camera" => self.set_camera(args),
            "pick" => self.pick_at(args),
            "command" => self.command(args),
            "add_shape" => self.add_shape(args),
            "select_object" => {
                let id = self.object(args)?;
                // As Ctrl + click on a row: one more in the selection, the active object as it was.
                if flag(args, "add").unwrap_or(false) {
                    self.act(Action::SelectAlso(id));
                    let scene = &self.doc.scene;
                    let (count, name) = (scene.selected().len(), &scene.active().name);
                    return Ok(format!("{count} objects are selected. \"{name}\" is the active object."));
                }
                self.act(Action::SelectObject(id));
                Ok(format!("\"{}\" is the active object.", self.doc.scene.active().name))
            }
            "edit_object" => self.edit_object(args),
            "delete_object" => self.delete_object(args),
            "open_file" => self.open_file(args),
            "save_file" => self.save_file(args),
            "set_options" => self.set_options(args),
            other => Err(format!("Sculpt has no tool called \"{other}\".")),
        }
    }

    /// `quit`: closes the app, unless that would lose work. `Ok` means the
    /// app is to close now. With unsaved work it closes only if told to
    /// `discard` it, or to `save` it first; saving goes to the scene's own
    /// file and never puts up a dialog (nobody may be at the screen). With
    /// work that is in no file it closes only if told to `discard`. These
    /// are the two things the close button's own question is asked about
    /// (`question_before_closing`).
    fn quit_for_claude(&mut self, args: &Value) -> Result<String, String> {
        // Save never writes over a sculpt from the Unity app (see `save_action`).
        let file = self.doc.path.clone().filter(|path| !crate::unity_import::is_unity(path));
        let (save, discard) = (flag(args, "save").unwrap_or(false), flag(args, "discard").unwrap_or(false));
        let lost = self.scaffolds_at_risk(Loss::Closing);
        let (mid_stroke, unsaved) = (self.mid_stroke(), self.unsaved());
        match leave(Leaving { mid_stroke, unsaved, lost: &lost, file: file.as_deref(), save, discard }) {
            Leave::Close => Ok("Sculpt is closing.".into()),
            Leave::SaveFirst(path) => {
                self.save_scene(&path)?;
                Ok(saved_words(&path, &self.saved_note(), true))
            }
            Leave::Stay(why) => Err(why),
        }
    }

    /// The object an `object` argument names, by id or by name; the active
    /// one if there is no such argument.
    fn object(&self, args: &Value) -> Result<u64, String> {
        let scene = &self.doc.scene;
        let Some(wanted) = args.get("object").filter(|v| !v.is_null()) else { return Ok(scene.active_id()) };
        let found = match wanted.as_u64() {
            Some(id) => scene.get(id),
            None => {
                let name = wanted.as_str().unwrap_or_default();
                scene.objects.iter().find(|o| o.name.eq_ignore_ascii_case(name))
            }
        };
        found.map(|o| o.id).ok_or_else(|| format!("There is no object {wanted}. get_state lists the objects."))
    }

    /// Runs something that reports through the status line, and returns what
    /// it reported.
    fn reported(&mut self, run: impl FnOnce(&mut Running)) -> Result<String, String> {
        let before = std::mem::take(&mut self.ui.status);
        run(self);
        if self.ui.status.0.is_empty() {
            self.ui.status = before;
            return Ok("Done.".into());
        }
        let (text, colour) = self.ui.status.clone();
        if colour == theme::ERROR { Err(text) } else { Ok(text) }
    }

    /// Where a world box lands in the window, as [left, top, right, bottom]
    /// fractions. Null if it is not wholly in front of the camera.
    fn on_screen(&self, bounds: Aabb) -> Value {
        if bounds.is_empty() {
            return Value::Null;
        }
        let (mut low, mut high) = (Vec2::MAX, Vec2::MIN);
        for corner in 0..8 {
            let pick = |bit: usize, axis: usize| if corner >> bit & 1 == 0 { bounds.min[axis] } else { bounds.max[axis] };
            let Some(px) = self.view.project(Vec3::new(pick(0, 0), pick(1, 1), pick(2, 2))) else {
                return Value::Null;
            };
            let uv = px / self.view.size;
            low = low.min(uv);
            high = high.max(uv);
        }
        json!([round(low.x), round(low.y), round(high.x), round(high.y)])
    }

    fn brush_state(&self) -> Value {
        let s = &self.doc.sculptor.settings;
        let b = s.current();
        // The active object's own symmetry, which is the one strokes are made under.
        let symmetry = self.doc.scene.active_symmetry();
        let mut state = json!({
            "name": s.brush.name(),
            "radius": round(s.radius_px / self.view.size.y.max(1.0)),
            "radius_px": round(s.radius_px),
            "strength": round(b.strength),
            "focal_shift": round(b.focal_shift),
            "direction": if b.positive { "add" } else { "subtract" },
            "accumulate": b.accumulate,
            "spacing": round(b.spacing),
            "scatter": round(b.scatter),
            "size_jitter": round(b.size_jitter),
            "pressure_size": round(b.pressure_size),
            "pressure_strength": round(b.pressure_strength),
            "smooth_mode": b.smooth_mode.name().to_ascii_lowercase(),
            "keep_volume": round(b.keep_volume),
            "flatten_mode": b.flatten_mode.name().to_ascii_lowercase(),
            "lock_plane": b.lock_plane,
            "alpha": s.alpha[s.brush.index()].image.map(|image| image.name.as_str()),
            "alpha_random_rotation": round(s.alpha[s.brush.index()].random_rotation),
            "mask_mode": s.mask_mode,
            "mirror": { "x": symmetry.mirror[0], "y": symmetry.mirror[1], "z": symmetry.mirror[2] },
            "radial": {
                "on": symmetry.radial.on,
                "repeats": symmetry.radial.count,
                "axis": point(symmetry.radial.axis_vector()),
            },
            "symmetry_space": symmetry.space.name().to_ascii_lowercase(),
            "pose": {
                "mode": s.pose.mode.name(),
                "joints": s.pose.joints,
                "offset": round(s.pose.offset),
                "rigidity": round(s.pose.rigidity),
            },
        });
        // The brush as one of the library's: its own name, the rest of its
        // record, and the names there are to pick from.
        self.bridge_brush_state(&mut state);
        state
    }

    fn camera_state(&self) -> Value {
        let c = &self.doc.camera;
        json!({
            "yaw": round(c.yaw),
            "pitch": round(c.pitch),
            "distance": round(c.distance),
            "pivot": point(c.pivot),
            "orthographic": c.ortho,
        })
    }

    fn state(&mut self) -> Value {
        self.sync_view();
        let doc = &self.doc;
        let objects: Vec<Value> = doc
            .scene
            .objects
            .iter()
            .map(|o| {
                let mesh = doc.scene.mesh(o.mesh, &doc.mesh);
                let bounds = doc.scene.bounds_of(o, &doc.mesh);
                let size = if bounds.is_empty() { Vec3::ZERO } else { bounds.size() };
                json!({
                    "id": o.id,
                    "name": o.name,
                    "active": o.id == doc.scene.active_id(),
                    "selected": doc.scene.is_selected(o.id),
                    "visible": o.visible,
                    "position": point(o.position),
                    "size": point(size),
                    "triangles": mesh.map_or(0, |m| m.triangle_count()),
                    "vertices": mesh.map_or(0, |m| m.vertex_count()),
                    "instances": doc.scene.users(o.mesh),
                    "mirror": o.mirror.map(|m| sculpt_symmetry::MirrorRepeater::axis_name_of(m.axis_mask)),
                    "symmetry": doc.scene.symmetry(o.id).summary(),
                    "levels": super::levels::state_of(mesh),
                    "layers": super::layers::state_of(mesh),
                    "groups": super::groups::state_of(&doc.scene, &doc.opts.groups, o, mesh),
                    "on_screen": if o.visible { self.on_screen(bounds) } else { Value::Null },
                })
            })
            .collect();
        let (undo, redo) = doc.history.steps();
        json!({
            "window": [self.view.size.x, self.view.size.y],
            "file": doc.path.as_ref().map(|p| p.display().to_string()),
            "objects": objects,
            "brush": self.brush_state(),
            "camera": self.camera_state(),
            "undo_steps": undo,
            "redo_steps": redo,
            "status": self.ui.status.0,
            "options": {
                "matcap": doc.opts.matcap_on,
                "flat_shading": doc.opts.flat_shading,
                "cavity": doc.opts.cavity,
                "panels": !doc.opts.clean_view,
                "remesh_resolution": doc.opts.remesh_resolution,
                "new_shape_triangles": doc.opts.new_shape_tris,
                "interface_scale": round(doc.opts.ui_scale),
            },
            "frame_ms": round(doc.stats.frame_ms),
            "gpu": doc.stats.adapter,
        })
    }

    fn set_brush(&mut self, args: &Value) -> Result<String, String> {
        self.sync_view();
        let height = self.view.size.y;
        // Which brush of the library is in hand comes first: everything
        // after it sets that brush. As with the number keys, picking a brush
        // leaves mask painting.
        let mut said = self.bridge_library(args)?;
        let s = &mut self.doc.sculptor.settings;
        if let Some(on) = flag(args, "mask_mode") {
            s.mask_mode = on;
        }
        if let Some(radius) = num(args, "radius") {
            s.radius_px = (radius * height).clamp(2.0, (height * 600.0 / 1080.0).max(4.0));
        }
        if let Some(strength) = num(args, "strength") {
            s.current_mut().strength = strength.clamp(0.01, 1.0);
        }
        if let Some(focal) = num(args, "focal_shift") {
            s.current_mut().focal_shift = focal.clamp(-1.0, 1.0);
        }
        if let Some(direction) = text(args, "direction") {
            s.current_mut().positive = match key(direction).as_str() {
                "add" => true,
                "subtract" => false,
                _ => return Err("direction is \"add\" or \"subtract\".".into()),
            };
        }
        if let Some(on) = flag(args, "accumulate") {
            s.current_mut().accumulate = on;
        }
        // The brush's own stroke settings, each kept to what its slider allows.
        let within = |name: &str, range: std::ops::RangeInclusive<f32>| {
            num(args, name).map(|value| value.clamp(*range.start(), *range.end()))
        };
        let b = s.current_mut();
        b.spacing = within("spacing", BrushSettings::SPACING).unwrap_or(b.spacing);
        b.scatter = within("scatter", BrushSettings::SCATTER).unwrap_or(b.scatter);
        b.size_jitter = within("size_jitter", BrushSettings::SIZE_JITTER).unwrap_or(b.size_jitter);
        b.pressure_size = within("pressure_size", BrushSettings::PRESSURE).unwrap_or(b.pressure_size);
        b.pressure_strength = within("pressure_strength", BrushSettings::PRESSURE).unwrap_or(b.pressure_strength);
        // The kernel's own options: they are the brush's, whichever kernel reads them.
        b.keep_volume = within("keep_volume", 0.0..=1.0).unwrap_or(b.keep_volume);
        b.lock_plane = flag(args, "lock_plane").unwrap_or(b.lock_plane);
        if let Some(name) = text(args, "smooth_mode") {
            b.smooth_mode = SmoothMode::named(name).ok_or("smooth_mode is \"all\", \"peaks\", \"valleys\" or \"relax\".")?;
        }
        if let Some(name) = text(args, "flatten_mode") {
            b.flatten_mode = FlattenMode::named(name).ok_or("flatten_mode is \"both\", \"scrape\" or \"fill\".")?;
        }
        let slot = s.brush.index();
        if let Some(name) = text(args, "alpha") {
            if !s.brush.takes_alpha() {
                return Err(format!("{} does not print an alpha. Standard, Clay, Inflate and Layer do.", s.brush.name()));
            }
            s.alpha[slot].image = match key(name).as_str() {
                "none" => None,
                wanted => Some(alpha::listing().iter().find(|l| key(&l.name) == wanted).and_then(|l| alpha::find(&l.name)).ok_or_else(
                    || {
                        let names: Vec<String> = alpha::listing().iter().map(|l| l.name.clone()).collect();
                        format!("There is no alpha \"{name}\". The alphas are: none, {}.", names.join(", "))
                    },
                )?),
            };
        }
        if let Some(turn) = within("alpha_random_rotation", AlphaSettings::RANDOM_ROTATION) {
            s.alpha[slot].random_rotation = turn;
        }
        if let Some(name) = text(args, "pose_mode") {
            s.pose.mode = PoseMode::ALL
                .into_iter()
                .find(|mode| key(mode.name()) == key(name))
                .ok_or("pose_mode is \"bend\", \"resize\" or \"stretch\".")?;
        }
        if let Some(joints) = num(args, "pose_joints") {
            s.pose.joints = joints.round().clamp(1.0, PoseSettings::MAX_JOINTS as f32) as u32;
        }
        if let Some(offset) = num(args, "pose_offset") {
            s.pose.offset = offset.clamp(0.0, PoseSettings::MAX_OFFSET);
        }
        if let Some(rigidity) = num(args, "pose_rigidity") {
            s.pose.rigidity = rigidity.clamp(0.0, 1.0);
        }
        self.bridge_brush_values(args)?;
        // Symmetry is the active object's own; nothing is set if any of it is wrong.
        let mut symmetry = self.doc.scene.active_symmetry();
        for (axis, name) in ["mirror_x", "mirror_y", "mirror_z"].into_iter().enumerate() {
            if let Some(on) = flag(args, name) {
                symmetry.mirror[axis] = on;
            }
        }
        let radial = &mut symmetry.radial;
        if let Some(repeats) = num(args, "radial") {
            // Fewer than two is no repeat at all: off, with the count kept
            // for when it is switched back on.
            radial.on = repeats >= Radial::MIN_COUNT as f32;
            if radial.on {
                radial.count = (repeats as u32).clamp(Radial::MIN_COUNT, Radial::MAX_COUNT);
            }
        }
        if let Some(name) = text(args, "radial_axis") {
            let axis = [RadialAxis::X, RadialAxis::Y, RadialAxis::Z].into_iter().find(|a| key(a.name()) == key(name));
            radial.axis = axis.ok_or("radial_axis is \"x\", \"y\" or \"z\". radial_custom_axis takes any other direction.")?;
        }
        if let Some(direction) = vec3(args, "radial_custom_axis")? {
            if !(direction.is_finite() && direction.length_squared() > 1e-12) {
                return Err("radial_custom_axis must point somewhere: it cannot be all zeros.".into());
            }
            radial.set_custom_axis(direction);
        }
        if let Some(name) = text(args, "symmetry_space") {
            let space = Space::ALL.into_iter().find(|space| key(space.name()) == key(name));
            symmetry.space = space.ok_or("symmetry_space is \"local\" or \"world\".")?;
        }
        self.doc.edit_symmetry(|own| *own = symmetry);
        said.extend(self.bridge_library_after(args)?);
        let mut state = self.brush_state();
        if flag(args, "list") == Some(true) {
            state["brushes"] = self.bridge_library_list();
        }
        // What the app would have put on its status line: a brush that came
        // in without its alpha, where a deleted brush's file went.
        if !said.is_empty() {
            state["said"] = json!(said);
        }
        Ok(state.to_string())
    }

    /// Draws one stroke along a path of window positions, fed to the sculptor
    /// the way pen samples are.
    fn stroke(&mut self, args: &Value) -> Result<String, String> {
        const SHAPE: &str = "points is a list of [x, y] or [x, y, pressure], each from 0 to 1.";
        let raw = args.get("points").and_then(Value::as_array).filter(|p| !p.is_empty()).ok_or(SHAPE)?;
        self.sync_view();
        let size = self.view.size;
        let mut pen = false;
        let mut points: Vec<(Vec2, f32)> = Vec::with_capacity(raw.len());
        for p in raw {
            let part = |i: usize, name: &str| p.get(i).or_else(|| p.get(name)).and_then(Value::as_f64);
            let (Some(x), Some(y)) = (part(0, "x"), part(1, "y")) else { return Err(SHAPE.into()) };
            let pressure = part(2, "pressure");
            pen |= pressure.is_some();
            points.push((Vec2::new(x as f32, y as f32) * size, pressure.unwrap_or(1.0) as f32));
        }
        let mut samples = vec![points[0]];
        for pair in points.windows(2) {
            let (from, to) = (pair[0], pair[1]);
            let steps = (from.0.distance(to.0) / STROKE_STEP_PX).ceil().max(1.0) as usize;
            if samples.len() + steps > MAX_STROKE_SAMPLES {
                return Err("That stroke is too long for one call. Split it into several.".into());
            }
            for i in 1..=steps {
                let t = i as f32 / steps as f32;
                samples.push((from.0.lerp(to.0, t), from.1 + (to.1 - from.1) * t));
            }
        }

        let active = self.doc.scene.active_id();
        // On the object itself or on one of its live mirror copies, which the stroke is then worked through.
        let Some((_, copy)) = self.pick_placed(samples[0].0).filter(|&(id, _)| id == active) else {
            return Err("A stroke has to start on the active object, and its first point is not on it. \
                        get_state gives each object's on_screen box; pick tests a single point."
                .into());
        };
        self.doc.scene.aim_brush(active, copy);
        let local = self.view.local_to(self.doc.scene.brush_matrix());
        self.doc.scene.release_brush();
        let invert = flag(args, "invert").unwrap_or(false);
        let smooth = flag(args, "smooth").unwrap_or(false);
        let start = self.time();
        let sample = |i: usize| PointerSample {
            pos: samples[i].0,
            pressure: pen.then_some(samples[i].1),
            time: start + i as f64 / 240.0,
        };
        // As for a stroke of the pointer's: a sculpt layer that records but does not show takes none.
        if let Some(why) = self.doc.layer_stroke_refusal().or_else(|| self.doc.erase_refusal(smooth)) {
            return Err(why);
        }
        // And if it adds triangles, it is checked for holes.
        self.stroke_baseline(smooth);
        let doc = &mut self.doc;
        // By the name of the brush as the library has it, not of its kernel.
        let what = doc.sculptor.library.stroke_name(&doc.sculptor.settings, smooth).to_owned();
        let dabs_before = doc.sculptor.dabs_applied;
        if !doc.begin_stroke(&local, sample(0), invert, smooth) {
            return Err("The stroke did not land on the mesh.".into());
        }
        for i in 1..samples.len() {
            doc.sculptor.sample(&mut doc.mesh, &local, sample(i));
            if i % 4 == 0 {
                doc.sculptor.frame(&mut doc.mesh, &local, 1.0 / 60.0);
            }
        }
        doc.sculptor.frame(&mut doc.mesh, &local, 1.0 / 60.0);
        let changed = match doc.sculptor.end(&mut doc.mesh, &local) {
            Some(delta) => {
                doc.history.push_stroke(delta);
                true
            }
            None => false,
        };
        let dabs = doc.sculptor.dabs_applied - dabs_before;
        self.stroke_check();
        Ok(if changed && dabs == 0 {
            // Move and Pose take hold once and are dragged: they lay no dabs.
            format!("{what} drag made. One undo step.")
        } else if changed {
            format!("{what} stroke drawn ({dabs} dabs). One undo step.")
        } else {
            format!("{what} stroke drawn, but it changed nothing. Check the strength, the mask and the radius.")
        })
    }

    fn set_camera(&mut self, args: &Value) -> Result<String, String> {
        self.sync_view();
        let aspect = self.view.size.x / self.view.size.y.max(1.0);
        // Stops any turn or glide still playing, which would otherwise carry
        // on over what is set here. Changes apply at once, not animated, so
        // a screenshot taken straight afterwards shows the result.
        self.doc.camera.orbit(Vec2::ZERO, None);
        if let Some(name) = text(args, "view") {
            let quarter = (self.doc.camera.yaw / 90.0).round() * 90.0;
            let (yaw, pitch) = match key(name).as_str() {
                "front" => (0.0, 0.0),
                "back" => (180.0, 0.0),
                "right" => (-90.0, 0.0),
                "left" => (90.0, 0.0),
                "top" => (quarter, 90.0),
                "bottom" => (quarter, -90.0),
                _ => return Err("view is one of: front, back, left, right, top, bottom.".into()),
            };
            self.doc.camera.yaw = yaw;
            self.doc.camera.pitch = pitch;
        }
        if let Some(yaw) = num(args, "yaw") {
            self.doc.camera.yaw = yaw.rem_euclid(360.0);
        }
        if let Some(pitch) = num(args, "pitch") {
            self.doc.camera.pitch = pitch.clamp(-90.0, 90.0);
        }
        if let Some(ortho) = flag(args, "orthographic") {
            self.doc.camera.ortho = ortho;
        }
        if let Some(what) = text(args, "frame") {
            let scene = &self.doc.scene;
            let bounds = match key(what).as_str() {
                "active" => scene.bounds_of(scene.active(), &self.doc.mesh),
                "all" => scene.bounds(&self.doc.mesh),
                _ => return Err("frame is \"active\" or \"all\".".into()),
            };
            self.doc.camera.frame(bounds, aspect, false);
        }
        if let Some(pivot) = vec3(args, "pivot")? {
            self.doc.camera.pivot = pivot;
        }
        if let Some(distance) = num(args, "distance") {
            self.doc.camera.distance = distance.max(1e-4);
        }
        if let Some(zoom) = num(args, "zoom") {
            self.doc.camera.distance = (self.doc.camera.distance / zoom.max(1e-3)).max(1e-4);
        }
        self.sync_view();
        Ok(self.camera_state().to_string())
    }

    fn pick_at(&mut self, args: &Value) -> Result<String, String> {
        let (Some(x), Some(y)) = (num(args, "x"), num(args, "y")) else {
            return Err("pick needs x and y, each from 0 to 1.".into());
        };
        self.sync_view();
        let (origin, dir) = self.view.ray(Vec2::new(x, y) * self.view.size);
        let scene = &self.doc.scene;
        Ok(match scene.pick(origin, dir, &self.doc.mesh) {
            Some(hit) => json!({
                "object": hit.id,
                "name": scene.get(hit.id).map(|o| o.name.as_str()),
                "active": hit.id == scene.active_id(),
                "position": point(hit.pos),
                "normal": point(hit.normal),
                "group": super::groups::group_at(scene, &self.doc.mesh, hit.id, hit.copy, hit.pos),
            }),
            None => json!({ "object": null }),
        }
        .to_string())
    }

    fn command(&mut self, args: &Value) -> Result<String, String> {
        let name = text(args, "name").ok_or("command needs a name.")?;
        let mask = |op| Action::Mask(op);
        let action = match key(name).as_str() {
            "undo" if !self.doc.history.can_undo() => return Err("There is nothing to undo.".into()),
            "redo" if !self.doc.history.can_redo() => return Err("There is nothing to redo.".into()),
            // These two say what they did with a toast, not the status line.
            "undo" => {
                self.act(Action::Undo);
                return Ok("Undid one step.".into());
            }
            "redo" => {
                self.act(Action::Redo);
                return Ok("Redid one step.".into());
            }
            "remesh" => {
                if let Some(resolution) = num(args, "resolution") {
                    self.doc.opts.remesh_resolution = resolution.round().clamp(4.0, 1024.0);
                }
                Action::Remesh
            }
            "maskinvert" => mask(MaskOp::Invert),
            "maskclear" => mask(MaskOp::Clear),
            "maskblur" => mask(MaskOp::Blur),
            "masksharpen" => mask(MaskOp::Sharpen),
            "maskgrow" => mask(MaskOp::Grow),
            "maskshrink" => mask(MaskOp::Shrink),
            // A mask from the surface's shape, as much of it as `amount` says (sculpt/cavity.rs).
            "maskrecesses" | "maskpeaks" => {
                let shape = if key(name) == "maskpeaks" { cavity::Shape::Peaks } else { cavity::Shape::Recesses };
                Action::MaskShape(shape, num(args, "amount").unwrap_or(cavity::DEFAULT_AMOUNT).clamp(0.0, 1.0))
            }
            "setorigintomesh" => Action::SetOriginToMesh,
            "clone" => Action::CloneObject,
            "instance" => Action::InstanceObject,
            "makeunique" => {
                let scene = &self.doc.scene;
                if scene.users(scene.active_mesh()) < 2 {
                    return Err("The active object is not an instance; it already has its own mesh.".into());
                }
                Action::MakeUnique
            }
            // Subdivision levels and sculpt layers answer for their own (app/levels.rs, app/layers.rs).
            other => {
                let own = self.levels_command(other, args).or_else(|| self.layers_command(other, args));
                let own = own.or_else(|| self.groups_command(other, args));
                // A scripted run's stroke drawn by hand answers for its two (app/ui_demo/hand.rs).
                let own = own.or_else(|| self.hand_demo_command(other));
                return own.unwrap_or_else(|| Err(format!("There is no command \"{name}\".")));
            }
        };
        self.reported(|app| app.act(action))
    }

    fn add_shape(&mut self, args: &Value) -> Result<String, String> {
        let wanted = text(args, "shape").ok_or("add_shape needs a shape.")?;
        let kind = *Primitive::ALL.iter().find(|p| key(p.name()) == key(wanted)).ok_or_else(|| {
            let names: Vec<&str> = Primitive::ALL.iter().map(|p| p.name()).collect();
            format!("There is no shape \"{wanted}\". The shapes are: {}.", names.join(", "))
        })?;
        let position = vec3(args, "position")?;
        let triangles = num(args, "triangles").unwrap_or(self.doc.opts.new_shape_tris).clamp(500.0, 8_000_000.0);
        // Unsized, it comes in as the app's own Add buttons make it.
        let size = num(args, "size").unwrap_or_else(|| self.doc.scene.new_shape_size(&self.doc.mesh)).max(1e-3);
        let mesh = primitives::build(kind, triangles as usize, size);
        let name = text(args, "name").unwrap_or(kind.name());
        let mut id = 0;
        self.edit_scene(|scene, slot| {
            id = scene.add(name, mesh, slot);
            if let (Some(position), Some(object)) = (position, scene.get_mut(id)) {
                object.position = position;
            }
            true
        });
        let object = self.doc.scene.active();
        Ok(json!({ "id": id, "name": object.name, "triangles": self.doc.mesh.triangle_count() }).to_string())
    }

    fn edit_object(&mut self, args: &Value) -> Result<String, String> {
        let id = self.object(args)?;
        let position = vec3(args, "position")?;
        let (name, visible) = (text(args, "name"), flag(args, "visible"));
        let changed = self.edit_scene(|scene, _| {
            let name = name.filter(|n| scene.get(id).is_some_and(|o| o.name != *n)).map(|n| scene.unique_name(n));
            let Some(object) = scene.get_mut(id) else { return false };
            let before = object.clone();
            if let Some(name) = name {
                object.name = name;
            }
            object.visible = visible.unwrap_or(object.visible);
            object.position = position.unwrap_or(object.position);
            *object != before
        });
        let Some(object) = self.doc.scene.get(id) else { return Err("That object is gone.".into()) };
        let state = json!({ "id": id, "name": object.name, "visible": object.visible, "position": point(object.position) });
        Ok(if changed { state.to_string() } else { format!("Nothing changed: {state}") })
    }

    fn delete_object(&mut self, args: &Value) -> Result<String, String> {
        let id = self.object(args)?;
        if self.doc.scene.objects.len() < 2 {
            return Err("The last object cannot be deleted.".into());
        }
        self.reported(|app| app.act(Action::DeleteObject(id)))
    }

    /// The `path` argument, which must be on one of this computer's own
    /// drives (`bridge_guard::off_limits`; checked first, before anything
    /// looks at the disk) and absolute: the app's own working folder is not
    /// one a caller can know. `open_file` and `save_file` are the tools that
    /// take a path, and both come through here.
    fn path(args: &Value) -> Result<(PathBuf, String), String> {
        Self::path_named(args, "path")
    }

    /// [`Self::path`] for a path under another name: the brush files
    /// `set_brush` imports and exports are held to the same rule.
    fn path_named(args: &Value, name: &str) -> Result<(PathBuf, String), String> {
        let given = text(args, name).ok_or("A path is needed.")?;
        if let Some(why) = guard::off_limits(given) {
            return Err(why.into());
        }
        let path = PathBuf::from(given);
        if !path.is_absolute() {
            return Err(format!("{} is not a full path. Give one that starts at the drive.", path.display()));
        }
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        Ok((path, extension))
    }

    fn open_file(&mut self, args: &Value) -> Result<String, String> {
        let (path, extension) = Self::path(args)?;
        if !matches!(extension.as_str(), "sclp" | "sculpt" | "obj" | "stl") {
            return Err("Sculpt opens .sclp files (and .sculpt files from the Unity app) and imports .obj and .stl.".into());
        }
        if !path.is_file() {
            return Err(format!("There is no file at {}.", path.display()));
        }
        let native = is_native(&path);
        let lost = self.scaffolds_at_risk(Loss::SceneReplaced);
        may_open(native, self.unsaved(), &lost, flag(args, "discard").unwrap_or(false))?;
        self.reported(|app| app.load(path, native))
    }

    fn save_file(&mut self, args: &Value) -> Result<String, String> {
        let (path, extension) = Self::path(args)?;
        let name = self.doc.scene.active().name.clone();
        match extension.as_str() {
            "sclp" => {
                self.save_scene(&path)?;
                // As the interface's Save says it: with a word about points written mended, if any were.
                let message = saved_words(&path, &self.saved_note(), false);
                self.status(message.clone(), true);
                self.doc.path = Some(path);
                Ok(message)
            }
            "obj" => {
                let places = self.export_places();
                let places: Vec<(&str, glam::Mat4)> = places.iter().map(|(n, m)| (n.as_str(), *m)).collect();
                let (result, mended) = Self::split_mended(io::export_obj_placed(&self.doc.mesh, &places, &path));
                result?;
                Ok(format!("Exported \"{name}\"{} to {}{mended}", self.export_note(), path.display()))
            }
            "stl" => {
                // The same export as the button's, without its question: nobody is there to
                // answer it, so the file is written and the answer says if it may not print.
                self.export_stl_to(&path)
            }
            "fbx" => self.export_fbx_to(&path),
            _ => Err("The path must end in .sclp (the whole scene), .obj or .stl (the active object), or .fbx (every selected object).".into()),
        }
    }

    fn set_options(&mut self, args: &Value) -> Result<String, String> {
        let opts = &mut self.doc.opts;
        let set = |name: &str, target: &mut bool| {
            if let Some(on) = flag(args, name) {
                *target = on;
            }
        };
        set("matcap", &mut opts.matcap_on);
        set("flat_shading", &mut opts.flat_shading);
        set("cavity", &mut opts.cavity);
        set("symmetry_guides", &mut opts.symmetry_guides);
        if let Some(panels) = flag(args, "panels") {
            opts.clean_view = !panels;
        }
        if let Some(scale) = num(args, "interface_scale") {
            opts.ui_scale = scale.clamp(0.75, 3.0);
        }
        Ok(self.state()["options"].to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "C:\\Models\\head.sclp";

    const EITHER: [bool; 2] = [false, true];

    /// `leave` with no stroke under way and no work that is in no file.
    fn leave_idle(unsaved: bool, file: Option<&Path>, save: bool, discard: bool) -> Leave {
        leave(Leaving { mid_stroke: false, unsaved, lost: &[], file, save, discard })
    }

    const TUBES: &str = "3 unmerged tubes (Merge or Keep Separate first)";
    const RIG: &str = "an SSphere rig of 12 spheres (Make Skin first)";

    fn lost() -> Vec<String> {
        vec![RIG.to_owned(), TUBES.to_owned()]
    }

    /// The seam between the bridge and the closing question: `quit` went by
    /// unsaved changes alone, so with the scene saved it closed over a rig
    /// and three tubes that the close button would have asked about.
    #[test]
    fn quit_does_not_close_over_work_that_is_in_no_file_unless_told_to_discard_it() {
        let lost = lost();
        let file = Some(Path::new(FILE));
        for unsaved in EITHER {
            let decide = |save, discard| leave(Leaving { mid_stroke: false, unsaved, lost: &lost, file, save, discard });
            // Not on a bare quit, and not on save: a save does not keep it.
            for save in EITHER {
                let Leave::Stay(why) = decide(save, false) else { panic!("unsaved {unsaved}, save {save}: it closed") };
                let says = |words: &str| why.contains(words);
                assert!(says(RIG) && says(TUBES), "it names what would be lost: {why}");
                assert!(says("saving does not keep") && says("was not closed") && says("Ask the user"), "{why}");
                assert!(says("discard: true"), "and says how to close all the same: {why}");
                assert_eq!(says("save_file"), unsaved, "unsaved changes can still be saved first: {why}");
            }
            // Both at once is not an answer here either.
            assert!(matches!(decide(true, true), Leave::Stay(_)));
            // The user's word that it can go.
            assert_eq!(decide(false, true), Leave::Close);
        }
        // A stroke under way still comes first.
        let mid = leave(Leaving { mid_stroke: true, unsaved: false, lost: &lost, file, save: false, discard: true });
        assert_eq!(mid, Leave::Stay(STROKE_UNDER_WAY.into()));
    }

    #[test]
    fn a_sculpt_is_not_opened_over_work_that_is_in_no_file_unless_told_to_discard_it() {
        let lost = vec![TUBES.to_owned()];
        for unsaved in EITHER {
            let why = may_open(true, unsaved, &lost, false).expect_err("it opened over three unmerged tubes");
            let says = |words: &str| why.contains(words);
            assert!(says(TUBES) && says("Nothing was opened") && says("Ask the user") && says("discard: true"), "{why}");
            assert_eq!(says("unsaved changes"), unsaved, "{why}");
            assert_eq!(may_open(true, unsaved, &lost, true), Ok(()), "the user's word that it can go");
            // A model is added to the scene, which costs nothing that stands on it.
            assert_eq!(may_open(false, unsaved, &lost, false), Ok(()));
        }
    }

    /// The bridge's own replies to a save said nothing of points written
    /// mended, which the interface's Save does say.
    #[test]
    fn a_save_through_the_bridge_says_what_the_interfaces_save_says() {
        let path = Path::new(FILE);
        assert_eq!(saved_words(path, "", false), format!("Saved {FILE}"));
        assert_eq!(saved_words(path, "", true), format!("Saved {FILE}. Sculpt is closing."));
        let note = io::note_saved(2).map(|note| format!(". {note}")).expect("two points were mended");
        let said = saved_words(path, &note, false);
        assert!(said.starts_with(&format!("Saved {FILE}. 2 points of the model have no position")), "{said}");
        let said = saved_words(path, &note, true);
        assert!(said.contains("2 points of the model have no position"), "{said}");
        assert!(said.ends_with("unchanged. Sculpt is closing."), "{said}");
        assert!(!said.contains(".."), "{said}");
    }

    #[test]
    fn quit_closes_at_once_only_when_nothing_would_be_lost() {
        for (save, discard) in [(false, false), (true, false), (false, true), (true, true)] {
            assert_eq!(leave_idle(false, None, save, discard), Leave::Close);
            assert_eq!(leave_idle(false, Some(Path::new(FILE)), save, discard), Leave::Close);
        }
    }

    #[test]
    fn quit_never_closes_the_app_in_the_middle_of_a_stroke() {
        // Whatever else is so and whatever the call says: discard speaks for
        // saved-or-not work, not for a stroke somebody's hand is still on.
        for unsaved in EITHER {
            for file in [None, Some(Path::new(FILE))] {
                for (save, discard) in [(false, false), (true, false), (false, true), (true, true)] {
                    let decided = leave(Leaving { mid_stroke: true, unsaved, lost: &[], file, save, discard });
                    assert_eq!(decided, Leave::Stay(STROKE_UNDER_WAY.into()), "{unsaved} {file:?} {save} {discard}");
                }
            }
        }
        assert!(STROKE_UNDER_WAY.contains("was not closed") && STROKE_UNDER_WAY.contains("Try again"));
    }

    #[test]
    fn quit_with_unsaved_work_stays_open_until_told_which() {
        let file = Path::new(FILE);
        // Neither flag: refused, with or without a file, and the answer says what to do.
        for file in [None, Some(file)] {
            let Leave::Stay(why) = leave_idle(true, file, false, false) else { panic!("closed over unsaved work") };
            let says = |words: &str| why.contains(words);
            assert!(says("unsaved work") && says("Ask the user") && says("discard: true"), "{why}");
        }
        let Leave::Stay(why) = leave_idle(true, Some(file), false, false) else { unreachable!() };
        assert!(why.contains("save: true") && why.contains("head.sclp"), "{why}");
        // Told to discard: closes. Told to save: saves to the scene's own file first.
        assert_eq!(leave_idle(true, None, false, true), Leave::Close);
        assert_eq!(leave_idle(true, Some(file), false, true), Leave::Close);
        assert_eq!(leave_idle(true, Some(file), true, false), Leave::SaveFirst(file.to_owned()));
        // Nowhere to save to: stays open, and says to save_file first.
        let Leave::Stay(why) = leave_idle(true, None, true, false) else { panic!("closed with nowhere to save") };
        assert!(why.contains("save_file"), "{why}");
        // Both at once is not an answer.
        assert!(matches!(leave_idle(true, Some(file), true, true), Leave::Stay(_)));
        assert!(matches!(leave_idle(true, None, true, true), Leave::Stay(_)));
    }

    #[test]
    fn a_sculpt_is_not_opened_over_unsaved_work_unless_told_to_discard_it() {
        for sculpt in EITHER {
            for unsaved in EITHER {
                for discard in EITHER {
                    let decided = may_open(sculpt, unsaved, &[], discard);
                    // Refused in one case of the eight: a sculpt, over unsaved work, with no word to discard it.
                    let refused = sculpt && unsaved && !discard;
                    assert_eq!(decided.is_err(), refused, "sculpt {sculpt}, unsaved {unsaved}, discard {discard}");
                }
            }
        }
        // Spelt out, the cases that matter: work is kept, a model is always added, a saved scene always gives way.
        assert_eq!(may_open(true, true, &[], false), Err(REPLACING_UNSAVED.to_owned()));
        assert_eq!(may_open(true, true, &[], true), Ok(()));
        assert_eq!(may_open(false, true, &[], false), Ok(()));
        assert_eq!(may_open(true, false, &[], false), Ok(()));
        // And the refusal tells Claude what to do about it.
        let says = |words: &str| REPLACING_UNSAVED.contains(words);
        assert!(says("unsaved work") && says("Nothing was opened") && says("Ask the user") && says("discard: true"));
    }
}
