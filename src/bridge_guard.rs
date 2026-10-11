//! What stands between the Claude bridge's socket and the app: the key a
//! connection must show, the file that hands that key to Claude's server, the
//! handshake the two sides speak, the limits on a line, and which paths the
//! bridge will touch. `app::bridge` (the app's side) and `mcp` (the server
//! Claude starts) both use it.
//!
//! Why any of it is needed. The bridge listens on this machine only, but
//! "this machine" includes every web page open in a browser: a page can make
//! the browser send a request to 127.0.0.1, and a bridge that reads a line at
//! a time and passes over what it cannot read finds a line of JSON in the
//! body of that request and runs it. So:
//!
//! - **The key.** Each run of the app makes a new one from the operating
//!   system's random numbers (32 bytes, written as 64 hex digits) and keeps it
//!   in `claude-bridge-<port>.json` in the settings folder, beside the port,
//!   the app's process number and the handshake's version. The file is put in
//!   place whole (`io::replace_file`) and removed when the app closes
//!   properly. The settings folder is the user's own (`%APPDATA%\Sculpt`),
//!   which other accounts on the machine cannot read; a web page cannot read
//!   it at all. A run with no settings folder has nowhere to leave a key, so
//!   it has no bridge.
//! - **The handshake.** The first line of every connection must be
//!   `{"sculpt_bridge":1,"token":"<key>"}`. The app answers
//!   `{"ok":true,"sculpt_bridge":1}` and only then takes requests. Anything
//!   else first (a wrong key, another version, `POST / HTTP/1.1`, noise, an
//!   empty line, a line over a kilobyte, nothing at all for five seconds)
//!   gets one fixed line of refusal and the connection is closed, with
//!   nothing run. The refusal is the same whatever was wrong, so it tells a
//!   prober nothing, and it is shaped like an ordinary failed reply so that a
//!   server from a build before the key existed shows its words to Claude
//!   instead of a bare "connection lost".
//! - **After it.** Every line must be a request: a JSON object naming a
//!   tool. The first that is not (or is longer than [`REQUEST_LIMIT`]) closes
//!   the connection; nothing after it is read.
//! - **A stale file.** A run that crashed leaves its file behind. The server
//!   never takes the file's word that the app is running: it connects first
//!   (nothing listening means "not running"), it does not show a key whose
//!   process is gone, and a key the listener does not accept means the two do
//!   not match, said in plain words. None of these waits.
//! - **Room.** Connections that have shown the key and connections that have
//!   not are counted apart ([`Door`]), so that no number of the second kind
//!   can keep one of the first kind out: any program on this machine can open
//!   connections and say nothing. When the silent ones are at their limit,
//!   the next arrival takes the place of the one that has waited longest. A
//!   real server shows its key the moment it connects, so it is never the one
//!   that has waited longest for more than an instant. The two answers this
//!   adds (turned out to make room; the right key but no seat left) each say
//!   what they are, so that neither is put down to builds that do not match.
//! - **Paths.** [`off_limits`] refuses network shares, drive letters that
//!   stand for one, and device names, so a request cannot make the app reach
//!   out to another machine or write to a device.

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, ErrorKind, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

/// The handshake's version. Both sides must have the same one; it changes
/// only if the handshake itself does, not when a tool is added.
pub const VERSION: u64 = 1;
const KEY_BYTES: usize = 32;
/// The longest first line that is read. A real one is about a hundred bytes.
const HELLO_LIMIT: usize = 1024;
/// The longest request that is read. The largest real one is a stroke with
/// thousands of points, some hundreds of kilobytes.
pub const REQUEST_LIMIT: usize = 4 << 20;
/// How long a new connection has to show its key, all told: a connection
/// that says nothing, or dribbles a byte at a time, is closed when it is up.
pub const HELLO_WAIT: Duration = Duration::from_secs(5);
/// How often a connection that has not shown its key yet is looked at for
/// whether it has been turned out to make room, or has run out of time. Its
/// first line is read the moment it arrives, not at the next look.
const LOOK_EVERY: Duration = Duration::from_millis(10);
/// How long a reply may take to send. A peer that has stopped reading would
/// otherwise hold its connection's thread for good.
const SEND_WAIT: Duration = Duration::from_secs(60);
/// How many connections may be waiting to show their key at once. A real
/// server waits for as long as one line takes to cross, so this is only ever
/// reached by connections that are not going to show one.
pub const MAX_WAITING: usize = 64;
/// How many connections that have shown the key there may be at once. Each
/// Claude session holds one. [`FULL`] says the same number.
pub const MAX_ADMITTED: usize = 64;

/// What a connection that is turned down is told, whatever it did wrong.
pub const REFUSAL: &str = "Sculpt turned this connection down: Claude's Sculpt server and the app do not come \
from the same build. Restart the Claude session (or reconnect its sculpt server with /mcp); if that is not \
enough, close Sculpt, rebuild it and start it again.";

/// What a connection with the right key is told when [`MAX_ADMITTED`] others
/// are connected already.
pub const FULL: &str = "Sculpt already has 64 of Claude's sessions connected, which is as many as it takes at \
once. Close the Claude sessions that are no longer in use (each one holds a connection), then try again. Closing \
Sculpt and starting it again clears them all.";

/// What is said when the app turned a connection out before looking at its
/// key, which it does to the one that has waited longest when too many are
/// waiting ([`Door`]). Claude's server says the same of a connection that
/// was closed without a word.
pub const NO_TURN: &str = "Sculpt is running, but Claude's Sculpt server could not get a turn: the app closed the \
connection before it had looked at the server's key. It does that when a crowd of connections is waiting at its \
port, and Claude's own servers are never such a crowd, so another program on this computer is probably opening \
them. Try again. If it keeps happening, close Sculpt and start it again; if that does not cure it, restart the \
computer.";

/// What the key file says of itself, for whoever opens it.
const ABOUT: &str = "The key Claude's Sculpt server shows the running app before the app will listen to it. \
A new one is made every time the app starts, and this file is removed when the app closes.";

/// What is said of a path [`off_limits`] refuses.
pub const NOT_LOCAL: &str = "Claude's Sculpt tools only open and write files on this computer's own drives, \
by a full path such as C:\\Models\\head.sclp. A network share (\\\\server\\folder, or a drive letter that stands \
for one) or a device is refused.";

// ---- the key

/// One run's key: 64 hex digits. It is never printed or logged.
#[derive(Clone)]
pub struct Key(String);

impl Key {
    /// A new key from the operating system's random numbers.
    pub fn new() -> std::io::Result<Key> {
        let mut bytes = [0u8; KEY_BYTES];
        os_random(&mut bytes)?;
        Ok(Key(bytes.iter().map(|byte| format!("{byte:02x}")).collect()))
    }

    /// A key as its file holds it, if it has the shape of one.
    fn from_text(text: &str) -> Option<Key> {
        let shaped = text.len() == KEY_BYTES * 2 && text.bytes().all(|b| b.is_ascii_hexdigit());
        shaped.then(|| Key(text.to_owned()))
    }

    /// Whether `shown` is this key. Every byte is compared whatever the
    /// first difference, so how long the answer takes says nothing about how
    /// much of a guess was right.
    fn matches(&self, shown: &str) -> bool {
        let (ours, theirs) = (self.0.as_bytes(), shown.as_bytes());
        if ours.len() != theirs.len() {
            return false;
        }
        let differing = ours.iter().zip(theirs).fold(0u8, |sum, (a, b)| sum | (a ^ b));
        std::hint::black_box(differing) == 0
    }
}

#[cfg(windows)]
fn os_random(out: &mut [u8]) -> std::io::Result<()> {
    #[link(name = "bcrypt")]
    unsafe extern "system" {
        fn BCryptGenRandom(algorithm: *mut std::ffi::c_void, buffer: *mut u8, length: u32, flags: u32) -> i32;
    }
    /// "Use the system's preferred generator", which is what a null algorithm asks for.
    const SYSTEM_PREFERRED: u32 = 2;
    // SAFETY: `out` is valid for writes of its own length, and with this
    // flag the call takes no algorithm handle.
    let status = unsafe { BCryptGenRandom(std::ptr::null_mut(), out.as_mut_ptr(), out.len() as u32, SYSTEM_PREFERRED) };
    match status {
        0 => Ok(()),
        _ => Err(std::io::Error::other(format!("no random numbers from Windows ({status:#x})"))),
    }
}

#[cfg(not(windows))]
fn os_random(out: &mut [u8]) -> std::io::Result<()> {
    std::fs::File::open("/dev/urandom")?.read_exact(out)
}

