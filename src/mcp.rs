//! `sculpt.exe --mcp`: the Model Context Protocol server Claude connects to.
//!
//! It speaks MCP (JSON-RPC, one message per line) on stdin and stdout and
//! passes each tool call to the running app's bridge (`app::bridge`) over a
//! loopback socket. It keeps nothing between calls, so the app can be closed,
//! rebuilt and started again underneath a connected Claude session.
//!
//! The app only listens to a connection that shows its key first
//! (`crate::bridge_guard`). Each run of the app makes a new key and leaves it
//! in a file in the settings folder; this server reads the file every time it
//! connects, so an app that has been restarted is picked up without anyone
//! doing anything. Whether the app is running is never taken from that file
//! (a crashed run leaves it behind) but from the port: nothing listening is
//! "not running", and a listener that will not take the key is a build that
//! does not match this one, which is said in words that tell the user what to
//! restart. Neither case waits. An app that gives the server no turn has a
//! crowd at its port; that is tried again a few times and then said for
//! what it is, since rebuilding would not cure it.
//!
//! This server usually runs from a copy of the program (`Sculpt-mcp.bat`
//! copies it to `target\mcp` when a Claude session starts), so it can be
//! older or newer than the app it talks to. The handshake has a version for
//! that; the tools do not need one, since a tool the app does not have is
//! answered with an ordinary error.

use std::io::{BufRead, BufReader, ErrorKind, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::bridge_guard as guard;
use crate::prefs::Home;

const DEFAULT_PORT: u16 = 47_823;
const PROTOCOL: &str = "2025-06-18";
/// Longer than the app's own limit, so its answer is the one that gets back.
const REPLY_TIMEOUT: Duration = Duration::from_secs(150);
const LAUNCH_TIMEOUT: Duration = Duration::from_secs(60);
const NOT_RUNNING: &str = "Sculpt is not running. Call launch to start it.";
/// How long to leave an app that turned the key down before one more try:
/// it may have started that very moment, its new key not yet in the file.
const KEY_RETRY: Duration = Duration::from_millis(300);
const KEY_RETRY_WITHIN: Duration = Duration::from_secs(1);
/// How many more tries an app that gave this server no turn gets, and how
/// long it is left between them. It turns out the connection that has waited
/// longest when too many are waiting, which a server showing its key can
/// only be by a hair, so the next try gets in.
const TURN_RETRIES: usize = 4;
const TURN_RETRY: Duration = Duration::from_millis(100);
/// The option this program is run with to be the go-between that starts the
/// app (see [`start_and_leave`]).
pub const START_APP: &str = "--start-app";

const INSTRUCTIONS: &str = "Sees and drives Sculpt, the user's sculpting app, while it is running. \
Call launch first if a tool says the app is not running. Look with screenshot and get_state before \
acting. Screen positions are fractions of the window: [0, 0] is the top-left corner and [1, 1] the \
bottom-right. Everything that changes the model is one undo step in the app, and the user may be \
working in the same window, so do not undo more than you did yourself. quit and open_file do nothing \
while the scene has unsaved work, or work that is in no file and would be lost (they name it), unless told \
to discard it: ask the user before you tell them to.";

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
    keep_the_line_private();
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

/// This server's input and output are its line to Claude, and nobody
/// else's. Windows hands a program that is started every handle of its
/// parent's that is marked to be passed on, and these arrive marked so. Left
/// alone, the app `launch` starts would hold the line open after this server
/// had gone, and the Claude session it belonged to would never see it close.
/// (The `cmd /c start` that `launch` once went through passed them on too.)
#[cfg(windows)]
fn keep_the_line_private() {
    use std::os::windows::io::AsRawHandle;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn SetHandleInformation(handle: *mut std::ffi::c_void, mask: u32, flags: u32) -> i32;
    }
    const INHERIT: u32 = 1;
    let (input, output, errors) = (std::io::stdin(), std::io::stdout(), std::io::stderr());
    for handle in [input.as_raw_handle(), output.as_raw_handle(), errors.as_raw_handle()] {
        if !handle.is_null() {
            // SAFETY: a handle of this process's own, open for as long as the
            // process is; the call only clears a flag on it, and may fail.
            unsafe { SetHandleInformation(handle, INHERIT, 0) };
        }
    }
}

#[cfg(not(windows))]
fn keep_the_line_private() {}

