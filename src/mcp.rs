//! `sculpt.exe --mcp`: the Model Context Protocol server Claude connects to.
//!
//! It speaks MCP (JSON-RPC, one message per line) on stdin and stdout and
//! passes each tool call to the running app's bridge (`app::bridge`) over a
//! loopback socket. It keeps nothing between calls, so the app can be closed,
//! rebuilt and started again underneath a connected Claude session.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

const DEFAULT_PORT: u16 = 47_823;
const PROTOCOL: &str = "2025-06-18";
/// Longer than the app's own limit, so its answer is the one that gets back.
const REPLY_TIMEOUT: Duration = Duration::from_secs(150);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
const NOT_RUNNING: &str = "Sculpt is not running. Call launch to start it.";

const INSTRUCTIONS: &str = "Sees and drives Sculpt, the user's sculpting app, while it is running. \
Call launch first if a tool says the app is not running. Look with screenshot and get_state before \
acting. Screen positions are fractions of the window: [0, 0] is the top-left corner and [1, 1] the \
bottom-right. Everything that changes the model is one undo step in the app, and the user may be \
working in the same window, so do not undo more than you did yourself.";

/// The bridge's port. Both sides read it here.
pub fn port() -> u16 {
    std::env::var("SCULPT_BRIDGE_PORT").ok().and_then(|p| p.parse().ok()).unwrap_or(DEFAULT_PORT)
}

struct Server {
    /// The program `launch` starts.
    app: Option<PathBuf>,
    link: Option<BufReader<TcpStream>>,
}

/// Serves MCP until stdin closes. `app` is the program to launch on request;
/// this one if not given.
pub fn run(app: Option<PathBuf>) {
    let mut server = Server { app: app.or_else(|| std::env::current_exe().ok()), link: None };
    let mut stdout = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else { continue };
        // No id: a notification, which gets no answer. No method: an answer
        // to something, and this server asks nothing.
        let (Some(id), Some(method)) = (message.get("id").filter(|id| !id.is_null()), message["method"].as_str())
        else {
            continue;
        };
        let outcome = match method {
            "initialize" => Ok(json!({
                "protocolVersion": message["params"]["protocolVersion"].as_str().unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "sculpt", "version": env!("CARGO_PKG_VERSION") },
                "instructions": INSTRUCTIONS,
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => Ok(server.call(&message["params"])),
            _ => Err(json!({ "code": -32601, "message": format!("Method not found: {method}") })),
        };
        let reply = match outcome {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        };
        if writeln!(stdout, "{reply}").and_then(|()| stdout.flush()).is_err() {
            break;
        }
    }
}

fn connect() -> Option<BufReader<TcpStream>> {
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port()));
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500)).ok()?;
    stream.set_read_timeout(Some(REPLY_TIMEOUT)).ok()?;
    stream.set_nodelay(true).ok()?;
    Some(BufReader::new(stream))
}

/// Sends one request line and reads the reply line.
fn exchange(link: &mut BufReader<TcpStream>, request: &str) -> std::io::Result<Value> {
    writeln!(link.get_mut(), "{request}")?;
    let mut line = String::new();
    if link.read_line(&mut line)? == 0 {
        return Err(ErrorKind::UnexpectedEof.into());
    }
    serde_json::from_str(&line).map_err(|e| std::io::Error::new(ErrorKind::InvalidData, e))
}

impl Server {
    fn call(&mut self, params: &Value) -> Value {
        let name = params["name"].as_str().unwrap_or_default();
        let args = params.get("arguments").filter(|a| a.is_object()).cloned().unwrap_or_else(|| json!({}));
        let outcome = if name == "launch" { self.launch(&args) } else { self.forward(name, &args) };
        match outcome {
            Ok(reply) => {
                let mut content = vec![json!({ "type": "text", "text": reply["text"] })];
                if let Some(image) = reply["image"].as_str() {
                    content.push(json!({ "type": "image", "data": image, "mimeType": "image/png" }));
                }
                json!({ "content": content, "isError": reply["ok"] != true })
            }
            Err(text) => json!({ "content": [{ "type": "text", "text": text }], "isError": true }),
        }
    }