// ---- the key's file

/// The key file as it is written.
#[derive(Serialize, Deserialize)]
struct KeyFile {
    #[serde(default)]
    about: String,
    port: u16,
    pid: u32,
    version: u64,
    token: String,
}

/// Where the key for the bridge on `port` is kept. The port is in the name
/// so that a second copy of the app on a port of its own (a test, say) does
/// not take the first one's file.
pub fn key_file(folder: &Path, port: u16) -> PathBuf {
    folder.join(format!("claude-bridge-{port}.json"))
}

/// A key on disk for as long as this lives: dropping it removes the file.
pub struct Posted {
    file: PathBuf,
    key: Key,
}

impl Posted {
    /// Writes `key` where Claude's server will look for it, replacing a file
    /// an earlier run left, whole or not at all.
    pub fn post(folder: &Path, port: u16, key: &Key) -> Result<Posted, String> {
        let file = key_file(folder, port);
        let (about, pid) = (ABOUT.to_owned(), std::process::id());
        let card = KeyFile { about, port, pid, version: VERSION, token: key.0.clone() };
        let text = serde_json::to_string_pretty(&card).map_err(|e| format!("Could not write {}: {e}", file.display()))?;
        crate::io::replace_file(&file, |w| w.write_all(text.as_bytes()))?;
        Ok(Posted { file, key: key.clone() })
    }
}

impl Drop for Posted {
    /// The app is closing. The file goes, unless a later run has already put
    /// its own key there: that one is not this run's to remove.
    fn drop(&mut self) {
        if read_file(&self.file).is_some_and(|found| self.key.matches(&found.key.0)) {
            let _ = std::fs::remove_file(&self.file);
        }
    }
}

/// A key file's contents.
struct Found {
    key: Key,
    /// The process that wrote it, and the port it was listening on.
    pid: u32,
    port: u16,
}

fn read_file(file: &Path) -> Option<Found> {
    let card: KeyFile = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    Some(Found { key: Key::from_text(&card.token)?, pid: card.pid, port: card.port })
}

/// The key the app listening on `port` left in `folder`: `None` if there is
/// no file that reads as one, if it is another port's, or if the process
/// that wrote it is no longer running. That last is a run that crashed, and
/// its key is not shown to whatever has the port now.
///
/// A key coming back does not mean the app is running (the process number
/// may belong to something else by now): only a listener accepting it does.
pub fn read_key(folder: &Path, port: u16) -> Option<Key> {
    let found = read_file(&key_file(folder, port))?;
    (found.port == port && process_is_running(found.pid)).then_some(found.key)
}

/// Whether there is a process with this number.
#[cfg(windows)]
pub fn process_is_running(pid: u32) -> bool {
    use std::ffi::c_void;
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> *mut c_void;
        fn GetExitCodeProcess(process: *mut c_void, code: *mut u32) -> i32;
        fn CloseHandle(handle: *mut c_void) -> i32;
    }
    const QUERY_LIMITED_INFORMATION: u32 = 0x1000;
    const STILL_ACTIVE: u32 = 259;
    const ACCESS_DENIED: i32 = 5;
    // SAFETY: plain Win32 calls. The handle is used only if it was given,
    // and closed once; `code` outlives the call that fills it.
    unsafe {
        let process = OpenProcess(QUERY_LIMITED_INFORMATION, 0, pid);
        if process.is_null() {
            // Refused: it is there, but another account's. Anything else: it is not there.
            return std::io::Error::last_os_error().raw_os_error() == Some(ACCESS_DENIED);
        }
        let mut code = 0u32;
        let known = GetExitCodeProcess(process, &mut code) != 0;
        CloseHandle(process);
        !known || code == STILL_ACTIVE
    }
}

/// Linux can say. Where nothing can, the answer is yes and the port decides.
#[cfg(not(windows))]
pub fn process_is_running(pid: u32) -> bool {
    let processes = Path::new("/proc");
    !processes.is_dir() || processes.join(pid.to_string()).exists()
}

// ---- the handshake and the lines after it

/// One tool call, as a line after the handshake carries it.
pub struct Request {
    pub tool: String,
    pub args: Value,
}

/// One line, without its ending. `None` when there is not a whole one to be
/// had: the peer has gone, the wait is up, or the line is over `limit`.
fn read_line<R: BufRead>(input: &mut R, limit: usize) -> Option<Vec<u8>> {
    let mut line = Vec::new();
    input.take(limit as u64 + 1).read_until(b'\n', &mut line).ok()?;
    if line.pop() != Some(b'\n') {
        return None;
    }
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    Some(line)
}

/// Sends one line whole, its ending with it, in a single write: sent in two
/// parts, the ending can sit waiting for the other side to acknowledge the
/// first, and a connection closed straight after may never deliver it.
fn send_line<W: Write>(out: &mut W, mut line: String) -> std::io::Result<()> {
    line.push('\n');
    out.write_all(line.as_bytes())?;
    out.flush()
}

/// The first line a server sends: the key and the handshake's version.
pub fn hello(key: &Key) -> String {
    json!({ "sculpt_bridge": VERSION, "token": key.0 }).to_string()
}

/// What a connection is told about its first line.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Answer {
    /// It showed the key and has a seat: its requests will be run.
    Welcome,
    /// It did not show the key ([`REFUSAL`]).
    Refused,
    /// It showed the key, but as many as may be are connected already ([`FULL`]).
    Full,
    /// It was turned out to make room before its key was looked at ([`NO_TURN`]).
    NoTurn,
}

impl Answer {
    /// The line that says it. The words are for a server that knows no
    /// better than to show them. `reason` is what this build's server goes
    /// by ([`knock`]), so that what it tells Claude is always its own words
    /// and never those of whatever happened to be listening on the port.
    fn line(self) -> String {
        match self {
            Answer::Welcome => json!({ "ok": true, "sculpt_bridge": VERSION }),
            Answer::Refused => json!({ "ok": false, "text": REFUSAL }),
            Answer::Full => json!({ "ok": false, "reason": "full", "text": FULL }),
            Answer::NoTurn => json!({ "ok": false, "reason": "no turn", "text": NO_TURN }),
        }
        .to_string()
    }
}

/// Whether `line` is the handshake, with this run's key and this version.
fn shows_key(line: &[u8], key: &Key) -> bool {
    let Ok(Value::Object(said)) = serde_json::from_slice::<Value>(line) else { return false };
    let version = said.get("sculpt_bridge").and_then(Value::as_u64);
    let token = said.get("token").and_then(Value::as_str);
    // The key is looked at even if the version is wrong, so the two take as long.
    let right_key = key.matches(token.unwrap_or_default());
    right_key && version == Some(VERSION)
}

/// The app's side of the handshake: reads a connection's first line and
/// answers it. `door` is told whether the line showed the key, and says
/// what the answer is ([`Place::answer`]). True if the connection is welcome
/// and may go on; false if it is to be closed.
pub fn admit<R: BufRead, W: Write>(input: &mut R, out: &mut W, key: &Key, door: impl FnOnce(bool) -> Answer) -> bool {
    let shown = read_line(input, HELLO_LIMIT).is_some_and(|line| shows_key(&line, key));
    let answer = door(shown);
    let sent = send_line(out, answer.line()).is_ok();
    answer == Answer::Welcome && sent
}

/// The server's side of it: shows the key and reads how the app took it.
/// Anything that is not one of the app's own answers is a refusal: a wrong
/// key, another version of the handshake, something that is not Sculpt at
/// all, or nothing in the time allowed. A connection closed without a word
/// is no turn: that is how a build from before the door told a newcomer it
/// was full.
pub fn knock<R: BufRead, W: Write>(input: &mut R, out: &mut W, key: &Key) -> Answer {
    if send_line(out, hello(key)).is_err() {
        return Answer::NoTurn;
    }
    let mut line = Vec::new();
    let read = input.take(HELLO_LIMIT as u64 + 1).read_until(b'\n', &mut line);
    let waited_in_vain = read.is_err_and(|e| matches!(e.kind(), ErrorKind::TimedOut | ErrorKind::WouldBlock));
    if line.is_empty() && !waited_in_vain {
        return Answer::NoTurn;
    }
    if line.pop() != Some(b'\n') {
        return Answer::Refused;
    }
    let Ok(answer) = serde_json::from_slice::<Value>(&line) else { return Answer::Refused };
    let (ok, reason) = (answer["ok"].as_bool(), answer["reason"].as_str());
    match (ok, reason) {
        (Some(true), _) if answer["sculpt_bridge"].as_u64() == Some(VERSION) => Answer::Welcome,
        (Some(false), Some("full")) => Answer::Full,
        (Some(false), Some("no turn")) => Answer::NoTurn,
        _ => Answer::Refused,
    }
}

