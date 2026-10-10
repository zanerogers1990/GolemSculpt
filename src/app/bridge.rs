//! The Claude bridge, app side: a loopback socket that lets a Claude session
//! see and drive the running app (state, screenshots, strokes, camera, files).
//!
//! Claude itself talks to `sculpt.exe --mcp` (see `crate::mcp`), which passes
//! each tool call here as one line of JSON and gets one line back. Requests
//! are handed to the main thread and run between frames, through the same
//! code paths the interface uses, so what Claude does is undoable like
//! anything else.
//!
//! Screen positions are fractions of the window: (0, 0) is the top-left
//! corner and (1, 1) the bottom-right, whatever size a screenshot was sent at.

use std::io::{BufRead, BufReader, Cursor, Write};
use std::net::{Ipv4Addr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::Duration;

use glam::{Vec2, Vec3};
use serde_json::{Value, json};
use winit::event_loop::{ActiveEventLoop, EventLoopProxy};

use super::{Drag, Running, is_native};
use crate::doc::{Action, MaskOp, Options};
use crate::io;
use crate::mesh::bvh::Aabb;
use crate::mesh::primitives::{self, Primitive};
use crate::sculpt::{Brush, PointerSample, Radial, RadialAxis};
use crate::ui::theme;

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
}

/// A screenshot waiting for the next frame to be drawn.
pub struct Capture {
    reply: Sender<Reply>,
    max_size: u32,
    /// The panels were hidden for this shot; what to set the option back to.
    clean_view: Option<bool>,
}

/// Starts listening. `wake` rouses the event loop, which sleeps while nothing
/// is happening. Returns `None` if the port is taken, which is what happens
/// when a second copy of the app is opened: the first keeps the bridge.
pub fn start(wake: EventLoopProxy<()>) -> Option<Bridge> {
    let port = crate::mcp::port();
    let listener = match TcpListener::bind((Ipv4Addr::LOCALHOST, port)) {
        Ok(listener) => listener,
        Err(e) => {
            log::warn!("the Claude bridge is off (port {port}: {e})");
            return None;
        }
    };
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for stream in listener.incoming().flatten() {
            let (tx, wake) = (tx.clone(), wake.clone());
            std::thread::spawn(move || serve(stream, tx, wake));
        }
    });
    Some(Bridge { requests: rx })
}