/// Why there is no link to the app.
#[derive(Debug, PartialEq)]
enum Trouble {
    /// Nothing is listening on the bridge's port.
    NotRunning,
    /// Something is, but it and this server do not match, or it has no room.
    /// The words say what the user has to do about it.
    Unmatched(String),
    /// The app closed the connection before looking at the key: no turn.
    Crowded,
}

impl Trouble {
    fn words(self) -> String {
        match self {
            Trouble::NotRunning => NOT_RUNNING.to_owned(),
            Trouble::Unmatched(why) => why,
            Trouble::Crowded => guard::NO_TURN.to_owned(),
        }
    }

    /// What an answer that is not a welcome comes to. Every word of it is
    /// this program's own: nothing the listener sent is passed on to Claude.
    fn of(answer: guard::Answer) -> Option<Trouble> {
        match answer {
            guard::Answer::Welcome => None,
            guard::Answer::Refused => Some(Trouble::Unmatched(guard::REFUSAL.to_owned())),
            guard::Answer::Full => Some(Trouble::Unmatched(guard::FULL.to_owned())),
            guard::Answer::NoTurn => Some(Trouble::Crowded),
        }
    }
}

/// What is said when the port answers but there is no key to show it.
fn no_key(port: u16, folder: Option<&Path>) -> String {
    let place = folder.map_or("its settings folder".to_owned(), |folder| folder.display().to_string());
    format!(
        "Something is answering on Sculpt's port ({port}), but no running copy of Sculpt has left a key for Claude \
         in {place}. If it is Sculpt, it is an older build than this Claude session's server, or it was started \
         without a settings folder: close it and start it again, rebuilding it first if it is old."
    )
}

/// A link to the running app, with the key shown and accepted.
fn connect() -> Result<BufReader<TcpStream>, Trouble> {
    let began = Instant::now();
    let mut tried = link();
    // No turn: the app made room for a newer connection by closing this
    // one, so it goes back to the door.
    for _ in 0..TURN_RETRIES {
        if !matches!(tried, Err(Trouble::Crowded)) {
            break;
        }
        std::thread::sleep(TURN_RETRY);
        tried = link();
    }
    match tried {
        // Turned down at once: perhaps only because the app started this
        // very moment. A listener that kept this waiting is not that, and
        // is not asked twice.
        Err(Trouble::Unmatched(_)) if began.elapsed() < KEY_RETRY_WITHIN => {
            std::thread::sleep(KEY_RETRY);
            link()
        }
        other => other,
    }
}

/// One try at it. The port is asked first: whether the app is running is
/// for the port to say, never for a key file a crashed run may have left.
fn link() -> Result<BufReader<TcpStream>, Trouble> {
    let port = port();
    let address = SocketAddr::from((Ipv4Addr::LOCALHOST, port));
    let stream = TcpStream::connect_timeout(&address, Duration::from_millis(500)).map_err(|_| Trouble::NotRunning)?;
    let folder = Home::of_the_app();
    let key = folder.as_deref().and_then(|folder| guard::read_key(folder, port));
    let key = key.ok_or_else(|| Trouble::Unmatched(no_key(port, folder.as_deref())))?;
    let refused = || Trouble::Unmatched(guard::REFUSAL.to_owned());
    // The answer to the key comes at once or not at all.
    let limits = stream.set_read_timeout(Some(guard::HELLO_WAIT)).and_then(|()| stream.set_nodelay(true));
    limits.map_err(|_| refused())?;
    // The app says one line and then waits to be asked, so the reader used
    // for the answer holds nothing back when it is let go.
    if let Some(trouble) = Trouble::of(guard::knock(&mut BufReader::new(&stream), &mut &stream, &key)) {
        return Err(trouble);
    }
    stream.set_read_timeout(Some(REPLY_TIMEOUT)).map_err(|_| refused())?;
    Ok(BufReader::new(stream))
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
                self.link = Some(connect().map_err(Trouble::words)?);
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
        let file = args.get("file").and_then(Value::as_str).map(launch_file).transpose()?;
        match connect() {
            Ok(link) => {
                self.link = Some(link);
                return done(match file {
                    Some(_) => "Sculpt is already running. Use open_file to open the file in it.",
                    None => "Sculpt is already running.",
                });
            }
            Err(Trouble::NotRunning) => {}
            // Something has the port already: a second copy would get no bridge.
            Err(unmatched) => return Err(unmatched.words()),
        }
        let app = self.app.clone().filter(|p| p.is_file()).ok_or_else(|| {
            let shown = self.app.as_ref().map_or("sculpt.exe".into(), |p| p.display().to_string());
            format!("Sculpt has not been built yet: {shown} is missing. Build it with `cargo build --release`.")
        })?;
        let started = start(&app, file.as_deref()).map_err(|e| format!("Could not start {}: {e}", app.display()))?;
        let began = Instant::now();
        let mut trouble = Trouble::NotRunning;
        while began.elapsed() < LAUNCH_TIMEOUT {
            std::thread::sleep(Duration::from_millis(250));
            match connect() {
                Ok(link) => {
                    self.link = Some(link);
                    return done("Sculpt is running.");
                }
                Err(why) => trouble = why,
            }
            // Gone already: no use waiting the full minute for its bridge.
            if !guard::process_is_running(started) {
                return Err(format!(
                    "{} closed again straight after it was started, so there is nothing to connect to. \
                     Starting it by hand (Sculpt.bat) will show what it has to say.",
                    app.display()
                ));
            }
        }
        Err(match trouble {
            Trouble::NotRunning => {
                "Sculpt was started but its bridge did not answer. Another program may be using the port.".to_owned()
            }
            other => other.words(),
        })
    }
}