/// A request line: a JSON object with a tool's name, and its arguments if
/// it has any.
fn request(line: &[u8]) -> Option<Request> {
    let Ok(Value::Object(mut said)) = serde_json::from_slice::<Value>(line) else { return None };
    let tool = said.get("tool")?.as_str().filter(|name| !name.is_empty())?.to_owned();
    let args = match said.remove("args") {
        Some(args @ Value::Object(_)) => args,
        Some(Value::Null) | None => json!({}),
        Some(_) => return None,
    };
    Some(Request { tool, args })
}

/// After the handshake: hands each request to `run` and writes back the
/// line it returns, until the peer goes, a line is not a request, or `run`
/// has no answer (the app is closing).
pub fn requests<R: BufRead, W: Write>(input: &mut R, out: &mut W, mut run: impl FnMut(Request) -> Option<String>) {
    while let Some(request) = read_line(input, REQUEST_LIMIT).and_then(|line| request(&line)) {
        let Some(reply) = run(request) else { return };
        if send_line(out, reply).is_err() {
            return;
        }
    }
}

/// A connection's socket as its thread reads it. Until the key has been
/// shown, the thread has to be able to stop at two moments that no byte
/// from the peer announces: when the time for the first line is up, however
/// the bytes arrive (a read timeout alone starts again with every byte), and
/// when the door turns the connection out to make room. A read that is
/// already waiting cannot be relied on to end at either: on Windows, shutting
/// the socket down from another thread leaves it waiting, and a read that
/// times out leaves the connection in a state Windows will not vouch for. So
/// until then nothing waits in a read: the socket is asked what it has, and
/// if that is nothing the thread waits for something to arrive, for no
/// longer than [`LOOK_EVERY`], and looks again.
struct Timed<'a> {
    stream: &'a TcpStream,
    /// When the wait for the first line is up. `None` once the key has been
    /// shown: from then on a read waits for as long as it takes.
    until: Option<Instant>,
    /// Set by the door when it turns this connection out.
    turned_out: Arc<AtomicBool>,
}

impl Timed<'_> {
    fn new(stream: &TcpStream, hello_wait: Duration, turned_out: Arc<AtomicBool>) -> std::io::Result<Timed<'_>> {
        stream.set_nonblocking(true)?;
        Ok(Timed { stream, until: Some(Instant::now() + hello_wait), turned_out })
    }

    /// The key has been shown: from here the connection may sit idle between
    /// requests for as long as Claude does.
    fn wait_for_ever(&mut self) -> std::io::Result<()> {
        self.until = None;
        self.stream.set_nonblocking(false)
    }
}

impl Read for Timed<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
        let mut stream = self.stream;
        let Some(until) = self.until else { return stream.read(buffer) };
        loop {
            if self.turned_out.load(Ordering::Acquire) {
                return Err(ErrorKind::ConnectionAborted.into());
            }
            let left = until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(ErrorKind::TimedOut.into());
            }
            match stream.read(buffer) {
                Err(e) if e.kind() == ErrorKind::WouldBlock => wait_for_word(stream, left.min(LOOK_EVERY)),
                read => return read,
            }
        }
    }
}

/// Waits until `stream` has something to read (bytes, its end, or an error),
/// or until `at_most` has passed, whichever is first. The moment matters: a
/// server's key has to be read as it arrives, since until it has been read
/// the connection is one of the waiting, and can be turned out.
#[cfg(windows)]
fn wait_for_word(stream: &TcpStream, at_most: Duration) {
    use std::os::windows::io::AsRawSocket;
    #[repr(C)]
    struct Watched {
        socket: usize,
        asked: i16,
        seen: i16,
    }
    #[link(name = "ws2_32")]
    unsafe extern "system" {
        fn WSAPoll(watched: *mut Watched, count: u32, milliseconds: i32) -> i32;
    }
    /// "Tell me when there is something to read."
    const READABLE: i16 = 0x0100;
    let mut watched = Watched { socket: stream.as_raw_socket() as usize, asked: READABLE, seen: 0 };
    let milliseconds = at_most.as_millis().clamp(1, 1000) as i32;
    // SAFETY: one entry, as the count says, alive for the whole call; the
    // socket is open, since `stream` is borrowed for as long.
    let outcome = unsafe { WSAPoll(&mut watched, 1, milliseconds) };
    if outcome < 0 {
        // The wait could not be had. Sleeping keeps the looks apart all the same.
        std::thread::sleep(at_most);
    }
}

/// Elsewhere there is no such wait without another library, so the looks
/// are simply close together.
#[cfg(not(windows))]
fn wait_for_word(_: &TcpStream, at_most: Duration) {
    std::thread::sleep(at_most.min(Duration::from_millis(1)));
}

/// One connection to the app, start to finish: the handshake within
/// `hello_wait`, then requests for as long as they come. `place` is the
/// connection's place at the door, given up when this returns. Returns when
/// the connection is over; dropping the socket closes it.
///
/// The one socket is read and written as it is, never through a copy of it
/// (`try_clone`): Windows marks a copied socket to be passed on to any
/// program this one starts (the file manager, for one, when a recording's
/// folder is opened), and a connection such a program held open would look
/// alive to Claude's server long after the app had closed.
pub fn serve(
    stream: TcpStream,
    key: &Key,
    mut place: Place,
    hello_wait: Duration,
    run: impl FnMut(Request) -> Option<String>,
) {
    let mut out = &stream;
    if out.set_write_timeout(Some(SEND_WAIT)).is_err() {
        return;
    }
    let Ok(timed) = Timed::new(&stream, hello_wait, place.turned_out.clone()) else { return };
    let mut input = BufReader::new(timed);
    // The answer to the first line goes out while the socket still does not
    // wait. It is the first thing sent on the connection and a few hundred
    // bytes long, so there is always room for it to be taken whole.
    if admit(&mut input, &mut out, key, |shown| place.answer(shown)) && input.get_mut().wait_for_ever().is_ok() {
        requests(&mut input, &mut out, run);
    }
}

// ---- room

/// Who is connected: those that have not shown the key yet, and how many
/// that have.
///
/// One count for both would let anything on this machine keep Claude out,
/// since connecting and saying nothing costs nothing: enough silent
/// connections, opened again as each is timed out, and the count is always
/// full when Claude's server arrives. So a connection that has shown nothing
/// can only ever take the place of another that has shown nothing. Past
/// `max_waiting` of them, the one that has waited longest is turned out for
/// each that arrives. A real server sends its key as it connects and is read
/// at once, so the one that has waited longest is not a real server unless
/// `max_waiting` more connections arrive in the instant between its
/// connecting and its key being read; a server that loses that race is told
/// so ([`Answer::NoTurn`]) and tries again.
///
/// Connections that have shown the key have a limit of their own, so that
/// threads cannot pile up without end; one past it is told so ([`FULL`]).
pub struct Door {
    max_waiting: usize,
    max_admitted: usize,
    crowd: Mutex<Crowd>,
}

#[derive(Default)]
struct Crowd {
    /// The connections that have not shown the key, longest wait first: the
    /// number each was given on arriving, and the flag that turns it out.
    waiting: VecDeque<(u64, Arc<AtomicBool>)>,
    admitted: usize,
    arrivals: u64,
}

/// One connection's place at the [`Door`]. Dropping it gives the place up.
pub struct Place {
    door: Arc<Door>,
    number: u64,
    seated: bool,
    /// Set when the door turns the connection out; its thread looks at this
    /// between reads ([`Timed`]) and ends, which closes the connection.
    turned_out: Arc<AtomicBool>,
}

impl Door {
    pub fn new(max_waiting: usize, max_admitted: usize) -> Arc<Door> {
        Arc::new(Door { max_waiting: max_waiting.max(1), max_admitted, crowd: Mutex::default() })
    }

    /// The counts are plain numbers and a list of flags, right whatever a
    /// thread that held the lock was doing if it panicked.
    fn crowd(&self) -> MutexGuard<'_, Crowd> {
        self.crowd.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A connection has arrived and joins those waiting to show the key. If
    /// they are at their limit, the one that has waited longest is turned
    /// out first.
    pub fn arrive(self: &Arc<Door>) -> Place {
        let mut crowd = self.crowd();
        while crowd.waiting.len() >= self.max_waiting {
            let Some((_, longest)) = crowd.waiting.pop_front() else { break };
            longest.store(true, Ordering::Release);
        }
        crowd.arrivals += 1;
        let (number, turned_out) = (crowd.arrivals, Arc::new(AtomicBool::new(false)));
        crowd.waiting.push_back((number, turned_out.clone()));
        Place { door: self.clone(), number, seated: false, turned_out }
    }