    fn forward(&mut self, tool: &str, args: &Value) -> Result<Value, String> {
        let request = json!({ "tool": tool, "args": args }).to_string();
        // The app may have been restarted since the last call, which leaves a
        // dead link behind: that gets one fresh try.
        for fresh in [false, true] {
            if self.link.is_none() {
                self.link = Some(connect().ok_or(NOT_RUNNING)?);
            }
            let Some(link) = self.link.as_mut() else { break };
            match exchange(link, &request) {
                Ok(reply) => return Ok(reply),
                Err(e) => {
                    self.link = None;
                    if matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock) {
                        return Err("Sculpt is still busy with that and has not answered yet.".into());
                    }
                    if fresh {
                        return Err(format!("Lost the connection to Sculpt: {e}"));
                    }
                }
            }
        }
        Err(NOT_RUNNING.into())
    }

    fn launch(&mut self, args: &Value) -> Result<Value, String> {
        let done = |text: &str| Ok(json!({ "ok": true, "text": text }));
        let file = args.get("file").and_then(Value::as_str);
        if connect().is_some() {
            return done(match file {
                Some(_) => "Sculpt is already running. Use open_file to open the file in it.",
                None => "Sculpt is already running.",
            });
        }
        let app = self.app.clone().filter(|p| p.is_file()).ok_or_else(|| {
            let shown = self.app.as_ref().map_or("sculpt.exe".into(), |p| p.display().to_string());
            format!("Sculpt has not been built yet: {shown} is missing. Build it with `cargo build --release`.")
        })?;
        start(&app, file).map_err(|e| format!("Could not start {}: {e}", app.display()))?;
        let began = Instant::now();
        while began.elapsed() < LAUNCH_TIMEOUT {
            std::thread::sleep(Duration::from_millis(250));
            if connect().is_some() {
                return done("Sculpt is running.");
            }
        }
        Err("Sculpt was started but its bridge did not answer. Another program may be using the port.".into())
    }
}

/// Starts the app on its own, not as a child of this process: it is the
/// user's window and must outlive the Claude session that opened it.
#[cfg(windows)]
fn start(app: &std::path::Path, file: Option<&str>) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    let mut line = format!("/c start \"\" \"{}\"", app.display());
    if let Some(file) = file {
        line.push_str(&format!(" \"{file}\""));
    }
    let mut command = Command::new("cmd");
    command.raw_arg(line);
    if let Some(folder) = app.parent() {
        command.current_dir(folder);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).status().map(|_| ())
}

#[cfg(not(windows))]
fn start(app: &std::path::Path, file: Option<&str>) -> std::io::Result<()> {
    let mut command = Command::new(app);
    command.args(file);
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|_| ())
}