/// One connection: a request per line in, a reply per line out.
fn serve(stream: TcpStream, requests: Sender<Request>, wake: EventLoopProxy<()>) {
    let Ok(mut out) = stream.try_clone() else { return };
    for line in BufReader::new(stream).lines() {
        let Ok(line) = line else { return };
        if line.trim().is_empty() {
            continue;
        }
        let reply = match serde_json::from_str::<Value>(&line) {
            Ok(message) => {
                let (tx, rx) = mpsc::channel();
                let request = Request {
                    tool: message["tool"].as_str().unwrap_or_default().to_owned(),
                    args: message["args"].clone(),
                    reply: tx,
                };
                if requests.send(request).is_err() || wake.send_event(()).is_err() {
                    return;
                }
                rx.recv_timeout(REPLY_TIMEOUT)
                    .unwrap_or_else(|_| Reply::from(Err("The app did not answer in time.".into())))
            }
            Err(e) => Reply::from(Err(format!("Bad request: {e}"))),
        };
        let mut message = json!({ "ok": reply.ok, "text": reply.text });
        if let Some(png) = reply.image {
            message["image"] = base64(&png).into();
        }
        if writeln!(out, "{message}").is_err() {
            return;
        }
    }
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
                    let _ = request.reply.send(Reply::from(Ok("Sculpt is closing.".into())));
                    event_loop.exit();
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

impl Running {
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

    fn call(&mut self, tool: &str, args: &Value) -> Result<String, String> {
        if tool == "get_state" {
            return Ok(self.state().to_string());
        }
        if self.doc.sculptor.is_active() || self.input.drag != Drag::None {
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
        json!({
            "name": s.brush.name(),
            "radius": round(s.radius_px / self.view.size.y.max(1.0)),
            "radius_px": round(s.radius_px),
            "strength": round(b.strength),
            "focal_shift": round(b.focal_shift),
            "direction": if b.positive { "add" } else { "subtract" },
            "accumulate": b.accumulate,
            "mask_mode": s.mask_mode,
            "mirror": { "x": s.symmetry[0], "y": s.symmetry[1], "z": s.symmetry[2] },
            "radial": { "on": s.radial.on, "repeats": s.radial.count, "axis": point(s.radial.axis_vector()) },
        })
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
                    "visible": o.visible,
                    "position": point(o.position),
                    "size": point(size),
                    "triangles": mesh.map_or(0, |m| m.triangle_count()),
                    "vertices": mesh.map_or(0, |m| m.vertex_count()),
                    "instances": doc.scene.users(o.mesh),
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
        let s = &mut self.doc.sculptor.settings;
        if let Some(name) = text(args, "brush") {
            let brush = Brush::ALL.iter().find(|b| key(b.name()) == key(name));
            s.brush = *brush.ok_or_else(|| {
                let names: Vec<&str> = Brush::ALL.iter().map(|b| b.name()).collect();
                format!("There is no brush \"{name}\". The brushes are: {}.", names.join(", "))
            })?;
            // As with the number keys, picking a brush leaves mask painting.
            s.mask_mode = false;
        }
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
        for (axis, name) in ["mirror_x", "mirror_y", "mirror_z"].into_iter().enumerate() {
            if let Some(on) = flag(args, name) {
                s.symmetry[axis] = on;
            }
        }
        if let Some(repeats) = num(args, "radial") {
            // Fewer than two is no repeat at all: off, with the count kept
            // for when it is switched back on.
            s.radial.on = repeats >= Radial::MIN_COUNT as f32;
            if s.radial.on {
                s.radial.count = (repeats as u32).clamp(Radial::MIN_COUNT, Radial::MAX_COUNT);
            }
        }
        if let Some(name) = text(args, "radial_axis") {
            let axis = [RadialAxis::X, RadialAxis::Y, RadialAxis::Z].into_iter().find(|a| key(a.name()) == key(name));
            s.radial.axis = axis.ok_or("radial_axis is \"x\", \"y\" or \"z\". radial_custom_axis takes any other direction.")?;
        }
        if let Some(direction) = vec3(args, "radial_custom_axis")? {
            if !(direction.is_finite() && direction.length_squared() > 1e-12) {
                return Err("radial_custom_axis must point somewhere: it cannot be all zeros.".into());
            }
            s.radial.set_custom_axis(direction);
        }
        Ok(self.brush_state().to_string())
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

        if self.pick(samples[0].0) != Some(self.doc.scene.active_id()) {
            return Err("A stroke has to start on the active object, and its first point is not on it. \
                        get_state gives each object's on_screen box; pick tests a single point."
                .into());
        }
        let invert = flag(args, "invert").unwrap_or(false);
        let smooth = flag(args, "smooth").unwrap_or(false);
        let start = self.time();
        let sample = |i: usize| PointerSample {
            pos: samples[i].0,
            pressure: pen.then_some(samples[i].1),
            time: start + i as f64 / 240.0,
        };
        let doc = &mut self.doc;
        let brush = doc.sculptor.effective_brush(smooth);
        let dabs_before = doc.sculptor.dabs_applied;
        if !doc.sculptor.begin(&mut doc.mesh, &self.local, sample(0), invert, smooth) {
            return Err("The stroke did not land on the mesh.".into());
        }
        for i in 1..samples.len() {
            doc.sculptor.sample(&mut doc.mesh, &self.local, sample(i));
            if i % 4 == 0 {
                doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
            }
        }
        doc.sculptor.frame(&mut doc.mesh, &self.local, 1.0 / 60.0);
        let changed = match doc.sculptor.end(&mut doc.mesh, &self.local) {
            Some(delta) => {
                doc.history.push_stroke(delta);
                true
            }
            None => false,
        };
        let dabs = doc.sculptor.dabs_applied - dabs_before;
        let what = if doc.sculptor.settings.mask_mode { "Mask" } else { brush.name() };
        Ok(if changed {
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
            "clone" => Action::CloneObject,
            "instance" => Action::InstanceObject,
            "makeunique" => {
                let scene = &self.doc.scene;
                if scene.users(scene.active_mesh()) < 2 {
                    return Err("The active object is not an instance; it already has its own mesh.".into());
                }
                Action::MakeUnique
            }
            _ => return Err(format!("There is no command \"{name}\".")),
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

    /// The `path` argument, which must be absolute: the app's own working
    /// folder is not one a caller can know.
    fn path(args: &Value) -> Result<(PathBuf, String), String> {
        let path = PathBuf::from(text(args, "path").ok_or("A path is needed.")?);
        if !path.is_absolute() {
            return Err(format!("{} is not a full path. Give one that starts at the drive.", path.display()));
        }
        let extension = path.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
        Ok((path, extension))
    }

    fn open_file(&mut self, args: &Value) -> Result<String, String> {
        let (path, extension) = Self::path(args)?;
        if !matches!(extension.as_str(), "sclp" | "obj" | "stl") {
            return Err("Sculpt opens .sclp files and imports .obj and .stl.".into());
        }
        if !path.is_file() {
            return Err(format!("There is no file at {}.", path.display()));
        }
        let native = is_native(&path);
        self.reported(|app| app.load(path, native))
    }

    fn save_file(&mut self, args: &Value) -> Result<String, String> {
        let (path, extension) = Self::path(args)?;
        let name = self.doc.scene.active().name.clone();
        match extension.as_str() {
            "sclp" => {
                self.save_scene(&path)?;
                let message = format!("Saved {}", path.display());
                self.status(message.clone(), true);
                self.doc.path = Some(path);
                Ok(message)
            }
            "obj" => {
                io::export_obj(&self.doc.mesh, self.doc.scene.active().matrix(), &path)?;
                Ok(format!("Exported \"{name}\" to {}", path.display()))
            }
            "stl" => {
                io::export_stl(&self.doc.mesh, self.doc.scene.active().matrix(), &path)?;
                Ok(format!("Exported \"{name}\" to {} (millimetres, Z up)", path.display()))
            }
            _ => Err("The path must end in .sclp (the whole scene), or .obj or .stl (the active object).".into()),
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