    /// How many are waiting to show the key, and how many have.
    #[cfg(test)]
    fn counts(&self) -> (usize, usize) {
        let crowd = self.crowd();
        (crowd.waiting.len(), crowd.admitted)
    }
}

impl Place {
    /// What the connection is to be told, now that its first line is in
    /// (`shown` is whether it was the key) or is not coming. A connection
    /// that showed the key stops being one of the waiting and, if there is
    /// room, becomes one of the admitted. One that was turned out meanwhile
    /// is told that, key or no key.
    pub fn answer(&mut self, shown: bool) -> Answer {
        let mut crowd = self.door.crowd();
        let Some(at) = crowd.waiting.iter().position(|(number, _)| *number == self.number) else {
            return Answer::NoTurn;
        };
        if !shown {
            return Answer::Refused;
        }
        crowd.waiting.remove(at);
        if crowd.admitted >= self.door.max_admitted {
            return Answer::Full;
        }
        crowd.admitted += 1;
        self.seated = true;
        Answer::Welcome
    }
}

impl Drop for Place {
    fn drop(&mut self) {
        let mut crowd = self.door.crowd();
        if self.seated {
            crowd.admitted -= 1;
        } else {
            crowd.waiting.retain(|(number, _)| *number != self.number);
        }
    }
}

/// Takes connections for as long as the listener lives, giving each a place
/// at `door` and a thread of its own, so one that says nothing holds nobody
/// else up. `run` answers a request; every connection gets a copy of it.
pub fn listen<F>(listener: TcpListener, key: Key, door: Arc<Door>, hello_wait: Duration, run: F)
where
    F: FnMut(Request) -> Option<String> + Clone + Send + 'static,
{
    for stream in listener.incoming().flatten() {
        let (place, key, run) = (door.arrive(), key.clone(), run.clone());
        // No thread to be had: the connection is dropped, and the next may fare better.
        let _ = std::thread::Builder::new().spawn(move || serve(stream, &key, place, hello_wait, run));
    }
}

// ---- paths

/// The names Windows keeps for devices, in any folder and with any ending:
/// `C:\Models\NUL.stl` is the null device, not a file. `COM` and `LPT` are
/// device names with one of [`PORT_DIGITS`] after them.
const DEVICE_NAMES: [&str; 6] = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
const PORT_NAMES: [&str; 2] = ["COM", "LPT"];
/// What numbers a port: any one digit, 0 included, and the three raised
/// digits of the old Western code pages, which Windows reads as 1, 2 and 3
/// here (`COM¹` is `COM1`).
const PORT_DIGITS: &str = "0123456789\u{b9}\u{b2}\u{b3}";

/// Why the bridge will not open or write `path`, if it will not. Nothing
/// here touches the path itself: merely asking whether `\\server\share\x`
/// is a file makes Windows sign in to `server`. It goes by the text, and by
/// what kind of drive the path's letter stands for, which Windows knows
/// without going to the drive.
///
/// Refused, with either kind of slash: anything starting with two slashes
/// (a network share `\\server\share`, the long forms `\\?\C:\...` and
/// `\\?\UNC\...`, a device `\\.\COM1`), the kernel's own `\??\` form, a
/// colon anywhere but after a drive letter (a stream hidden behind a file,
/// `x.sclp:other`), a device name (`NUL`, `COM1`...) as any part, a path
/// with a zero byte in it, and a path on a drive letter that is a network
/// share under another name (`Z:\head.sclp` with `Z:` mapped to
/// `\\server\share`). A path on one of this computer's own drives
/// (`C:\Models\head.sclp`, `C:/Models/head.sclp`) and a relative one
/// (`models\head.obj`) pass.
pub fn off_limits(path: &str) -> Option<&'static str> {
    off_limits_given(path, is_network_drive)
}

/// [`off_limits`], with the question of which drive letters are network
/// shares left to `network_drive`: the answer is the machine's, and a test
/// has to be able to give its own.
fn off_limits_given(path: &str, network_drive: impl Fn(char) -> bool) -> Option<&'static str> {
    let path = path.replace('/', "\\");
    let letter = path.chars().next().filter(|c| c.is_ascii_alphabetic() && path.as_bytes().get(1) == Some(&b':'));
    let rest = if letter.is_some() { &path[2..] } else { &path[..] };
    let refused = path.starts_with("\\\\")
        || path.starts_with("\\??\\")
        || path.contains('\0')
        || rest.contains(':')
        || rest.split('\\').any(is_device_name)
        || letter.is_some_and(network_drive);
    refused.then_some(NOT_LOCAL)
}

/// Whether one part of a path names a device. Windows looks at the name up
/// to its first dot and ignores spaces after it: `nul`, `NUL.txt` and
/// `nul .obj` are all the null device.
fn is_device_name(part: &str) -> bool {
    let base = part.split('.').next().unwrap_or_default().trim_end_matches(' ');
    let is = |name: &str| name.eq_ignore_ascii_case(base);
    let is_port = |name: &str| {
        let Some((front, number)) = base.split_at_checked(name.len()) else { return false };
        let mut number = number.chars();
        let one_digit = number.next().is_some_and(|digit| PORT_DIGITS.contains(digit)) && number.next().is_none();
        one_digit && name.eq_ignore_ascii_case(front)
    };
    DEVICE_NAMES.into_iter().any(is) || PORT_NAMES.into_iter().any(is_port)
}

/// Whether a drive letter stands for a network share (`net use Z:
/// \\server\share`, or "Map network drive" in Explorer). Windows keeps the
/// kind of every drive letter and answers from that, without going to the
/// drive, so asking does not reach out to the server. A letter with no drive
/// behind it is not a network drive: the open or save that follows fails as
/// it would for any path that is not there.
#[cfg(windows)]
fn is_network_drive(letter: char) -> bool {
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetDriveTypeW(root: *const u16) -> u32;
    }
    const DRIVE_REMOTE: u32 = 4;
    let root: Vec<u16> = format!("{letter}:\\\0").encode_utf16().collect();
    // SAFETY: `root` is a string of UTF-16 ending in a zero, as the call asks, and outlives it.
    unsafe { GetDriveTypeW(root.as_ptr()) == DRIVE_REMOTE }
}