/// The `file` argument of `launch` as a path the app can be handed: on this
/// computer's own drives (looked at before the disk is, see
/// `bridge_guard::off_limits`), full, and there.
///
/// Being full is also what keeps a file from being taken for an option. The
/// app reads any argument starting with `--` as one (`prefs::is_option`, by
/// which `main.rs` goes), and a full path starts with a drive letter (a `/`
/// off Windows), whatever the file itself is called: `C:\Models\--demo.sclp`
/// is a file, and a bare `--demo` is refused here.
fn launch_file(given: &str) -> Result<PathBuf, String> {
    if let Some(why) = guard::off_limits(given) {
        return Err(why.to_owned());
    }
    let path = PathBuf::from(given);
    if !path.is_absolute() {
        return Err(format!("{given} is not a full path. Give one that starts at the drive."));
    }
    if !path.is_file() {
        return Err(format!("There is no file at {}.", path.display()));
    }
    Ok(path)
}

/// The command that starts the app, on its own and not through a shell: the
/// program is run directly and the file is one argument of its own, so
/// nothing in a file's name (a space, `&`, `"`, `%`, `^`) can be read as
/// anything but part of the name. (`tests/launch_args.rs` starts a program
/// this way, through the real go-between, and has it write down what it was
/// given.)
///
/// The app gets no console, input or output of whoever starts it, and is
/// not waited for.
fn launch_command(app: &Path, file: Option<&Path>) -> Command {
    let mut command = Command::new(app);
    command.args(file);
    if let Some(folder) = app.parent().filter(|folder| !folder.as_os_str().is_empty()) {
        command.current_dir(folder);
    }
    command.stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null());
    detach(&mut command);
    command
}

/// Cuts the new process loose from this one's console and its Ctrl+C.
#[cfg(windows)]
fn detach(command: &mut Command) {
    use std::os::windows::process::CommandExt;
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP);
}

#[cfg(not(windows))]
fn detach(_: &mut Command) {}

/// The command that runs this program as the go-between: the option, then
/// the app, then the file if there is one, each an argument of its own.
fn errand_command(me: &Path, app: &Path, file: Option<&Path>) -> Command {
    let mut errand = Command::new(me);
    errand.arg(START_APP).arg(app).args(file).stdin(Stdio::null());
    detach(&mut errand);
    errand
}

/// Starts the app by way of the go-between ([`start_and_leave`]) and gives
/// back the app's process number.
fn start(app: &Path, file: Option<&Path>) -> Result<u32, String> {
    let me = std::env::current_exe().map_err(|e| e.to_string())?;
    let done = errand_command(&me, app, file).output().map_err(|e| e.to_string())?;
    let started = String::from_utf8_lossy(&done.stdout).trim().parse::<u32>().ok().filter(|_| done.status.success());
    started.ok_or_else(|| match String::from_utf8_lossy(&done.stderr).trim() {
        "" => "the program that starts it did not say why".to_owned(),
        why => why.to_owned(),
    })
}