fn tools() -> Value {
    let tool = |name: &str, description: &str, properties: Value, required: &[&str]| {
        json!({
            "name": name,
            "description": description,
            "inputSchema": { "type": "object", "properties": properties, "required": required },
        })
    };
    let number = |description: &str| json!({ "type": "number", "description": description });
    let boolean = |description: &str| json!({ "type": "boolean", "description": description });
    let string = |description: &str| json!({ "type": "string", "description": description });
    let choice = |options: &[&str], description: &str| json!({ "type": "string", "enum": options, "description": description });
    let xyz = |description: &str| {
        json!({ "type": "array", "items": { "type": "number" }, "minItems": 3, "maxItems": 3, "description": description })
    };
    let object = || {
        json!({ "type": ["integer", "string"], "description": "An object's id or name, from get_state. Leave out for the active object." })
    };
    json!([
        tool(
            "launch",
            "Start the Sculpt app if it is not already running, and wait until it is ready.",
            json!({ "file": string("Full path of a .sclp, .obj or .stl file to open at start.") }),
            &[],
        ),
        tool(
            "quit",
            "Close the Sculpt app. Unsaved work is lost, so save first unless the user said otherwise. \
             Needed before rebuilding the app, since Windows will not replace a running program.",
            json!({}),
            &[],
        ),
        tool(
            "get_state",
            "Everything about the app right now, as JSON: window size, the objects (id, name, size, triangle \
             count, and the on_screen box [left, top, right, bottom] each covers, as fractions of the window), \
             the brush and its settings, the camera, undo steps, the status line and the display options.",
            json!({}),
            &[],
        ),
        tool(
            "screenshot",
            "A picture of the app window as it is now: the model and, unless panels is false, the interface.",
            json!({
                "panels": boolean("False hides the interface panels for this picture. Default true."),
                "max_size": number("Longest side of the picture in pixels. Default 1280; raise it to read small text."),
            }),
            &[],
        ),
        tool(
            "set_brush",
            "Choose the brush and its settings. Only what is given changes. Returns the brush as it now is.",
            json!({
                "brush": choice(
                    &["Move", "Clay", "Smooth", "Crease", "Inflate", "Flatten", "Pose", "Standard", "Layer", "Snake Hook"],
                    "The brush. Choosing one turns mask painting off. Pose bends a limb: a stroke with it presses on \
                     the limb and drags, and the limb turns about a joint the brush finds one brush radius back \
                     along it, toward the body. Size the brush to put the joint where the bend should be.",
                ),
                "radius": number("Brush radius as a fraction of the window height, e.g. 0.05."),
                "strength": number("0.01 to 1. The app's default is 0.1."),
                "focal_shift": number("-1 to 1: where the falloff sits between the centre and the rim."),
                "direction": choice(&["add", "subtract"], "Whether the brush builds up or digs in."),
                "accumulate": boolean("Whether passing over the same place again keeps adding."),
                "pose_mode": choice(
                    &["bend", "resize", "stretch"],
                    "Pose: what a drag does. bend turns the limb about the joint (an inverted stroke twists it \
                     about its own length); resize grows or shrinks it about the joint (inverted: slides it \
                     without turning); stretch makes it longer and thinner or shorter and fatter.",
                ),
                "pose_joints": number(
                    "Pose, bend only: 1 to 8 joints. More than one makes a chain back along the limb that reaches \
                     for the pointer while its far end stays put.",
                ),
                "pose_offset": number(
                    "Pose: 0 to 2. Moves the joint this many brush radii further up the limb, past the brush ring.",
                ),
                "pose_rigidity": number(
                    "Pose: 0 to 1. How sharply a joint bends: 0 a long soft curve, 1 a hinge. The app's default is 0.6.",
                ),
                "mask_mode": boolean("Paint the mask instead of sculpting. Masked areas do not move."),
                "mirror_x": boolean("Mirror strokes across the X plane (left and right)."),
                "mirror_y": boolean("Mirror strokes across the Y plane."),
                "mirror_z": boolean("Mirror strokes across the Z plane."),
                "radial": number("Radial symmetry: how many times each stroke is made round the axis, 2 to 32. 0 or 1 switches it off. It combines with the mirror planes."),
                "radial_axis": choice(&["x", "y", "z"], "The axis radial symmetry goes round, through the active object's centre. The app starts with y."),
                "radial_custom_axis": xyz("Any other axis for it, as a direction in the active object's own space."),
            }),
            &[],
        ),
        tool(
            "stroke",
            "Draw one stroke on the active object with the current brush, along a path of window positions. \
             The first point must be on the active object. The path is smoothed, so a few points make a \
             curve. One point makes a single dab. Take a screenshot afterwards to see the result.",
            json!({
                "points": {
                    "type": "array",
                    "minItems": 1,
                    "items": { "type": "array", "items": { "type": "number" }, "minItems": 2, "maxItems": 3 },
                    "description": "[x, y] or [x, y, pressure] each, all from 0 to 1. Without pressure the stroke is at full pressure, like a mouse.",
                },
                "invert": boolean(
                    "Flip the brush's direction for this stroke (like holding Ctrl). With Pose: twist instead of \
                     bend, slide instead of resize.",
                ),
                "smooth": boolean("Smooth instead, whatever the brush (like holding Shift)."),
            }),
            &["points"],
        ),
        tool(
            "set_camera",
            "Move the camera. Only what is given changes, applied in the order listed, at once (not animated). \
             Returns the camera as it now is.",
            json!({
                "view": choice(&["front", "back", "left", "right", "top", "bottom"], "Turn to an axis view."),
                "yaw": number("Degrees. 0 is the front view; positive turns the model to the right on screen."),
                "pitch": number("Degrees, -90 to 90. Positive looks down from above."),
                "orthographic": boolean("Orthographic instead of perspective."),
                "frame": choice(&["active", "all"], "Fit the active object, or everything, in view."),
                "pivot": xyz("The point the camera looks at and turns about."),
                "distance": number("Distance from the pivot."),
                "zoom": number("Multiply the magnification: 2 is twice as close, 0.5 twice as far."),
            }),
            &[],
        ),
        tool(
            "pick",
            "What is under a window position: the object, and the point and surface normal there. \
             Use it to check that a stroke will start on the model.",
            json!({ "x": number("0 (left) to 1 (right)."), "y": number("0 (top) to 1 (bottom).") }),
            &["x", "y"],
        ),
        tool(
            "command",
            "Run one of the app's commands on the active object.",
            json!({
                "name": choice(
                    &[
                        "undo", "redo", "remesh", "mask_invert", "mask_clear", "mask_blur", "mask_sharpen",
                        "mask_grow", "mask_shrink", "clone", "instance", "make_unique",
                    ],
                    "remesh rebuilds the mesh from voxels, evening out the triangles. clone adds an independent \
                     copy; instance adds a linked copy that shares the mesh; make_unique unlinks an instance.",
                ),
                "resolution": number("For remesh: voxels along the longest side, 4 to 1024. Default: the app's setting."),
            }),
            &["name"],
        ),
        tool(
            "add_shape",
            "Add a new object to the scene and make it the active one.",
            json!({
                "shape": choice(&["Sphere", "Cube", "Cylinder", "Capsule", "Cone", "Wedge"], "The starting shape."),
                "name": string("A name for it. Default: the shape's name."),
                "position": xyz("Where to put it. Default: the centre."),
                "size": number("Half its extent in world units. Default: the size the app's own button uses, nine tenths of the top object in the list (0.45 beside the starting sphere)."),
                "triangles": number("How finely to mesh it. Default: the app's setting (200,000)."),
            }),
            &["shape"],
        ),
        tool("select_object", "Make an object the active one: the one strokes and commands apply to.", json!({ "object": object() }), &["object"]),
        tool(
            "edit_object",
            "Rename, show, hide or move an object. Only what is given changes.",
            json!({
                "object": object(),
                "name": string("A new name."),
                "visible": boolean("Shown or hidden."),
                "position": xyz("A new position."),
            }),
            &[],
        ),
        tool("delete_object", "Delete an object. The last one cannot be deleted.", json!({ "object": object() }), &[]),
        tool(
            "open_file",
            "Open a .sclp file in place of the scene, or add an .obj or .stl model to it as a new object.",
            json!({ "path": string("Full path of the file.") }),
            &["path"],
        ),
        tool(
            "save_file",
            "Save to a file, replacing it if it exists. A .sclp path saves the whole scene; an .obj or .stl \
             path exports the active object where it stands in the scene (STL in millimetres, Z up, 1 unit = 10 mm).",
            json!({ "path": string("Full path to write, ending in .sclp, .obj or .stl.") }),
            &["path"],
        ),
        tool(
            "set_options",
            "Change how the app displays things. Only what is given changes.",
            json!({
                "matcap": boolean("Matcap shading; off is plain lit shading."),
                "flat_shading": boolean("Faceted shading, which shows the triangles."),
                "cavity": boolean("Darken valleys and brighten ridges."),
                "symmetry_guides": boolean("Show the mirror planes and the radial symmetry sectors."),
                "panels": boolean("Show the interface panels (false is the Tab clean view)."),
                "interface_scale": number("Interface size, 0.75 to 3."),
            }),
            &[],
        ),
    ])
}