/// Only Windows has drive letters.
#[cfg(not(windows))]
fn is_network_drive(_: char) -> bool {
    false
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::net::{Ipv4Addr, SocketAddr};

    use super::*;

    fn key(digit: char) -> Key {
        Key(std::iter::repeat_n(digit, KEY_BYTES * 2).collect())
    }

    /// What a door with room for everyone says.
    fn no_door(shown: bool) -> Answer {
        if shown { Answer::Welcome } else { Answer::Refused }
    }

    /// The one line a connection that did not show the key gets.
    fn refusal() -> String {
        Answer::Refused.line()
    }

    /// Feeds `sent` to the app's side as one connection would, and gives
    /// back the tools that were run and everything that was written back.
    fn connection(sent: &[u8], key: &Key) -> (Vec<String>, String) {
        let (mut input, mut out, mut ran) = (Cursor::new(sent.to_vec()), Vec::new(), Vec::new());
        if admit(&mut input, &mut out, key, no_door) {
            requests(&mut input, &mut out, |request| {
                ran.push(request.tool);
                Some(json!({ "ok": true, "text": "done" }).to_string())
            });
        }
        (ran, String::from_utf8(out).unwrap())
    }

    fn refused(sent: &[u8], key: &Key) {
        let (ran, written) = connection(sent, key);
        let shown = String::from_utf8_lossy(&sent[..sent.len().min(60)]).into_owned();
        assert!(ran.is_empty(), "{shown:?} got {ran:?} run");
        assert_eq!(written, format!("{}\n", refusal()), "{shown:?}");
    }

    #[test]
    fn a_key_is_long_random_and_hex() {
        let (a, b) = (Key::new().unwrap(), Key::new().unwrap());
        for key in [&a, &b] {
            assert_eq!(key.0.len(), 64);
            assert!(key.0.bytes().all(|c| c.is_ascii_hexdigit()));
            // 32 random bytes are never all one value.
            assert!(key.0.bytes().any(|c| c != key.0.as_bytes()[0]), "{}", key.0);
            assert!(Key::from_text(&key.0).is_some());
        }
        assert!(!a.matches(&b.0), "two runs made the same key");
    }

    #[test]
    fn only_the_whole_key_matches() {
        let ours = key('a');
        assert!(ours.matches(&"a".repeat(64)));
        assert!(!ours.matches(&"a".repeat(63)));
        assert!(!ours.matches(&"a".repeat(65)));
        assert!(!ours.matches(&format!("{}b", "a".repeat(63))));
        assert!(!ours.matches(&format!("b{}", "a".repeat(63))));
        assert!(!ours.matches(""));
        // A file that does not hold a key is not read as one.
        assert!(Key::from_text("").is_none());
        assert!(Key::from_text(&"g".repeat(64)).is_none());
        assert!(Key::from_text(&"a".repeat(62)).is_none());
    }

    #[test]
    fn a_connection_that_shows_the_key_has_its_requests_run() {
        let ours = key('3');
        let sent = format!("{}\n{{\"tool\":\"get_state\",\"args\":{{}}}}\n{{\"tool\":\"quit\"}}\r\n", hello(&ours));
        let (ran, written) = connection(sent.as_bytes(), &ours);
        assert_eq!(ran, ["get_state", "quit"]);
        let lines: Vec<&str> = written.lines().collect();
        assert_eq!(lines.len(), 3, "{written}");
        assert_eq!(lines[0], r#"{"ok":true,"sculpt_bridge":1}"#);
        // The server's side reads that answer as a yes.
        let mut sent_by_server = Vec::new();
        let taken = knock(&mut Cursor::new(format!("{}\n", lines[0])), &mut sent_by_server, &ours);
        assert_eq!(taken, Answer::Welcome);
        assert_eq!(String::from_utf8(sent_by_server).unwrap(), format!("{}\n", hello(&ours)));
    }

    #[test]
    fn a_wrong_key_or_version_is_refused() {
        let ours = key('3');
        refused(format!("{}\n{{\"tool\":\"quit\"}}\n", hello(&key('4'))).as_bytes(), &ours);
        refused(b"{\"sculpt_bridge\":1}\n{\"tool\":\"quit\"}\n", &ours);
        refused(b"{\"sculpt_bridge\":1,\"token\":7}\n{\"tool\":\"quit\"}\n", &ours);
        // The right key from a build whose handshake is another version.
        let other_version = hello(&ours).replace("\"sculpt_bridge\":1", "\"sculpt_bridge\":2");
        assert!(other_version.contains(":2"));
        refused(format!("{other_version}\n{{\"tool\":\"quit\"}}\n").as_bytes(), &ours);
        // The server's side takes a refusal, and anything else that is said, as a no.
        let refused = format!("{}\n", refusal());
        for answer in [refused.as_str(), "{\"ok\":true,\"sculpt_bridge\":2}\n", "hello\n", "{\"ok\":true", "\n"] {
            assert_eq!(knock(&mut Cursor::new(answer), &mut Vec::new(), &ours), Answer::Refused, "{answer:?}");
        }
    }

    /// A line to nowhere: every write fails, as on a connection the other side has closed.
    struct Closed;

    impl Write for Closed {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(ErrorKind::ConnectionReset.into())
        }

        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn the_server_can_tell_no_turn_and_no_room_from_a_refusal() {
        let ours = key('3');
        // What the app says of each, and what the server makes of it, are the same thing.
        for answer in [Answer::Welcome, Answer::Refused, Answer::Full, Answer::NoTurn] {
            let (mut out, mut ran) = (Vec::new(), 0);
            let mut input = Cursor::new(format!("{}\n{{\"tool\":\"quit\"}}\n", hello(&ours)));
            if admit(&mut input, &mut out, &ours, |_| answer) {
                requests(&mut input, &mut out, |_| {
                    ran += 1;
                    None
                });
            }
            // Only the welcome gets anything run, whatever the first line was.
            assert_eq!(ran, usize::from(answer == Answer::Welcome), "{answer:?}");
            let said = String::from_utf8(out).unwrap();
            assert_eq!(said, format!("{}\n", answer.line()), "{answer:?}");
            assert_eq!(knock(&mut Cursor::new(said), &mut Vec::new(), &ours), answer);
        }
        // Each says what it is in words too, and a crowded door is not put down to the build.
        let says = |answer: Answer, words: &str| answer.line().contains(words);
        assert!(says(Answer::Refused, "same build") && says(Answer::Full, FULL) && says(Answer::NoTurn, NO_TURN));
        assert!(!FULL.contains("same build") && !NO_TURN.contains("same build") && !NO_TURN.contains("rebuild"));
        // The limit the words give is the limit there is.
        assert!(FULL.contains(&format!(" {MAX_ADMITTED} ")), "{FULL}");
        // Closed without a word, before or after the key went out: no turn.
        assert_eq!(knock(&mut Cursor::new(""), &mut Vec::new(), &ours), Answer::NoTurn);
        assert_eq!(knock(&mut Cursor::new(format!("{}\n", refusal())), &mut Closed, &ours), Answer::NoTurn);
        // The door is told what the first line was, and nothing else decides.
        let first_lines = [(hello(&ours), true), (hello(&key('4')), false), ("POST / HTTP/1.1".into(), false)];
        for (first_line, is_key) in first_lines {
            let mut told = None;
            admit(&mut Cursor::new(format!("{first_line}\n")), &mut Vec::new(), &ours, |shown| {
                told = Some(shown);
                Answer::Refused
            });
            assert_eq!(told, Some(is_key), "{first_line}");
        }
    }

    /// Something listening that is not the app: takes one connection and does `then` with it.
    fn stranger(then: impl FnOnce(TcpStream) + Send + 'static) -> SocketAddr {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        std::thread::spawn(move || then(listener.accept().unwrap().0));
        address
    }

    #[test]
    fn over_a_socket_a_closed_door_is_no_turn_and_a_silent_one_is_a_refusal() {
        let ours = key('3');
        // Taken and closed at once, the key unread: what a full app of the build before did.
        let (answer, _) = server(stranger(drop), &ours, Duration::from_secs(5));
        assert_eq!(answer, Answer::NoTurn);
        // Taken and never answered: not Sculpt, and the wait is the server's own.
        let hold = |stream: TcpStream| {
            std::thread::sleep(Duration::from_secs(2));
            drop(stream);
        };
        let started = Instant::now();
        let (answer, _) = server(stranger(hold), &ours, Duration::from_millis(300));
        assert_eq!(answer, Answer::Refused);
        assert!(started.elapsed() < Duration::from_millis(1500), "{:?}", started.elapsed());
        // Answered in some other tongue.
        let web = |mut stream: TcpStream| {
            let _ = stream.read(&mut [0; 256]);
            let _ = stream.write_all(b"HTTP/1.1 400 Bad Request\r\n\r\n");
        };
        assert_eq!(server(stranger(web), &ours, Duration::from_secs(5)).0, Answer::Refused);
    }

    #[test]
    fn a_request_from_before_the_key_existed_is_refused() {
        // What a server built before the handshake sends first: a tool call.
        refused(b"{\"tool\":\"quit\",\"args\":{}}\n", &key('3'));
        refused(b"{\"tool\":\"save_file\",\"args\":{\"path\":\"C:\\\\x.sclp\"}}\n", &key('3'));
    }

    #[test]
    fn a_web_page_s_request_is_refused() {
        let ours = key('3');
        // A form or a fetch from a page: the request line comes first, and a
        // line of JSON in the body is never reached.
        let post = "POST / HTTP/1.1\r\nHost: 127.0.0.1:47823\r\nContent-Type: text/plain\r\nContent-Length: 40\r\n\r\n\
                    {\"tool\":\"quit\",\"args\":{}}\n";
        refused(post.as_bytes(), &ours);
        // Not even with the real key further down.
        let with_key = format!("POST / HTTP/1.1\r\n\r\n{}\n{{\"tool\":\"quit\"}}\n", hello(&ours));
        refused(with_key.as_bytes(), &ours);
        refused(b"GET /?{\"tool\":\"quit\"} HTTP/1.1\r\n\r\n", &ours);
        refused(b"OPTIONS / HTTP/1.1\r\nOrigin: http://example.com\r\n\r\n", &ours);
    }

    #[test]
    fn noise_and_silence_are_refused() {
        let ours = key('3');
        refused(b"", &ours);
        refused(b"\n", &ours);
        refused(b"\r\n", &ours);
        refused(b"hello\n", &ours);
        refused(b"\xff\xfe\x00\x01\x02\n", &ours);
        refused(b"[]\n", &ours);
        refused(b"null\n", &ours);
        // An empty line is not skipped to find the handshake after it.
        refused(format!("\n{}\n{{\"tool\":\"quit\"}}\n", hello(&ours)).as_bytes(), &ours);
        // The handshake with no line ending: the peer went before finishing it.
        refused(hello(&ours).as_bytes(), &ours);
    }

    #[test]
    fn an_over_long_first_line_is_refused() {
        let ours = key('3');
        refused(format!("{}\n{{\"tool\":\"quit\"}}\n", "a".repeat(HELLO_LIMIT * 4)).as_bytes(), &ours);
        refused("a".repeat(HELLO_LIMIT * 4).as_bytes(), &ours);
        // The real handshake, padded past the limit with spaces JSON allows.
        let padded = format!("{}{}\n{{\"tool\":\"quit\"}}\n", " ".repeat(HELLO_LIMIT), hello(&ours));
        refused(padded.as_bytes(), &ours);
        // Padded to just inside it, it is still the handshake.
        let line = hello(&ours);
        let inside = format!("{}{line}\n{{\"tool\":\"quit\"}}\n", " ".repeat(HELLO_LIMIT - line.len()));
        assert_eq!(connection(inside.as_bytes(), &ours).0, ["quit"]);
    }

    #[test]
    fn a_line_that_is_not_a_request_ends_the_connection() {
        let ours = key('3');
        let good = "{\"tool\":\"get_state\"}";
        for bad in [
            "not json",
            "",
            "{\"tool\":",
            "[1,2]",
            "{}",
            "{\"tool\":\"\"}",
            "{\"tool\":7}",
            "{\"tool\":\"quit\",\"args\":[1]}",
            "POST / HTTP/1.1",
        ] {
            let sent = format!("{}\n{good}\n{bad}\n{{\"tool\":\"quit\"}}\n", hello(&ours));
            let (ran, written) = connection(sent.as_bytes(), &ours);
            // What came before it ran; it and everything after it did not.
            assert_eq!(ran, ["get_state"], "after {bad:?}");
            assert_eq!(written.lines().count(), 2, "after {bad:?}: {written}");
        }
    }

    #[test]
    fn an_over_long_request_ends_the_connection() {
        let ours = key('3');
        let long = format!("{{\"tool\":\"stroke\",\"args\":{{\"pad\":\"{}\"}}}}", "a".repeat(REQUEST_LIMIT));
        let sent = format!("{}\n{long}\n{{\"tool\":\"quit\"}}\n", hello(&ours));
        assert!(connection(sent.as_bytes(), &ours).0.is_empty());
        // One that fits is a request like any other.
        let fits = format!("{{\"tool\":\"stroke\",\"args\":{{\"pad\":\"{}\"}}}}", "a".repeat(REQUEST_LIMIT / 2));
        let sent = format!("{}\n{fits}\n{{\"tool\":\"quit\"}}\n", hello(&ours));
        assert_eq!(connection(sent.as_bytes(), &ours).0, ["stroke", "quit"]);
    }

    #[test]
    fn a_request_carries_its_tool_and_arguments() {
        let read = request(b"{\"tool\":\"pick\",\"args\":{\"x\":0.5,\"y\":0.25}}").unwrap();
        assert_eq!((read.tool.as_str(), read.args), ("pick", json!({ "x": 0.5, "y": 0.25 })));
        // No arguments, or a null in their place, is none.
        assert_eq!(request(b"{\"tool\":\"quit\"}").unwrap().args, json!({}));
        assert_eq!(request(b"{\"tool\":\"quit\",\"args\":null}").unwrap().args, json!({}));
    }

    fn scratch(name: &str) -> PathBuf {
        let folder = std::env::temp_dir().join(format!("sculpt-bridge-guard-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        folder
    }

    #[test]
    fn the_key_file_is_there_while_the_app_runs() {
        let folder = scratch("file");
        let ours = key('5');
        assert!(read_key(&folder, 47_999).is_none(), "nothing has been written yet");
        let posted = Posted::post(&folder, 47_999, &ours).expect("the folder is made and the file written");
        let found = read_key(&folder, 47_999).expect("the file reads back");
        assert!(found.matches(&ours.0));
        assert_eq!(read_file(&key_file(&folder, 47_999)).map(|found| found.pid), Some(std::process::id()));
        let found = std::fs::read_dir(&folder).unwrap().flatten();
        let names: Vec<String> = found.map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        assert_eq!(names, ["claude-bridge-47999.json"], "and nothing is left beside it");
        // Another port's bridge has another file.
        assert!(read_key(&folder, 47_998).is_none());
        // A file moved to another port's name is not that port's key.
        std::fs::copy(key_file(&folder, 47_999), key_file(&folder, 47_998)).unwrap();
        assert!(read_key(&folder, 47_998).is_none());
        // Closing properly takes the file away.
        drop(posted);
        assert!(!key_file(&folder, 47_999).exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_later_run_s_key_file_is_left_alone() {
        let folder = scratch("later");
        let first = Posted::post(&folder, 47_999, &key('5')).unwrap();
        // The first run crashed, say, and a second has started since.
        let second = Posted::post(&folder, 47_999, &key('6')).unwrap();
        drop(first);
        assert!(read_key(&folder, 47_999).is_some_and(|found| found.matches(&key('6').0)));
        drop(second);
        assert!(!key_file(&folder, 47_999).exists());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_key_left_by_a_run_that_is_gone_is_not_read() {
        let folder = scratch("gone");
        std::fs::create_dir_all(&folder).unwrap();
        // A process that has come and gone: this test program, asked to run nothing.
        let mut run = std::process::Command::new(std::env::current_exe().unwrap());
        run.args(["--exact", "no test has this name"]).stdout(std::process::Stdio::null());
        let mut gone = run.spawn().unwrap();
        let pid = gone.id();
        gone.wait().unwrap();
        let card = |pid: u32| {
            let card = KeyFile { about: String::new(), port: 47_999, pid, version: VERSION, token: key('5').0 };
            std::fs::write(key_file(&folder, 47_999), serde_json::to_string(&card).unwrap()).unwrap();
        };
        card(pid);
        assert!(!process_is_running(pid));
        assert!(read_key(&folder, 47_999).is_none(), "the key of a run that is gone was read");
        // The same file from a process that is still here is read.
        card(std::process::id());
        assert!(process_is_running(std::process::id()));
        assert!(read_key(&folder, 47_999).is_some());
        let _ = std::fs::remove_dir_all(&folder);
    }

    #[test]
    fn a_file_that_is_not_a_key_reads_as_none() {
        let folder = scratch("damaged");
        std::fs::create_dir_all(&folder).unwrap();
        let pid = std::process::id();
        let short = format!(r#"{{"port":47999,"pid":{pid},"version":1,"token":"short"}}"#);
        for text in ["", "not json", "{}", short.as_str()] {
            std::fs::write(key_file(&folder, 47_999), text).unwrap();
            assert!(read_key(&folder, 47_999).is_none(), "{text:?}");
        }
        let _ = std::fs::remove_dir_all(&folder);
    }

    /// A listener on a port the system picks, serving one connection with
    /// `key`; gives back the address and a handle that returns the tools run.
    fn one_connection(key: Key, hello_wait: Duration) -> (SocketAddr, std::thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let served = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let place = Door::new(1, 1).arrive();
            let mut ran = Vec::new();
            serve(stream, &key, place, hello_wait, |request| {
                ran.push(request.tool);
                Some(json!({ "ok": true, "text": "done" }).to_string())
            });
            ran
        });
        (address, served)
    }

    /// The tools that have been run, as the threads serving connections note them down.
    type Ran = Arc<Mutex<Vec<String>>>;

    /// The bridge as the app runs it ([`listen`]), on a port the system picks, with room for `waiting`
    /// connections that have not shown `key` and `admitted` that have.
    fn bridge(key: &Key, waiting: usize, admitted: usize, hello_wait: Duration) -> (SocketAddr, Arc<Door>, Ran) {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let address = listener.local_addr().unwrap();
        let (door, ran) = (Door::new(waiting, admitted), Ran::default());
        let (key, at, noted) = (key.clone(), door.clone(), ran.clone());
        std::thread::spawn(move || {
            listen(listener, key, at, hello_wait, move |request: Request| {
                noted.lock().unwrap().push(request.tool);
                Some(json!({ "ok": true, "text": "done" }).to_string())
            })
        });
        (address, door, ran)
    }

    /// What Claude's server does on connecting: shows the key, and waits up to `wait` for how it is taken.
    fn server(address: SocketAddr, key: &Key, wait: Duration) -> (Answer, BufReader<TcpStream>) {
        let stream = connected(address);
        stream.set_read_timeout(Some(wait)).unwrap();
        let answer = knock(&mut BufReader::new(&stream), &mut &stream, key);
        (answer, BufReader::new(stream))
    }

    /// One tool call over a link that has been admitted, and the line that comes back.
    fn ask(link: &mut BufReader<TcpStream>, tool: &str) -> String {
        writeln!(link.get_mut(), "{}", json!({ "tool": tool, "args": {} })).unwrap();
        let mut reply = String::new();
        link.read_line(&mut reply).unwrap();
        reply.trim_end().to_owned()
    }

    /// A connection, and not a word said on it. (A burst of them can find the listener's queue full for
    /// a moment on a busy machine; that is the machine's doing, so it is tried again.)
    fn connected(address: SocketAddr) -> TcpStream {
        let mut tries = 0;
        loop {
            match TcpStream::connect(address) {
                Ok(stream) => return stream,
                Err(e) if tries == 50 => panic!("could not connect: {e}"),
                Err(_) => std::thread::sleep(Duration::from_millis(20)),
            }
            tries += 1;
        }
    }

    /// Waits up to two seconds for `done` to come true.
    fn settles(done: impl Fn() -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(2);
        while !done() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(5));
        }
        done()
    }

    const DONE: &str = r#"{"ok":true,"text":"done"}"#;

    #[test]
    fn silent_connections_cannot_keep_a_keyed_one_out() {
        let ours = key('7');
        // As the app runs it: its own limits, and its own five seconds for a connection that says nothing.
        let (address, door, ran) = bridge(&ours, MAX_WAITING, MAX_ADMITTED, HELLO_WAIT);
        // More than may wait and may be admitted put together, none of them saying a word.
        let crowd = MAX_WAITING + MAX_ADMITTED + 8;
        let mut silent: Vec<TcpStream> = (0..crowd).map(|_| connected(address)).collect();
        let started = Instant::now();
        let (answer, mut link) = server(address, &ours, Duration::from_secs(5));
        assert_eq!(answer, Answer::Welcome, "a server with the key was kept out by connections without it");
        assert_eq!(ask(&mut link, "get_state"), DONE);
        let took = started.elapsed();
        assert!(took < Duration::from_secs(1), "served only after {took:?}");
        // And again while the crowd keeps coming, as one that reopens its connections does.
        for _ in 0..4 {
            silent.extend((0..MAX_WAITING).map(|_| connected(address)));
            let started = Instant::now();
            let (answer, mut another) = server(address, &ours, Duration::from_secs(5));
            assert_eq!(answer, Answer::Welcome);
            assert_eq!(ask(&mut another, "pick"), DONE);
            assert!(started.elapsed() < Duration::from_secs(1), "served only after {:?}", started.elapsed());
        }
        // The first server is still connected through all of it.
        assert_eq!(ask(&mut link, "get_state"), DONE);
        assert_eq!(ran.lock().unwrap().iter().filter(|tool| *tool == "get_state").count(), 2);
        // However many came, no more than the limit are being kept waiting. (One fewer, as it happens:
        // the last server took a silent connection's place on arriving, and then left the waiting itself.)
        assert!(settles(|| door.counts() == (MAX_WAITING - 1, 1)), "{:?}", door.counts());
        drop(silent);
        assert!(settles(|| door.counts() == (0, 1)), "{:?}", door.counts());
    }

    #[test]
    fn the_door_turns_out_whoever_has_waited_longest_and_never_one_with_a_seat() {
        let door = Door::new(2, 1);
        let turned_out = |place: &Place| place.turned_out.load(Ordering::Acquire);
        let (mut first, mut second) = (door.arrive(), door.arrive());
        assert_eq!(door.counts(), (2, 0));
        // A third: the first has waited longest and goes. Shown the key after all, it gets nothing.
        let mut third = door.arrive();
        assert!(turned_out(&first) && !turned_out(&second) && !turned_out(&third));
        assert_eq!(door.counts(), (2, 0));
        assert_eq!((first.answer(true), first.answer(false)), (Answer::NoTurn, Answer::NoTurn));
        // One that does not show the key is refused, and seats nobody.
        assert_eq!((third.answer(false), door.counts()), (Answer::Refused, (2, 0)));
        // The second shows the key and is seated: it is no longer one of the waiting, and no crowd moves it.
        assert_eq!(second.answer(true), Answer::Welcome);
        assert_eq!(door.counts(), (1, 1));
        let crowd: Vec<Place> = (0..10).map(|_| door.arrive()).collect();
        assert!(!turned_out(&second) && turned_out(&third), "the crowd took a seat, or left the waiting alone");
        assert_eq!(door.counts(), (2, 1));
        // The one seat is taken, so the next with the key is told so; it stops waiting all the same.
        let mut late = door.arrive();
        assert_eq!(late.answer(true), Answer::Full);
        assert_eq!(door.counts(), (1, 1));
        // Leaving gives the place up, whichever kind it was.
        drop((first, third, late, crowd));
        assert_eq!(door.counts(), (0, 1));
        drop(second);
        assert_eq!(door.counts(), (0, 0));
        // At its lowest, the limit on the waiting is one, not none: a door nobody can come to is no door.
        let narrow = Door::new(0, 1);
        let mut only = narrow.arrive();
        assert_eq!((narrow.counts(), only.answer(true)), ((1, 0), Answer::Welcome));
    }

    #[test]
    fn a_first_line_is_read_as_it_arrives_not_at_the_next_look() {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).unwrap();
        let mut far = connected(listener.local_addr().unwrap());
        let (near, _) = listener.accept().unwrap();
        // Nothing to read: the wait lasts as long as it was given, near enough.
        let started = Instant::now();
        wait_for_word(&near, Duration::from_millis(100));
        if cfg!(windows) {
            let waited = started.elapsed();
            assert!(waited >= Duration::from_millis(80) && waited < Duration::from_millis(900), "{waited:?}");
        }
        // A word arrives a twentieth of a second into a wait of a whole one: the wait ends there.
        let speaker = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            far.write_all(b"word\n").unwrap();
            far
        });
        let started = Instant::now();
        wait_for_word(&near, Duration::from_secs(1));
        let waited = started.elapsed();
        assert!(waited < Duration::from_millis(600), "the word was not noticed for {waited:?}");
        // The other side going is something to read too: a wait does not sit it out.
        drop(speaker.join().unwrap());
        let mut word = [0; 5];
        (&near).read_exact(&mut word).unwrap();
        let started = Instant::now();
        wait_for_word(&near, Duration::from_secs(1));
        assert!(started.elapsed() < Duration::from_millis(600), "{:?}", started.elapsed());
    }

    #[test]
    fn a_connection_closed_to_make_room_is_gone_at_once() {
        // Thirty seconds to show a key, so that nothing here ends by running out of time.
        let (address, door, _) = bridge(&key('7'), 2, 2, Duration::from_secs(30));
        let places = || Arc::strong_count(&door);
        let empty = places();
        let mut first = connected(address);
        let _second = connected(address);
        assert!(settles(|| door.counts() == (2, 0) && places() == empty + 2));
        // A third arrives: the first has waited longest, and is told it had no turn and closed.
        let _third = connected(address);
        first.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
        let (started, mut said) = (Instant::now(), String::new());
        first.read_to_string(&mut said).expect("closed, not left to time out");
        assert_eq!(said, format!("{}\n", Answer::NoTurn.line()));
        assert!(started.elapsed() < Duration::from_secs(2), "closed only after {:?}", started.elapsed());
        // Its thread has gone with it (each holds a place), not stayed to wait out its thirty seconds.
        let settled = settles(|| door.counts() == (2, 0) && places() == empty + 2);
        assert!(settled, "{:?} waiting and admitted, {} places", door.counts(), places() - empty);
    }

    #[test]
    fn the_admitted_have_a_limit_of_their_own_and_are_told_when_it_is_reached() {
        let ours = key('7');
        let (address, door, _) = bridge(&ours, 4, 2, Duration::from_secs(5));
        let wait = Duration::from_secs(5);
        let (first, mut one) = server(address, &ours, wait);
        let (second, mut two) = server(address, &ours, wait);
        assert_eq!((first, second), (Answer::Welcome, Answer::Welcome));
        // A third with the right key is told there is no room; nothing it sends after is run.
        let (third, _) = server(address, &ours, wait);
        assert_eq!(third, Answer::Full);
        // A crowd without the key takes no seat from the two that have one.
        let silent: Vec<TcpStream> = (0..20).map(|_| connected(address)).collect();
        assert!(settles(|| door.counts() == (4, 2)), "{:?}", door.counts());
        assert_eq!((ask(&mut one, "get_state"), ask(&mut two, "get_state")), (DONE.to_owned(), DONE.to_owned()));
        // One leaves, and there is room for another, the crowd still at the door.
        drop(one);
        assert!(settles(|| door.counts().1 == 1), "{:?}", door.counts());
        let (fourth, mut four) = server(address, &ours, wait);
        assert_eq!(fourth, Answer::Welcome);
        assert_eq!(ask(&mut four, "get_state"), DONE);
        drop(silent);
    }

    #[test]
    fn over_a_socket_the_key_opens_the_door_and_a_web_request_does_not() {
        let ours = key('7');
        let (address, served) = one_connection(ours.clone(), Duration::from_secs(5));
        let (answer, mut link) = server(address, &ours, Duration::from_secs(5));
        assert_eq!(answer, Answer::Welcome);
        assert_eq!(ask(&mut link, "get_state"), DONE);
        drop(link);
        assert_eq!(served.join().unwrap(), ["get_state"]);

        let (address, served) = one_connection(ours, Duration::from_secs(5));
        let mut page = TcpStream::connect(address).unwrap();
        let request = b"POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: 28\r\n\r\n{\"tool\":\"quit\",\"args\":{}}\n";
        page.write_all(request).unwrap();
        assert!(served.join().unwrap().is_empty(), "the body of a web request was run");
    }

    #[test]
    fn a_connection_that_says_nothing_is_closed() {
        let wait = Duration::from_millis(300);
        let (address, served) = one_connection(key('7'), wait);
        let started = Instant::now();
        let mut silent = TcpStream::connect(address).unwrap();
        // A byte now and then, never a whole line: each one would start a
        // plain read timeout again.
        let dribble = std::thread::spawn(move || {
            for _ in 0..40 {
                if silent.write_all(b" ").is_err() {
                    break;
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        });
        assert!(served.join().unwrap().is_empty());
        let took = started.elapsed();
        assert!(took >= wait && took < Duration::from_millis(1500), "closed after {took:?}");
        let _ = dribble.join();
    }

    #[test]
    fn network_and_device_paths_are_off_limits() {
        let refused = [
            r"\\server\share\head.sclp",
            r"\\server\share",
            r"\\192.168.1.9\c$\head.obj",
            r"\\?\UNC\server\share\head.sclp",
            r"\\?\C:\Models\head.sclp",
            r"\\.\COM1",
            r"\\.\PhysicalDrive0",
            r"\\.\pipe\name",
            r"\\?\GLOBALROOT\Device\Null",
            "//server/share/head.sclp",
            "//?/UNC/server/share/head.sclp",
            "//./COM1",
            r"\/server\share\head.sclp",
            r"/\server/share/head.sclp",
            r"\??\C:\Models\head.sclp",
            r"\??\UNC\server\share\head.sclp",
            r"C:\Models\head.sclp:hidden",
            r"C:\Models\head.sclp::$DATA",
            r"C:\Models\NUL",
            r"C:\Models\nul.stl",
            r"C:\Models\COM1.obj",
            r"C:\Models\con .sclp",
            r"C:\aux\head.sclp",
            r"C:\Models\LPT9.tar.stl",
            "CONOUT$",
            "nul",
            "C:\\Models\\head\0.sclp",
            r"1:\head.sclp",
        ];
        for path in refused {
            assert_eq!(off_limits_given(path, |_| false), Some(NOT_LOCAL), "{path:?} was let through");
        }
        let allowed = [
            r"C:\Models\head.sclp",
            "C:/Models/head.sclp",
            r"d:\a b\c & d\50% ^ 'x'.stl",
            r"D:\Sculptingapp2\target\w\-leading-dash.obj",
            r"C:\Models\console.sclp",
            r"C:\Models\com10.obj",
            r"C:\Models\null.stl",
            r"C:\Models\my.nul.stl",
            r"models\head.obj",
            "models/head.obj",
            r"..\head.obj",
            "head.obj",
            r"\Models\head.obj",
            "/home/zane/head.obj",
            r"C:head.obj",
            "",
        ];
        // Which drive letters are network shares is the machine's to say; here none is.
        for path in allowed {
            assert_eq!(off_limits_given(path, |_| false), None, "{path:?} was refused");
        }
    }

    #[test]
    fn every_device_name_is_off_limits_however_it_is_dressed() {
        // Every name Windows keeps for a device. A port is numbered by any one digit, 0 included, or by
        // a raised 1, 2 or 3.
        let mut names: Vec<String> = ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"].map(String::from).to_vec();
        for port in ["COM", "LPT"] {
            names.extend("0123456789\u{b9}\u{b2}\u{b3}".chars().map(|digit| format!("{port}{digit}")));
        }
        assert_eq!(names.len(), 6 + 2 * 13);
        let mut tried = 0;
        for name in &names {
            let lower = name.to_lowercase();
            let upper_if_even = |(at, c): (usize, char)| if at % 2 == 0 { c.to_ascii_uppercase() } else { c };
            let mixed: String = lower.chars().enumerate().map(upper_if_even).collect();
            for cased in [name, &lower, &mixed] {
                // An ending, several, dots and spaces after the name: Windows drops them all and finds the device.
                for dress in ["", ".stl", ".tar.stl", " ", "   ", ".", "...", " .obj", ". .", " . stl", ".sclp "] {
                    let part = format!("{cased}{dress}");
                    let places = [
                        part.clone(),
                        format!("C:\\Models\\{part}"),
                        format!("C:\\{part}\\head.sclp"),
                        format!("C:/Models/{part}/deeper/head.obj"),
                        format!("models\\{part}"),
                        format!("d:\\a\\..\\{part}"),
                    ];
                    for path in places {
                        assert_eq!(off_limits_given(&path, |_| false), Some(NOT_LOCAL), "{path:?} was let through");
                        tried += 1;
                    }
                }
            }
        }
        assert_eq!(tried, 32 * 3 * 11 * 6);
        // Names that only look like one are files like any other.
        let files = [
            "COM", "LPT", "COM10", "LPT10", "COM1x", "COM\u{b9}0", "COM\u{2074}", "LPT\u{b9}\u{b2}", "COMA", "CON1",
            "CONIN", "CONOUT", "CONIN$$", "NULL", "AUXILIARY", "PRN2", "xCOM1", "COM 1", "my.COM1", "my.nul", "\u{b9}",
            "COM\u{e9}",
        ];
        for name in files {
            for path in [name.to_owned(), format!("C:\\Models\\{name}.stl"), format!("C:\\{name}\\head.sclp")] {
                assert_eq!(off_limits_given(&path, |_| false), None, "{path:?} was refused");
            }
        }
    }

    #[test]
    fn a_drive_letter_that_stands_for_a_network_share_is_off_limits() {
        // A machine where Z: is mapped to a share (net use Z: \\server\share) and nothing else is.
        let mapped = |letter: char| letter.eq_ignore_ascii_case(&'z');
        for path in [r"Z:\head.sclp", r"z:\Models\head.obj", "Z:/Models/head.stl", r"Z:\", "Z:", "z:head.obj"] {
            assert_eq!(off_limits_given(path, mapped), Some(NOT_LOCAL), "{path:?} was let through");
        }
        // Only a drive letter is asked about: a Z anywhere else is part of a name.
        let local = [r"C:\Models\head.sclp", r"Y:\head.sclp", r"C:\Z\head.obj", r"models\z\head.obj", "Zebra.obj", "z"];
        for path in local {
            assert_eq!(off_limits_given(path, mapped), None, "{path:?} was refused");
        }
        let asked = std::cell::RefCell::new(Vec::new());
        let note = |letter: char| {
            asked.borrow_mut().push(letter);
            false
        };
        for path in [r"q:\Models\head.sclp", "R:/head.obj", r"\\server\share\q.obj", r"models\head.obj", ""] {
            let _ = off_limits_given(path, note);
        }
        // The letter asked about is the path's own, and only a path on a drive has one.
        assert_eq!(*asked.borrow(), ['q', 'R']);
        // What the machine itself says: the drive Windows is on is its own, so a path on it passes.
        if cfg!(windows) {
            let system = std::env::var("SystemDrive").ok().and_then(|drive| drive.chars().next()).unwrap_or('C');
            assert!(!is_network_drive(system), "{system}: is this computer's own");
            assert_eq!(off_limits(&format!("{system}:\\Models\\head.sclp")), None);
            assert_eq!(off_limits(&format!("{system}:\\Models\\COM0.stl")), Some(NOT_LOCAL));
        }
    }
}