/// `sculpt.exe --start-app <app> [file]`: starts the app, prints its process
/// number and exits at once. `args` is what follows the option.
///
/// This is the step between the server and the app. The app is the user's
/// window and must outlive the Claude session that opened it; started by the
/// server itself it would be the server's child, and whatever ends a session
/// by ending the server "and everything it started" would close the app and
/// lose the work in it. Started by a go-between that has already gone, it is
/// nobody's child. `cmd /c start` used to be the go-between, with the file's
/// name pasted into its command line, where a `"` or an `&` in the name was
/// read as a command; this one hands the name on as the argument it is.
pub fn start_and_leave(args: &[String]) {
    // The app must not be handed the pipe this number is printed to.
    keep_the_line_private();
    let Some(app) = args.first() else { std::process::exit(2) };
    match launch_command(Path::new(app), args.get(1).map(Path::new)).spawn() {
        Ok(started) => println!("{}", started.id()),
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    }
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
            json!({
                "file": string(
                    "Full path (starting at the drive, on this computer) of a .sclp, .sculpt (Unity app), .obj or \
                     .stl file to open at start.",
                ),
            }),
            &[],
        ),
        tool(
            "quit",
            "Close the Sculpt app. Needed before rebuilding it, since Windows will not replace a running program. \
             If the scene has unsaved work the app stays open and says so: ask the user what to do with the work, \
             never decide for them. Then call quit again with save: true or with discard: true. It also stays open \
             over work that is in no file and that saving does not keep (SSpheres not yet made into a skin, \
             unmerged tubes, planned keys, a lathe outline): the answer names it, and only discard: true closes \
             over it.",
            json!({
                "save": boolean(
                    "Save the scene to its own .sclp file first, then close. A scene that has never been saved has \
                     no such file: use save_file with a path, then quit.",
                ),
                "discard": boolean(
                    "Close without saving; the unsaved work, and any work that is in no file, is lost for good. \
                     Only when the user has said so.",
                ),
            }),
            &[],
        ),
        tool(
            "get_state",
            "Everything about the app right now, as JSON: window size, the objects (id, name, size, triangle \
             count, each one's own symmetry, and the on_screen box [left, top, right, bottom] each covers, as \
             fractions of the window), the brush and its settings (with the active object's symmetry, which is \
             the one strokes are made under), the camera, undo steps, the status line and the display options.",
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
            "Choose the brush and its settings. Only what is given changes. Returns the brush as it now is. \
             The symmetry settings (mirror_x, mirror_y, mirror_z, radial, radial_axis, radial_custom_axis, \
             symmetry_space) are not the brush's: each object has its own, and these set the active object's. \
             Select another object and its own symmetry is what strokes are made under.",
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
                "radial_custom_axis": xyz("Any other axis for it, as a direction in the active object's own space (the world's, with symmetry_space world)."),
                "symmetry_space": choice(&["local", "world"], "Where the active object's mirror planes and radial axis sit: local is through the object's own centre along its own axes, moving with it; world is through the world's centre along the world's axes, staying put while the object moves through them. The app starts with local."),
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
            "Open a .sclp file (or a .sculpt from the Unity app) in place of the scene, or add an .obj or .stl model \
             to it as a new object. A sculpt is not opened over a scene with unsaved work, or with work on its \
             objects that is in no file (unmerged tubes, planned keys): the answer says so and names it, and the \
             user is to be asked what to do with the work first.",
            json!({
                "path": string("Full path of the file, on this computer's own drives."),
                "discard": boolean(
                    "Open a sculpt even though the scene has unsaved work, which is then put aside, or work that \
                     is in no file, which is lost. Only when the user has said so.",
                ),
            }),
            &["path"],
        ),
        tool(
            "save_file",
            "Save to a file, replacing it if it exists. A .sclp path saves the whole scene; an .obj or .stl \
             path exports the active object where it stands in the scene (STL in millimetres, Z up, 1 unit = 10 mm).",
            json!({
                "path": string("Full path to write, on this computer's own drives, ending in .sclp, .obj or .stl."),
            }),
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

#[cfg(test)]
mod tests {
    use std::ffi::OsStr;

    use super::*;

    /// File names with everything a shell would trip over. (`"` cannot be in
    /// a Windows file name, but it can be in what a caller sends.)
    const AWKWARD: [&str; 8] = [
        "head.sclp",
        "two words.sclp",
        "this & that.obj",
        "50% %PATH% %USERPROFILE%.stl",
        "caret ^ and (brackets) 'quote'.sclp",
        "-leading-dash.sclp",
        "--demo",
        "--mcp",
    ];

    /// No shell stands between the server, the go-between and the app. (That Windows then hands the
    /// name over whole, whatever is in it, is `tests/launch_args.rs`: it runs the real go-between.)
    #[test]
    fn the_app_is_started_directly_with_the_file_as_one_argument() {
        let app = Path::new("C:\\Sculpt\\target\\release\\sculpt.exe");
        for name in AWKWARD.into_iter().chain(["said \"quoted\" & echo gotcha > x.txt", "a\" & calc & rem \""]) {
            let file = Path::new("C:\\Models").join(name);
            let command = launch_command(app, Some(&file));
            // The program itself, not a shell, and the file whole and alone after it.
            assert_eq!(command.get_program(), app.as_os_str(), "{name}");
            assert_eq!(command.get_args().collect::<Vec<&OsStr>>(), [file.as_os_str()], "{name}");
            assert_eq!(command.get_current_dir(), app.parent(), "{name}");
        }
        let bare = launch_command(app, None);
        assert_eq!((bare.get_program(), bare.get_args().count()), (app.as_os_str(), 0));
        // The go-between is this program, not a shell, and is handed the same things whole.
        let me = Path::new("C:\\Sculpt\\target\\mcp\\sculpt-mcp.exe");
        for name in AWKWARD {
            let file = Path::new("C:\\Models").join(name);
            let errand = errand_command(me, app, Some(&file));
            assert_eq!(errand.get_program(), me.as_os_str(), "{name}");
            let expected = [OsStr::new(START_APP), app.as_os_str(), file.as_os_str()];
            assert_eq!(errand.get_args().collect::<Vec<&OsStr>>(), expected, "{name}");
        }
        assert_eq!(errand_command(me, app, None).get_args().count(), 2);
        // A program named without a folder is not given an empty one to start in.
        assert_eq!(launch_command(Path::new("sculpt.exe"), None).get_current_dir(), None);
    }

    #[test]
    fn a_file_to_launch_with_must_be_a_full_local_path_to_a_real_file() {
        let folder = std::env::temp_dir().join(format!("sculpt-launch-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        for name in AWKWARD {
            let file = folder.join(name);
            std::fs::write(&file, b"").unwrap();
            let given = file.to_string_lossy().into_owned();
            let path = launch_file(&given).unwrap_or_else(|e| panic!("{given} was refused: {e}"));
            assert_eq!(path, file);
            // The name alone is refused: it could be anything, an option included.
            let alone = launch_file(name).expect_err(name);
            assert!(alone.contains("not a full path"), "{name}: {alone}");
        }
        let gone = folder.join("gone.sclp").to_string_lossy().into_owned();
        assert!(launch_file(&gone).is_err_and(|e| e.starts_with("There is no file at")));
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// The app's own reading of its arguments (`main.rs`), given what `launch` hands it.
    fn as_the_app_reads(args: &[String]) -> (Option<crate::Errand>, crate::app::Args) {
        (crate::errand(args), crate::window_options(args))
    }

    #[test]
    fn a_file_launch_hands_the_app_is_never_read_as_an_option() {
        let folder = std::env::temp_dir().join(format!("sculpt-launch-options-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();
        // Every option the app has that could do something a file name should not, as a file's whole name.
        let options = ["--mcp", "--start-app", "--bench", "--bench-volume", "--demo", "--screenshot", "--record-loop"];
        for name in AWKWARD.into_iter().chain(options) {
            let file = folder.join(name);
            std::fs::write(&file, b"").unwrap();
            let path = launch_file(&file.to_string_lossy()).unwrap_or_else(|e| panic!("{name} was refused: {e}"));
            // What the app is started with: the arguments of the very command `launch` builds.
            let command = launch_command(Path::new("C:\\Sculpt\\sculpt.exe"), Some(&path));
            let args: Vec<String> = command.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
            assert_eq!(args.len(), 1, "{name}");
            // The app reads it as the file to open and as nothing else: no errand, no option set.
            assert!(!crate::prefs::is_option(&args[0]), "{name}");
            let (errand, window) = as_the_app_reads(&args);
            assert_eq!(errand, None, "{name}");
            assert_eq!(window, crate::app::Args { open: Some(file.clone()), ..Default::default() }, "{name}");
            // The go-between is handed the same file after its own option, which is the one it goes by.
            let me = Path::new("C:\\Sculpt\\target\\mcp\\sculpt-mcp.exe");
            let errand = errand_command(me, Path::new("C:\\Sculpt\\sculpt.exe"), Some(&path));
            let args: Vec<String> = errand.get_args().map(|arg| arg.to_string_lossy().into_owned()).collect();
            assert_eq!(crate::errand(&args), Some(crate::Errand::StartApp), "{name}");
        }
        // The same names bare are what the app would read as options, which is why `launch_file` has to
        // refuse them (it does: see the test above). This is the reading the full path keeps them from.
        for name in options {
            assert!(crate::prefs::is_option(name), "{name}");
            let (errand, window) = as_the_app_reads(&[name.to_owned()]);
            assert!(errand.is_some() || window.open.is_none(), "{name} was read as a file");
            assert!(launch_file(name).is_err(), "{name}");
        }
        assert_eq!(crate::errand(&["--mcp".to_owned()]), Some(crate::Errand::Server));
        assert!(as_the_app_reads(&["--demo".to_owned()]).1.demo);
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_network_path_is_refused_before_the_disk_is_asked() {
        // These would otherwise make Windows sign in to "server" just to look.
        let network = [
            r"\\server\share\head.sclp",
            "//server/share/head.sclp",
            r"\\?\UNC\server\share\head.sclp",
            r"\\.\COM1",
        ];
        for given in network {
            assert_eq!(launch_file(given), Err(guard::NOT_LOCAL.to_owned()), "{given}");
        }
    }

    #[test]
    fn the_tools_say_how_unsaved_work_is_kept() {
        let tools = tools();
        let tool = |name: &str| tools.as_array().unwrap().iter().find(|t| t["name"] == name).unwrap().clone();
        let quit = tool("quit");
        assert!(quit["description"].as_str().unwrap().contains("ask the user"));
        assert_eq!(quit["inputSchema"]["properties"]["discard"]["type"], "boolean");
        assert_eq!(quit["inputSchema"]["properties"]["save"]["type"], "boolean");
        let open = tool("open_file");
        assert!(open["description"].as_str().unwrap().contains("unsaved work"));
        // And of the work that is in no file, which neither tool may drop on the quiet.
        for tool in [&quit, &open] {
            let said = tool["description"].as_str().unwrap();
            assert!(said.contains("in no file") && said.contains("unmerged tubes"), "{said}");
        }
        assert!(INSTRUCTIONS.contains("in no file"));
        assert_eq!(open["inputSchema"]["properties"]["discard"]["type"], "boolean");
        assert!(INSTRUCTIONS.contains("ask the user"));
    }

    #[test]
    fn what_went_wrong_is_said_in_words_that_name_the_cure() {
        assert_eq!(Trouble::NotRunning.words(), NOT_RUNNING);
        let words = no_key(47_823, Some(Path::new("C:\\Users\\me\\AppData\\Roaming\\Sculpt")));
        let says = |part: &str| words.contains(part);
        assert!(says("47823") && says("Roaming\\Sculpt") && says("start it again"), "{words}");
        assert!(guard::REFUSAL.contains("Restart the Claude session") && guard::REFUSAL.contains("rebuild"));
    }

    #[test]
    fn a_crowded_door_is_not_called_a_build_that_does_not_match() {
        use guard::Answer;
        assert_eq!(Trouble::of(Answer::Welcome), None);
        assert_eq!(Trouble::of(Answer::Refused), Some(Trouble::Unmatched(guard::REFUSAL.to_owned())));
        assert_eq!(Trouble::of(Answer::NoTurn), Some(Trouble::Crowded));
        assert_eq!(Trouble::of(Answer::Full), Some(Trouble::Unmatched(guard::FULL.to_owned())));
        // Closed without a word: that is said, with what to do, and nobody is sent off to rebuild.
        let words = Trouble::Crowded.words();
        let says = |part: &str| words.contains(part);
        assert!(says("could not get a turn") && says("Try again") && says("another program"), "{words}");
        assert!(!says("same build") && !says("rebuild"), "{words}");
        assert!(guard::FULL.contains("Close the Claude sessions") && !guard::FULL.contains("same build"));
    }
}
